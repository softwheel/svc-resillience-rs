use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use softwheel_resilience::{
    CircuitBreaker, CircuitBreakerConfig, CircuitBreakerRejected, CircuitState,
};

fn breaker(
    failure_threshold: u32,
    half_open_success_threshold: u32,
) -> CircuitBreaker {
    CircuitBreaker::new(
        CircuitBreakerConfig::new(
            failure_threshold,
            Duration::from_millis(1),
            half_open_success_threshold,
        )
        .unwrap(),
    )
}

fn trip_and_wait_for_half_open(breaker: &CircuitBreaker) {
    breaker.try_acquire().unwrap().failure();
    assert_eq!(breaker.state(), CircuitState::Open);
    thread::sleep(Duration::from_millis(5));
    assert_eq!(breaker.state(), CircuitState::HalfOpen);
}

#[test]
fn half_open_failure_reopens_immediately() {
    let breaker = breaker(1, 1);
    trip_and_wait_for_half_open(&breaker);

    breaker.try_acquire().unwrap().failure();

    assert_eq!(breaker.state(), CircuitState::Open);
    assert_eq!(breaker.try_acquire().unwrap_err(), CircuitBreakerRejected);
}

#[test]
fn dropped_half_open_permit_releases_exactly_one_probe_slot() {
    let breaker = breaker(1, 1);
    trip_and_wait_for_half_open(&breaker);

    let permit = breaker.try_acquire().unwrap();
    assert_eq!(breaker.try_acquire().unwrap_err(), CircuitBreakerRejected);

    drop(permit);

    let replacement = breaker.try_acquire();
    assert!(replacement.is_ok());
    assert_eq!(breaker.try_acquire().unwrap_err(), CircuitBreakerRejected);
}

#[test]
fn stale_closed_result_cannot_mutate_a_newer_generation() {
    let breaker = breaker(1, 1);

    let stale_permit = breaker.try_acquire().unwrap();
    let trip_permit = breaker.try_acquire().unwrap();
    trip_permit.failure();
    assert_eq!(breaker.state(), CircuitState::Open);

    thread::sleep(Duration::from_millis(5));
    assert_eq!(breaker.state(), CircuitState::HalfOpen);
    breaker.try_acquire().unwrap().success();
    assert_eq!(breaker.state(), CircuitState::Closed);

    stale_permit.failure();

    assert_eq!(breaker.state(), CircuitState::Closed);
    assert!(breaker.try_acquire().is_ok());
}

#[test]
fn half_open_concurrent_admission_never_exceeds_probe_limit() {
    const WORKERS: usize = 8;
    const PROBE_LIMIT: usize = 2;

    let breaker = breaker(1, PROBE_LIMIT as u32);
    trip_and_wait_for_half_open(&breaker);

    let start = Arc::new(std::sync::Barrier::new(WORKERS + 1));
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let (tx, rx) = mpsc::channel();
    let mut handles = Vec::with_capacity(WORKERS);

    for _ in 0..WORKERS {
        let breaker = breaker.clone();
        let start = Arc::clone(&start);
        let release = Arc::clone(&release);
        let tx = tx.clone();
        handles.push(thread::spawn(move || {
            start.wait();
            match breaker.try_acquire() {
                Ok(permit) => {
                    tx.send(true).unwrap();
                    let (lock, cvar) = &*release;
                    let mut released = lock.lock().unwrap();
                    while !*released {
                        released = cvar.wait(released).unwrap();
                    }
                    drop(permit);
                }
                Err(CircuitBreakerRejected) => tx.send(false).unwrap(),
            }
        }));
    }
    drop(tx);

    start.wait();
    let outcomes: Vec<bool> = (0..WORKERS).map(|_| rx.recv().unwrap()).collect();
    assert_eq!(outcomes.iter().filter(|&&admitted| admitted).count(), PROBE_LIMIT);

    let (lock, cvar) = &*release;
    *lock.lock().unwrap() = true;
    cvar.notify_all();

    for handle in handles {
        handle.join().unwrap();
    }

    assert_eq!(breaker.state(), CircuitState::HalfOpen);
    assert!(breaker.try_acquire().is_ok());
}
