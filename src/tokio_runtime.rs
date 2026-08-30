use std::future::Future;
use std::time::Duration;

use crate::LogicalRequestBudget;

/// Runtime stop reason produced by Tokio mechanics without reclassifying downstream outcomes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TokioExecutionStop {
    /// The caller-provided cancellation future completed before the operation.
    CallerCancelled,
    /// The outer logical-request budget expired.
    LogicalDeadlineExceeded,
    /// A caller-requested child timeout expired before the outer logical deadline.
    ChildTimeout,
    /// A backoff sleep would consume all remaining logical-request budget.
    BackoffWouldExhaustBudget,
}

/// Tokio clock binding for a runtime-agnostic [`LogicalRequestBudget`].
///
/// Policy remains in the core budget type. This adapter only supplies a monotonic Tokio clock,
/// timeout enforcement, bounded sleeping, and explicit cancellation translation.
///
/// ```
/// # #[cfg(feature = "tokio")]
/// # async fn example() {
/// use std::time::Duration;
/// use softwheel_resilience::{LogicalRequestBudget, TokioRequestBudget};
///
/// let budget = TokioRequestBudget::start(LogicalRequestBudget::bounded(
///     Duration::from_millis(100),
/// ));
/// let value = budget
///     .timeout(Duration::from_millis(50), async { 42 })
///     .await
///     .unwrap();
/// assert_eq!(value, 42);
/// # }
/// ```
#[derive(Clone, Copy, Debug)]
pub struct TokioRequestBudget {
    core: LogicalRequestBudget,
    started: tokio::time::Instant,
}

impl TokioRequestBudget {
    /// Bind a logical-request budget to the current Tokio monotonic instant.
    pub fn start(core: LogicalRequestBudget) -> Self {
        Self::from_start(core, tokio::time::Instant::now())
    }

    /// Bind a logical-request budget to an existing Tokio monotonic start instant.
    ///
    /// This is useful when multiple adapter layers must share exactly one logical-request clock.
    pub const fn from_start(core: LogicalRequestBudget, started: tokio::time::Instant) -> Self {
        Self { core, started }
    }

    /// Return the runtime-agnostic core budget.
    pub const fn core(&self) -> LogicalRequestBudget {
        self.core
    }

    /// Return elapsed time on Tokio's monotonic clock.
    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// Return remaining logical-request budget, or `None` when unbounded.
    pub fn remaining(&self) -> Option<Duration> {
        self.core.remaining(self.elapsed())
    }

    /// Enforce a child timeout clamped to the remaining logical-request budget.
    ///
    /// The returned future owns and polls `future` directly; no task is spawned. Dropping this
    /// future therefore propagates cancellation by dropping the child future as well.
    pub async fn timeout<F>(
        &self,
        requested: Duration,
        future: F,
    ) -> Result<F::Output, TokioExecutionStop>
    where
        F: Future,
    {
        let elapsed = self.elapsed();
        if self.core.is_exhausted(elapsed) {
            return Err(TokioExecutionStop::LogicalDeadlineExceeded);
        }

        let remaining = self.core.remaining(elapsed);
        let effective = self.core.clamp_child(elapsed, requested);
        let stop = match remaining {
            Some(remaining) if remaining <= requested => {
                TokioExecutionStop::LogicalDeadlineExceeded
            }
            _ => TokioExecutionStop::ChildTimeout,
        };

        tokio::time::timeout(effective, future)
            .await
            .map_err(|_| stop)
    }

    /// Enforce the clamped timeout while translating an explicit caller-cancellation signal.
    ///
    /// Cancellation is a policy stop, not a downstream failure. The adapter does not spawn either
    /// branch, so whichever branch loses is dropped immediately.
    pub async fn timeout_or_cancel<F, C>(
        &self,
        requested: Duration,
        future: F,
        cancelled: C,
    ) -> Result<F::Output, TokioExecutionStop>
    where
        F: Future,
        C: Future<Output = ()>,
    {
        tokio::select! {
            biased;
            _ = cancelled => Err(TokioExecutionStop::CallerCancelled),
            result = self.timeout(requested, future) => result,
        }
    }

    /// Sleep for retry backoff only when positive logical-request budget remains afterwards.
    ///
    /// The core's strict `delay < remaining` rule is checked before sleeping. The budget is checked
    /// again after wake-up so runtime scheduling delay cannot silently start another attempt after
    /// the outer deadline.
    pub async fn sleep_backoff(&self, delay: Duration) -> Result<(), TokioExecutionStop> {
        let elapsed = self.elapsed();
        if !self.core.can_wait(elapsed, delay) {
            return Err(TokioExecutionStop::BackoffWouldExhaustBudget);
        }

        tokio::time::sleep(delay).await;
        if self.core.is_exhausted(self.elapsed()) {
            return Err(TokioExecutionStop::LogicalDeadlineExceeded);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future;

    #[tokio::test]
    async fn completed_child_is_returned_without_reclassification() {
        let budget = TokioRequestBudget::start(LogicalRequestBudget::bounded(Duration::from_secs(1)));

        let result = budget.timeout(Duration::from_millis(50), async { 7 }).await;

        assert_eq!(result, Ok(7));
    }

    #[tokio::test]
    async fn exhausted_outer_budget_starts_no_child_work() {
        let budget = TokioRequestBudget::start(LogicalRequestBudget::bounded(Duration::ZERO));
        let mut polled = false;
        let child = future::poll_fn(|_| {
            polled = true;
            std::task::Poll::Ready(())
        });

        let result = budget.timeout(Duration::from_secs(1), child).await;

        assert_eq!(result, Err(TokioExecutionStop::LogicalDeadlineExceeded));
        assert!(!polled);
    }

    #[tokio::test]
    async fn ready_cancellation_is_not_classified_as_downstream_failure() {
        let budget = TokioRequestBudget::start(LogicalRequestBudget::bounded(Duration::from_secs(1)));

        let result = budget
            .timeout_or_cancel(
                Duration::from_secs(1),
                future::pending::<()>(),
                future::ready(()),
            )
            .await;

        assert_eq!(result, Err(TokioExecutionStop::CallerCancelled));
    }

    #[tokio::test]
    async fn backoff_equal_to_remaining_budget_is_suppressed() {
        let budget = TokioRequestBudget::start(LogicalRequestBudget::bounded(Duration::ZERO));

        let result = budget.sleep_backoff(Duration::ZERO).await;

        assert_eq!(result, Err(TokioExecutionStop::BackoffWouldExhaustBudget));
    }

    #[tokio::test]
    async fn zero_backoff_is_allowed_when_positive_budget_remains() {
        let budget = TokioRequestBudget::start(LogicalRequestBudget::bounded(Duration::from_secs(1)));

        assert_eq!(budget.sleep_backoff(Duration::ZERO).await, Ok(()));
    }
}
