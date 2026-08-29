use std::fmt;
use std::num::NonZeroU32;
use std::time::{Duration, Instant};

/// Whether a failed operation is safe and useful to retry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryDecision {
    Retry,
    DoNotRetry,
}

/// Jitter strategy applied to an exponential backoff cap.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Jitter {
    /// Deterministic exponential backoff. Useful for tests, but synchronized clients may herd.
    None,
    /// Uniformly sample between zero and the exponential cap.
    #[default]
    Full,
    /// Keep half the cap and uniformly sample the remaining half.
    Equal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackoffConfigError {
    ZeroInitialDelay,
    MaximumBelowInitial,
    FactorBelowOne,
}

impl fmt::Display for BackoffConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::ZeroInitialDelay => "initial delay must be greater than zero",
            Self::MaximumBelowInitial => "maximum delay must not be below initial delay",
            Self::FactorBelowOne => "backoff factor must be at least one",
        };
        f.write_str(message)
    }
}

impl std::error::Error for BackoffConfigError {}

/// Bounded exponential backoff with optional jitter.
#[derive(Clone, Debug)]
pub struct ExponentialBackoff {
    initial: Duration,
    maximum: Duration,
    factor: u32,
    jitter: Jitter,
}

impl ExponentialBackoff {
    pub fn new(
        initial: Duration,
        maximum: Duration,
        factor: u32,
        jitter: Jitter,
    ) -> Result<Self, BackoffConfigError> {
        if initial.is_zero() {
            return Err(BackoffConfigError::ZeroInitialDelay);
        }
        if maximum < initial {
            return Err(BackoffConfigError::MaximumBelowInitial);
        }
        if factor < 1 {
            return Err(BackoffConfigError::FactorBelowOne);
        }

        Ok(Self {
            initial,
            maximum,
            factor,
            jitter,
        })
    }

    /// Delay before the next attempt after `attempt` failed. Attempts are one-based.
    pub fn delay_after(&self, attempt: u32) -> Duration {
        let exponent = attempt.saturating_sub(1);
        let multiplier = self.factor.saturating_pow(exponent);
        let cap = self.initial.saturating_mul(multiplier).min(self.maximum);

        match self.jitter {
            Jitter::None => cap,
            Jitter::Full => random_duration(cap),
            Jitter::Equal => {
                let floor = cap / 2;
                floor.saturating_add(random_duration(cap.saturating_sub(floor)))
            }
        }
    }

    pub fn initial(&self) -> Duration {
        self.initial
    }

    pub fn maximum(&self) -> Duration {
        self.maximum
    }

    pub fn factor(&self) -> u32 {
        self.factor
    }

    pub fn jitter(&self) -> Jitter {
        self.jitter
    }
}

fn random_duration(maximum: Duration) -> Duration {
    let nanos = maximum.as_nanos().min(u64::MAX as u128) as u64;
    Duration::from_nanos(fastrand::u64(0..=nanos))
}

/// Finite retry policy. `max_attempts` includes the initial attempt.
#[derive(Clone, Debug)]
pub struct RetryPolicy {
    max_attempts: NonZeroU32,
    max_elapsed: Option<Duration>,
    backoff: ExponentialBackoff,
}

impl RetryPolicy {
    pub fn new(max_attempts: NonZeroU32, backoff: ExponentialBackoff) -> Self {
        Self {
            max_attempts,
            max_elapsed: None,
            backoff,
        }
    }

    pub fn with_max_elapsed(mut self, max_elapsed: Duration) -> Self {
        self.max_elapsed = Some(max_elapsed);
        self
    }

    /// Return the delay before another attempt, or `None` when the retry budget is exhausted.
    pub fn next_delay(
        &self,
        attempt: u32,
        elapsed: Duration,
        decision: RetryDecision,
    ) -> Option<Duration> {
        if decision == RetryDecision::DoNotRetry || attempt >= self.max_attempts.get() {
            return None;
        }

        let delay = self.backoff.delay_after(attempt);
        if let Some(max_elapsed) = self.max_elapsed {
            if elapsed.saturating_add(delay) > max_elapsed {
                return None;
            }
        }

        Some(delay)
    }

    pub fn max_attempts(&self) -> NonZeroU32 {
        self.max_attempts
    }

    pub fn max_elapsed(&self) -> Option<Duration> {
        self.max_elapsed
    }

    pub fn backoff(&self) -> &ExponentialBackoff {
        &self.backoff
    }
}

/// Execute a synchronous operation under a retry policy.
///
/// The classifier is intentionally supplied by the caller: transport failures and 5xx responses
/// are commonly retryable, while validation errors and most 4xx responses are not.
pub fn retry<T, E, Operation, Classify>(
    policy: &RetryPolicy,
    mut operation: Operation,
    classify: Classify,
) -> Result<T, E>
where
    Operation: FnMut() -> Result<T, E>,
    Classify: Fn(&E) -> RetryDecision,
{
    let started = Instant::now();
    let mut attempt = 1;

    loop {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) => {
                let decision = classify(&error);
                let Some(delay) = policy.next_delay(attempt, started.elapsed(), decision) else {
                    return Err(error);
                };

                std::thread::sleep(delay);
                attempt += 1;
            }
        }
    }
}

