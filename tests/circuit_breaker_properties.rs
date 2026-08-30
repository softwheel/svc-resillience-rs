use std::time::Duration;

use softwheel_resilience::{
    CircuitBreaker, CircuitBreakerConfig, CircuitBreakerConfigError, CircuitBreakerRejected,
    CircuitState,
};

#[test]
fn bounded_config_validation_has_stable_error_precedence() {
    for failure_threshold in 0..=3 {
        for timeout_ms in 0..=3 {
            for half_open_success_threshold in 0..=3 {
                let result = CircuitBreakerConfig::new(
                    failure_threshold,
                    Duration::from_millis(timeout_ms),
                    half_open_success_threshold,
                );

                let expected = if failure_threshold == 0 {
                    Err(CircuitBreakerConfigError::ZeroFailureThreshold)
                } else if timeout_ms == 0 {
                    Err(CircuitBreakerConfigError::ZeroOpenTimeout)
                } else if half_open_success_threshold == 0 {
                    Err(CircuitBreakerConfigError::ZeroHalfOpenSuccessThreshold)
                } else {
                    Ok(())
                };

                assert_eq!(result.map(|_| ()), expected);
            }
        }
    }
}

#[test]
fn bounded_closed_state_sequences_match_consecutive_failure_model() {
    const MAX_SEQUENCE_LEN: u32 = 8;

    for failure_threshold in 1..=MAX_SEQUENCE_LEN {
        for sequence_len in 0..=MAX_SEQUENCE_LEN {
            let sequence_count = 1_u32 << sequence_len;

            for sequence in 0..sequence_count {
                let breaker = CircuitBreaker::new(
                    CircuitBreakerConfig::new(
                        failure_threshold,
                        Duration::from_secs(60),
                        1,
                    )
                    .unwrap(),
                );
                let mut consecutive_failures = 0_u32;
                let mut expected_open = false;

                for step in 0..sequence_len {
                    let is_failure = sequence & (1_u32 << step) != 0;

                    if expected_open {
                        assert_eq!(breaker.state(), CircuitState::Open);
                        assert_eq!(breaker.try_acquire().unwrap_err(), CircuitBreakerRejected);
                        continue;
                    }

                    let permit = breaker.try_acquire().unwrap();
                    if is_failure {
                        permit.failure();
                        consecutive_failures += 1;
                        expected_open = consecutive_failures >= failure_threshold;
                    } else {
                        permit.success();
                        consecutive_failures = 0;
                    }

                    assert_eq!(
                        breaker.state(),
                        if expected_open {
                            CircuitState::Open
                        } else {
                            CircuitState::Closed
                        }
                    );
                }
            }
        }
    }
}
