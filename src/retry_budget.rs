use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

/// Configuration for a shared cross-request retry budget.
///
/// The budget uses success-based replenishment rather than an internal clock. This keeps the
/// accounting deterministic and runtime-agnostic: callers consume one token immediately before a
/// retry attempt starts and report successful physical attempts explicitly.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetryBudgetConfig {
    capacity: NonZeroU32,
    replenish_per_success: NonZeroU32,
}

impl RetryBudgetConfig {
    pub const fn new(capacity: NonZeroU32, replenish_per_success: NonZeroU32) -> Self {
        Self {
            capacity,
            replenish_per_success,
        }
    }

    pub const fn capacity(self) -> NonZeroU32 {
        self.capacity
    }

    pub const fn replenish_per_success(self) -> NonZeroU32 {
        self.replenish_per_success
    }
}

/// Result of trying to reserve budget for one retry attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryBudgetDecision {
    /// The retry may start. `remaining` is the number of retry tokens left after admission.
    Admitted { remaining: u32 },
    /// No retry token is available. The retry must be suppressed.
    Suppressed,
}

#[derive(Debug)]
struct RetryBudgetState {
    available: u32,
}

#[derive(Clone, Debug)]
struct SharedRetryBudget {
    config: RetryBudgetConfig,
    state: Arc<Mutex<RetryBudgetState>>,
}

impl SharedRetryBudget {
    fn new(config: RetryBudgetConfig) -> Self {
        Self {
            config,
            state: Arc::new(Mutex::new(RetryBudgetState {
                available: config.capacity.get(),
            })),
        }
    }

    fn try_acquire_retry(&self) -> RetryBudgetDecision {
        let mut state = self.lock_state();
        if state.available == 0 {
            return RetryBudgetDecision::Suppressed;
        }

        state.available -= 1;
        RetryBudgetDecision::Admitted {
            remaining: state.available,
        }
    }

    fn record_success(&self) -> u32 {
        let mut state = self.lock_state();
        state.available = state
            .available
            .saturating_add(self.config.replenish_per_success.get())
            .min(self.config.capacity.get());
        state.available
    }

    fn available_retries(&self) -> u32 {
        self.lock_state().available
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, RetryBudgetState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

macro_rules! retry_budget_namespace {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        ///
        /// Cloning this value intentionally shares accounting across requests in the same
        /// namespace. Primary and shadow budgets are distinct public types so an adapter cannot
        /// accidentally substitute one for the other.
        #[derive(Clone, Debug)]
        pub struct $name {
            inner: SharedRetryBudget,
        }

        impl $name {
            /// Create a full retry budget.
            pub fn new(config: RetryBudgetConfig) -> Self {
                Self {
                    inner: SharedRetryBudget::new(config),
                }
            }

            /// Reserve one token immediately before starting a retry physical attempt.
            ///
            /// Initial physical attempts do not call this method and therefore never require a
            /// retry token. A suppressed retry is a policy stop; it does not rewrite the outcome
            /// of the physical attempt that already completed.
            pub fn try_acquire_retry(&self) -> RetryBudgetDecision {
                self.inner.try_acquire_retry()
            }

            /// Replenish tokens after a successful physical attempt, saturating at capacity.
            ///
            /// The caller supplies the success signal explicitly, so the core budget neither
            /// reads a clock nor classifies transport/application outcomes itself.
            pub fn record_success(&self) -> u32 {
                self.inner.record_success()
            }

            /// Return the currently available number of retry tokens.
            pub fn available_retries(&self) -> u32 {
                self.inner.available_retries()
            }

            pub const fn config(&self) -> RetryBudgetConfig {
                self.inner.config
            }
        }
    };
}

retry_budget_namespace!(
    PrimaryRetryBudget,
    "Shared retry-storm budget for primary traffic."
);
retry_budget_namespace!(
    ShadowRetryBudget,
    "Shared retry-storm budget for isolated shadow traffic."
);

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    use std::thread;

    fn config(capacity: u32, replenish_per_success: u32) -> RetryBudgetConfig {
        RetryBudgetConfig::new(
            NonZeroU32::new(capacity).unwrap(),
            NonZeroU32::new(replenish_per_success).unwrap(),
        )
    }

    #[test]
    fn retries_consume_tokens_until_suppressed() {
        let budget = PrimaryRetryBudget::new(config(2, 1));

        assert_eq!(
            budget.try_acquire_retry(),
            RetryBudgetDecision::Admitted { remaining: 1 }
        );
        assert_eq!(
            budget.try_acquire_retry(),
            RetryBudgetDecision::Admitted { remaining: 0 }
        );
        assert_eq!(budget.try_acquire_retry(), RetryBudgetDecision::Suppressed);
    }

    #[test]
    fn successes_replenish_with_saturation() {
        let budget = PrimaryRetryBudget::new(config(3, 2));
        assert!(matches!(
            budget.try_acquire_retry(),
            RetryBudgetDecision::Admitted { .. }
        ));
        assert!(matches!(
            budget.try_acquire_retry(),
            RetryBudgetDecision::Admitted { .. }
        ));

        assert_eq!(budget.record_success(), 3);
        assert_eq!(budget.record_success(), 3);
        assert_eq!(budget.available_retries(), 3);
    }

    #[test]
    fn clones_share_cross_request_accounting() {
        let first_request = PrimaryRetryBudget::new(config(1, 1));
        let second_request = first_request.clone();

        assert_eq!(
            first_request.try_acquire_retry(),
            RetryBudgetDecision::Admitted { remaining: 0 }
        );
        assert_eq!(
            second_request.try_acquire_retry(),
            RetryBudgetDecision::Suppressed
        );

        second_request.record_success();
        assert_eq!(first_request.available_retries(), 1);
    }

    #[test]
    fn primary_and_shadow_namespaces_are_isolated() {
        let primary = PrimaryRetryBudget::new(config(1, 1));
        let shadow = ShadowRetryBudget::new(config(1, 1));

        assert!(matches!(
            shadow.try_acquire_retry(),
            RetryBudgetDecision::Admitted { .. }
        ));
        assert_eq!(shadow.try_acquire_retry(), RetryBudgetDecision::Suppressed);

        assert_eq!(primary.available_retries(), 1);
        assert_eq!(
            primary.try_acquire_retry(),
            RetryBudgetDecision::Admitted { remaining: 0 }
        );
    }

    #[test]
    fn concurrent_admission_never_exceeds_capacity() {
        const CAPACITY: u32 = 8;
        const CALLERS: usize = 32;

        let budget = PrimaryRetryBudget::new(config(CAPACITY, 1));
        let barrier = Arc::new(Barrier::new(CALLERS));
        let mut handles = Vec::with_capacity(CALLERS);

        for _ in 0..CALLERS {
            let budget = budget.clone();
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                barrier.wait();
                matches!(
                    budget.try_acquire_retry(),
                    RetryBudgetDecision::Admitted { .. }
                )
            }));
        }

        let admitted = handles
            .into_iter()
            .filter(|handle| handle.join().unwrap())
            .count();
        assert_eq!(admitted, CAPACITY as usize);
        assert_eq!(budget.available_retries(), 0);
    }
}