#[cfg(feature = "tokio")]
pub async fn retry_async<T, E, Operation, Future, Classify>(
    policy: &RetryPolicy,
    mut operation: Operation,
    classify: Classify,
) -> Result<T, E>
where
    Operation: FnMut() -> Future,
    Future: std::future::Future<Output = Result<T, E>>,
    Classify: Fn(&E) -> RetryDecision,
{
    let started = Instant::now();
    let mut attempt = 1;

    loop {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(error) => {
                let decision = classify(&error);
                let Some(delay) = policy.next_delay(attempt, started.elapsed(), decision) else {
                    return Err(error);
                };

                tokio::time::sleep(delay).await;
                attempt += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deterministic_backoff() -> ExponentialBackoff {
        ExponentialBackoff::new(
            Duration::from_millis(10),
            Duration::from_millis(80),
            2,
            Jitter::None,
        )
        .unwrap()
    }

    #[test]
    fn exponential_backoff_is_bounded() {
        let backoff = deterministic_backoff();
        assert_eq!(backoff.delay_after(1), Duration::from_millis(10));
        assert_eq!(backoff.delay_after(2), Duration::from_millis(20));
        assert_eq!(backoff.delay_after(3), Duration::from_millis(40));
        assert_eq!(backoff.delay_after(4), Duration::from_millis(80));
        assert_eq!(backoff.delay_after(20), Duration::from_millis(80));
    }

    #[test]
    fn full_jitter_never_exceeds_exponential_cap() {
        let backoff = ExponentialBackoff::new(
            Duration::from_millis(10),
            Duration::from_millis(80),
            2,
            Jitter::Full,
        )
        .unwrap();

        for attempt in 1..=12 {
            let exponent = attempt - 1;
            let expected_cap = Duration::from_millis(
                10_u64
                    .saturating_mul(2_u64.saturating_pow(exponent))
                    .min(80),
            );
            for _ in 0..256 {
                assert!(backoff.delay_after(attempt) <= expected_cap);
            }
        }
    }

    #[test]
    fn equal_jitter_stays_between_half_cap_and_cap() {
        let backoff = ExponentialBackoff::new(
            Duration::from_millis(10),
            Duration::from_millis(80),
            2,
            Jitter::Equal,
        )
        .unwrap();

        for attempt in 1..=12 {
            let exponent = attempt - 1;
            let cap = Duration::from_millis(
                10_u64
                    .saturating_mul(2_u64.saturating_pow(exponent))
                    .min(80),
            );
            let floor = cap / 2;
            for _ in 0..256 {
                let delay = backoff.delay_after(attempt);
                assert!(delay >= floor, "delay {delay:?} below floor {floor:?}");
                assert!(delay <= cap, "delay {delay:?} above cap {cap:?}");
            }
        }
    }

    #[test]
    fn retry_budget_stops_at_max_attempts() {
        let policy = RetryPolicy::new(NonZeroU32::new(3).unwrap(), deterministic_backoff());
        assert!(
            policy
                .next_delay(1, Duration::ZERO, RetryDecision::Retry)
                .is_some()
        );
        assert!(
            policy
                .next_delay(2, Duration::ZERO, RetryDecision::Retry)
                .is_some()
        );
        assert!(
            policy
                .next_delay(3, Duration::ZERO, RetryDecision::Retry)
                .is_none()
        );
    }

    #[test]
    fn max_elapsed_accepts_exact_boundary_and_rejects_overrun() {
        let policy = RetryPolicy::new(NonZeroU32::new(3).unwrap(), deterministic_backoff())
            .with_max_elapsed(Duration::from_millis(25));

        assert_eq!(
            policy.next_delay(1, Duration::from_millis(15), RetryDecision::Retry,),
            Some(Duration::from_millis(10))
        );
        assert_eq!(
            policy.next_delay(1, Duration::from_millis(16), RetryDecision::Retry,),
            None
        );
    }

    #[test]
    fn retry_executes_at_most_configured_attempts() {
        let policy = RetryPolicy::new(
            NonZeroU32::new(3).unwrap(),
            ExponentialBackoff::new(
                Duration::from_nanos(1),
                Duration::from_nanos(1),
                1,
                Jitter::None,
            )
            .unwrap(),
        );
        let mut calls = 0;

        let result: Result<(), &'static str> = retry(
            &policy,
            || {
                calls += 1;
                Err("transient")
            },
            |_| RetryDecision::Retry,
        );

        assert_eq!(result, Err("transient"));
        assert_eq!(calls, 3);
    }

    #[test]
    fn non_retryable_failure_stops_immediately() {
        let policy = RetryPolicy::new(NonZeroU32::new(3).unwrap(), deterministic_backoff());
        assert!(
            policy
                .next_delay(1, Duration::ZERO, RetryDecision::DoNotRetry)
                .is_none()
        );
    }
}
