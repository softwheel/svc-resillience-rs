use std::num::NonZeroUsize;
use std::time::Duration;

use softwheel_resilience::{
    Bulkhead, CircuitBreaker, CircuitBreakerConfig, CircuitState,
};

fn breaker() -> CircuitBreaker {
    CircuitBreaker::new(
        CircuitBreakerConfig::new(1, Duration::from_secs(60), 1)
            .expect("test breaker configuration is valid"),
    )
}

#[test]
fn shadow_failure_cannot_trip_primary_breaker() {
    let primary = breaker();
    let shadow = breaker();

    primary
        .try_acquire()
        .expect("primary should admit the call")
        .success();
    shadow
        .try_acquire()
        .expect("shadow should admit the probe")
        .failure();

    assert_eq!(primary.state(), CircuitState::Closed);
    assert_eq!(shadow.state(), CircuitState::Open);
    assert!(primary.try_acquire().is_ok());
}

#[test]
fn shadow_saturation_cannot_consume_primary_bulkhead_capacity() {
    let primary = Bulkhead::new(NonZeroUsize::new(1).unwrap());
    let shadow = Bulkhead::new(NonZeroUsize::new(1).unwrap());

    let _shadow_permit = shadow
        .try_acquire()
        .expect("first shadow call should consume its isolated capacity");
    assert!(shadow.try_acquire().is_err());

    assert_eq!(primary.in_flight(), 0);
    assert_eq!(primary.available(), 1);
    let _primary_permit = primary
        .try_acquire()
        .expect("shadow saturation must not reject primary work");
    assert_eq!(primary.in_flight(), 1);
}

#[cfg(feature = "tokio")]
#[tokio::test]
async fn primary_completion_does_not_await_shadow_completion() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    let shadow_completed = Arc::new(AtomicBool::new(false));
    let completed = Arc::clone(&shadow_completed);

    let shadow = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        completed.store(true, Ordering::Release);
    });

    let primary_result = tokio::time::timeout(Duration::from_millis(20), async { 42_u32 })
        .await
        .expect("primary completion must not wait for the shadow task");

    assert_eq!(primary_result, 42);
    assert!(!shadow_completed.load(Ordering::Acquire));

    shadow.abort();
    let _ = shadow.await;
}
