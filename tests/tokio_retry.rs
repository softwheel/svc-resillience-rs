#![cfg(feature = "tokio")]

use softwheel_resilience::{ExponentialBackoff, Jitter, RetryDecision, RetryPolicy, retry_async};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

#[tokio::test]
async fn retries_transient_failures_until_success() {
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
    let calls = Arc::new(AtomicUsize::new(0));
    let operation_calls = Arc::clone(&calls);

    let result = retry_async(
        &policy,
        move || {
            let calls = Arc::clone(&operation_calls);
            async move {
                let attempt = calls.fetch_add(1, Ordering::SeqCst) + 1;
                if attempt < 3 {
                    Err("transient")
                } else {
                    Ok("ok")
                }
            }
        },
        |_| RetryDecision::Retry,
    )
    .await;

    assert_eq!(result, Ok("ok"));
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn non_retryable_failure_stops_after_first_async_attempt() {
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
    let calls = Arc::new(AtomicUsize::new(0));
    let operation_calls = Arc::clone(&calls);

    let result: Result<(), &'static str> = retry_async(
        &policy,
        move || {
            let calls = Arc::clone(&operation_calls);
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Err("permanent")
            }
        },
        |_| RetryDecision::DoNotRetry,
    )
    .await;

    assert_eq!(result, Err("permanent"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
