use std::num::NonZeroU32;
use std::time::Duration;

use softwheel_resilience::{
    BackoffConfigError, ExponentialBackoff, Jitter, RetryDecision, RetryPolicy,
};

#[test]
fn backoff_configuration_validation_is_stable_across_bounded_inputs() {
    for initial_ms in 0..=8_u64 {
        for maximum_ms in 0..=8_u64 {
            for factor in 0..=4_u32 {
                let result = ExponentialBackoff::new(
                    Duration::from_millis(initial_ms),
                    Duration::from_millis(maximum_ms),
                    factor,
                    Jitter::None,
                );

                let expected_error = if initial_ms == 0 {
                    Some(BackoffConfigError::ZeroInitialDelay)
                } else if maximum_ms < initial_ms {
                    Some(BackoffConfigError::MaximumBelowInitial)
                } else if factor < 1 {
                    Some(BackoffConfigError::FactorBelowOne)
                } else {
                    None
                };

                assert_eq!(result.err(), expected_error);
            }
        }
    }
}

#[test]
fn deterministic_backoff_is_monotonic_and_never_exceeds_its_cap() {
    for initial_ms in 1..=8_u64 {
        for maximum_multiplier in 1..=8_u64 {
            let maximum_ms = initial_ms.saturating_mul(maximum_multiplier);
            for factor in 1..=4_u32 {
                let backoff = ExponentialBackoff::new(
                    Duration::from_millis(initial_ms),
                    Duration::from_millis(maximum_ms),
                    factor,
                    Jitter::None,
                )
                .unwrap();

                let mut previous = Duration::ZERO;
                for attempt in 1..=32 {
                    let delay = backoff.delay_after(attempt);
                    assert!(delay >= previous);
                    assert!(delay <= Duration::from_millis(maximum_ms));
                    previous = delay;
                }
            }
        }
    }
}

#[test]
fn retry_policy_never_admits_past_attempt_or_elapsed_budget() {
    let backoff = ExponentialBackoff::new(
        Duration::from_millis(1),
        Duration::from_millis(16),
        2,
        Jitter::None,
    )
    .unwrap();

    for max_attempts in 1..=8_u32 {
        for max_elapsed_ms in 0..=32_u64 {
            let policy = RetryPolicy::new(NonZeroU32::new(max_attempts).unwrap(), backoff.clone())
                .with_max_elapsed(Duration::from_millis(max_elapsed_ms));

            for attempt in 1..=10_u32 {
                for elapsed_ms in 0..=40_u64 {
                    let elapsed = Duration::from_millis(elapsed_ms);
                    let decision = policy.next_delay(attempt, elapsed, RetryDecision::Retry);

                    if attempt >= max_attempts {
                        assert!(decision.is_none());
                        continue;
                    }

                    if let Some(delay) = decision {
                        assert!(elapsed.saturating_add(delay) <= Duration::from_millis(max_elapsed_ms));
                    }
                }
            }
        }
    }
}

#[test]
fn non_retryable_decision_is_terminal_for_every_bounded_policy() {
    for max_attempts in 1..=8_u32 {
        let backoff = ExponentialBackoff::new(
            Duration::from_millis(1),
            Duration::from_millis(16),
            2,
            Jitter::None,
        )
        .unwrap();
        let policy = RetryPolicy::new(NonZeroU32::new(max_attempts).unwrap(), backoff);

        for attempt in 1..=10_u32 {
            for elapsed_ms in 0..=16_u64 {
                assert_eq!(
                    policy.next_delay(
                        attempt,
                        Duration::from_millis(elapsed_ms),
                        RetryDecision::DoNotRetry,
                    ),
                    None
                );
            }
        }
    }
}
