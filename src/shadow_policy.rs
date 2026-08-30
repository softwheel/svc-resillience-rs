use std::fmt;
use std::time::Duration;

use crate::retry::{RetryDecision, RetryPolicy};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShadowPolicyError {
    ZeroDeadline,
}

impl fmt::Display for ShadowPolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroDeadline => f.write_str("shadow deadline must be greater than zero"),
        }
    }
}

impl std::error::Error for ShadowPolicyError {}

/// Runtime-agnostic execution budget for diagnostic shadow traffic.
///
/// The conservative default is a single physical shadow attempt with no retry policy. Callers may
/// opt into retries explicitly with [`ShadowExecutionPolicy::with_retry_policy`]. The deadline is a
/// hard outer budget: adapters should enforce it around the complete shadow operation, including
/// transport execution, retry sleeps, and cancellation.
///
/// The effective shadow deadline is always clamped to the primary request's remaining budget.
///
/// ```
/// use std::time::Duration;
/// use softwheel_resilience::ShadowExecutionPolicy;
///
/// let policy = ShadowExecutionPolicy::new(Duration::from_millis(50)).unwrap();
/// assert!(!policy.retries_enabled());
/// assert_eq!(
///     policy.effective_deadline(Duration::from_millis(20)),
///     Duration::from_millis(20)
/// );
/// ```
#[derive(Clone, Debug)]
pub struct ShadowExecutionPolicy {
    deadline: Duration,
    retry_policy: Option<RetryPolicy>,
}

impl ShadowExecutionPolicy {
    /// Create a bounded shadow policy with retries disabled.
    pub fn new(deadline: Duration) -> Result<Self, ShadowPolicyError> {
        if deadline.is_zero() {
            return Err(ShadowPolicyError::ZeroDeadline);
        }

        Ok(Self {
            deadline,
            retry_policy: None,
        })
    }

    /// Explicitly opt shadow execution into an independent retry policy.
    pub fn with_retry_policy(mut self, retry_policy: RetryPolicy) -> Self {
        self.retry_policy = Some(retry_policy);
        self
    }

    pub fn deadline(&self) -> Duration {
        self.deadline
    }

    pub fn effective_deadline(&self, primary_remaining: Duration) -> Duration {
        self.deadline.min(primary_remaining)
    }

    pub fn retries_enabled(&self) -> bool {
        self.retry_policy.is_some()
    }

    pub fn retry_policy(&self) -> Option<&RetryPolicy> {
        self.retry_policy.as_ref()
    }

    /// Return the delay before another shadow attempt, clamped by both retry and shadow budgets.
    ///
    /// This method only governs retry admission and sleep. Runtime adapters must separately enforce
    /// [`ShadowExecutionPolicy::effective_deadline`] around transport execution so a slow physical
    /// attempt cannot outlive the shadow budget.
    pub fn next_retry_delay(
        &self,
        attempt: u32,
        elapsed: Duration,
        decision: RetryDecision,
    ) -> Option<Duration> {
        let retry_policy = self.retry_policy.as_ref()?;
        let delay = retry_policy.next_delay(attempt, elapsed, decision)?;
        (elapsed.saturating_add(delay) <= self.deadline).then_some(delay)
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use crate::retry::{ExponentialBackoff, Jitter};

    use super::*;

    fn retry_policy() -> RetryPolicy {
        RetryPolicy::new(
            NonZeroU32::new(3).unwrap(),
            ExponentialBackoff::new(
                Duration::from_millis(10),
                Duration::from_millis(10),
                1,
                Jitter::None,
            )
            .unwrap(),
        )
    }

    #[test]
    fn deadline_must_be_non_zero() {
        assert_eq!(
            ShadowExecutionPolicy::new(Duration::ZERO).unwrap_err(),
            ShadowPolicyError::ZeroDeadline
        );
    }

    #[test]
    fn conservative_policy_disables_retries() {
        let policy = ShadowExecutionPolicy::new(Duration::from_millis(50)).unwrap();

        assert!(!policy.retries_enabled());
        assert!(policy.retry_policy().is_none());
        assert_eq!(
            policy.next_retry_delay(1, Duration::ZERO, RetryDecision::Retry),
            None
        );
    }

    #[test]
    fn effective_deadline_never_exceeds_primary_remaining_budget() {
        let policy = ShadowExecutionPolicy::new(Duration::from_millis(50)).unwrap();

        assert_eq!(
            policy.effective_deadline(Duration::from_millis(80)),
            Duration::from_millis(50)
        );
        assert_eq!(
            policy.effective_deadline(Duration::from_millis(20)),
            Duration::from_millis(20)
        );
    }

    #[test]
    fn retries_require_explicit_opt_in_and_remain_shadow_bounded() {
        let policy = ShadowExecutionPolicy::new(Duration::from_millis(15))
            .unwrap()
            .with_retry_policy(retry_policy());

        assert!(policy.retries_enabled());
        assert_eq!(
            policy.next_retry_delay(1, Duration::ZERO, RetryDecision::Retry),
            Some(Duration::from_millis(10))
        );
        assert_eq!(
            policy.next_retry_delay(2, Duration::from_millis(10), RetryDecision::Retry,),
            None
        );
        assert_eq!(
            policy.next_retry_delay(1, Duration::ZERO, RetryDecision::DoNotRetry),
            None
        );
    }
}
