use std::num::NonZeroUsize;
use std::time::Duration;

use softwheel_resilience::{Bulkhead, CircuitBreaker, CircuitBreakerConfig, CircuitState};

fn breaker(failure_threshold: u32) -> CircuitBreaker {
    CircuitBreaker::new(
        CircuitBreakerConfig::new(failure_threshold, Duration::from_secs(60), 1)
            .expect("bounded property configuration is valid"),
    )
}

#[test]
fn shadow_breaker_failures_never_mutate_primary_state_across_thresholds() {
    for threshold in 1..=16 {
        let primary = breaker(threshold);
        let shadow = breaker(threshold);

        for failure in 1..=threshold {
            shadow
                .try_acquire()
                .expect("shadow breaker should admit until its threshold opens it")
                .failure();

            assert_eq!(
                primary.state(),
                CircuitState::Closed,
                "shadow failure {failure}/{threshold} mutated primary breaker state"
            );
            assert!(
                primary.try_acquire().is_ok(),
                "shadow failure {failure}/{threshold} consumed primary breaker admission"
            );
        }

        assert_eq!(shadow.state(), CircuitState::Open);
        assert_eq!(primary.state(), CircuitState::Closed);
    }
}

#[test]
fn shadow_bulkhead_saturation_never_consumes_primary_capacity() {
    for capacity in 1..=16 {
        let capacity = NonZeroUsize::new(capacity).unwrap();
        let primary = Bulkhead::new(capacity);
        let shadow = Bulkhead::new(capacity);

        let shadow_permits: Vec<_> = (0..capacity.get())
            .map(|_| {
                shadow
                    .try_acquire()
                    .expect("shadow should admit exactly its configured capacity")
            })
            .collect();

        assert!(shadow.try_acquire().is_err());
        assert_eq!(primary.in_flight(), 0);
        assert_eq!(primary.available(), capacity.get());

        let primary_permits: Vec<_> = (0..capacity.get())
            .map(|_| {
                primary
                    .try_acquire()
                    .expect("shadow saturation must not reduce primary capacity")
            })
            .collect();

        assert_eq!(primary.in_flight(), capacity.get());
        assert_eq!(primary.available(), 0);

        drop(shadow_permits);
        assert_eq!(shadow.in_flight(), 0);
        assert_eq!(primary.in_flight(), capacity.get());

        drop(primary_permits);
        assert_eq!(primary.in_flight(), 0);
    }
}
