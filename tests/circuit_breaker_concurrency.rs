use std::sync::{Arc, Barrier};
use std::time::Duration;

use softwheel_resilience::{CircuitBreaker, CircuitBreakerConfig, CircuitState};

fn breaker(open_timeout: Duration, half_open_success_threshold: u32) -> CircuitBreaker {
    CircuitBreaker::new(
        CircuitBreakerConfig::new(1, open_timeout, half_open_success_threshold).unwrap(),
    )
}

#[test]
fn stale_closed_outcome_cannot_mutate_new_generation() {
    let breaker = breaker(Duration::from_secs(60), 1);

    let stale = breaker.try_acquire().unwrap();
    breaker.try_acquire().unwrap().failure();
    assert_eq!(breaker.state(), CircuitState::Open);

    stale.success();

    assert_eq!(breaker.state(), CircuitState::Open);
    assert!(breaker.try_acquire().is_err());
}

#[test]
fn half_open_concurrent_admission_never_exceeds_probe_budget() {
    const THREADS: usize = 8;
    let breaker = Arc::new(breaker(Duration::from_millis(2), 1));
    breaker.try_acquire().unwrap().failure();
    std::thread::sleep(Duration::from_millis(5));
    assert_eq!(breaker.state(), CircuitState::HalfOpen);

    let start = Arc::new(Barrier::new(THREADS));
    let finish = Arc::new(Barrier::new(THREADS + 1));
    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            let breaker = Arc::clone(&breaker);
            let start = Arc::clone(&start);
            let finish = Arc::clone(&finish);
            std::thread::spawn(move || {
                start.wait();
                let permit = breaker.try_acquire().ok();
                finish.wait();
                permit
            })
        })
        .collect();

    finish.wait();

    let admitted = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .filter(|permit| permit.is_some())
        .count();

    assert_eq!(admitted, 1);
}

#[test]
fn abandoned_half_open_probe_releases_capacity() {
    let breaker = breaker(Duration::from_millis(2), 1);
    breaker.try_acquire().unwrap().failure();
    std::thread::sleep(Duration::from_millis(5));
    assert_eq!(breaker.state(), CircuitState::HalfOpen);

    let permit = breaker.try_acquire().unwrap();
    assert!(breaker.try_acquire().is_err());
    drop(permit);

    assert!(breaker.try_acquire().is_ok());
}
