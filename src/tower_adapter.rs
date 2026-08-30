use std::fmt;
use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use tower::{Layer, Service};

use crate::{RetryDecision, RetryPolicy, TokioExecutionStop, TokioRequestBudget};

/// Explicit replay source for Tower requests.
///
/// Retries cannot safely assume arbitrary request values are cloneable. Callers therefore supply a
/// factory that creates a fresh request for every physical attempt.
pub struct TowerRequestFactory<F> {
    make: F,
    budget: TokioRequestBudget,
    attempt_timeout: Duration,
}

impl<F> TowerRequestFactory<F> {
    /// Create an explicit replay source for one logical request.
    pub const fn new(make: F, budget: TokioRequestBudget, attempt_timeout: Duration) -> Self {
        Self {
            make,
            budget,
            attempt_timeout,
        }
    }

    /// Return the logical-request budget shared by all physical attempts.
    pub const fn budget(&self) -> TokioRequestBudget {
        self.budget
    }

    /// Return the requested timeout for each physical attempt.
    pub const fn attempt_timeout(&self) -> Duration {
        self.attempt_timeout
    }
}

/// Error returned by the Tower retry adapter.
#[derive(Debug)]
pub enum TowerRetryError<E> {
    /// The wrapped service failed while being polled for readiness/backpressure capacity.
    Readiness(E),
    /// A physical downstream call failed and no further retry was admitted.
    Call(E),
    /// Tokio runtime mechanics stopped execution before another downstream result existed.
    Runtime(TokioExecutionStop),
}

impl<E: fmt::Display> fmt::Display for TowerRetryError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Readiness(error) => write!(f, "service readiness failed: {error}"),
            Self::Call(error) => write!(f, "service call failed: {error}"),
            Self::Runtime(stop) => write!(f, "runtime stopped execution: {stop:?}"),
        }
    }
}

impl<E> std::error::Error for TowerRetryError<E>
where
    E: std::error::Error + 'static,
{
}

/// Optional Tower layer for explicit, bounded physical retries.
///
/// The layer delegates outer `poll_ready` directly to the wrapped service. Downstream call errors
/// may be retried according to the verified core [`RetryPolicy`], but readiness failures are
/// returned distinctly and are never passed to the retry classifier. Every retry uses the same
/// [`TokioRequestBudget`], so backoff and subsequent readiness waits cannot extend the logical
/// deadline.
///
/// The request itself is an explicit [`TowerRequestFactory`]; this adapter never clones an opaque
/// Tower request behind the caller's back.
///
/// ```
/// # #[cfg(feature = "tower")]
/// # async fn example() {
/// use std::num::NonZeroU32;
/// use std::time::Duration;
/// use softwheel_resilience::{
///     ExponentialBackoff, Jitter, LogicalRequestBudget, RetryDecision, RetryPolicy,
///     TokioRequestBudget, TowerRequestFactory, TowerRetryLayer,
/// };
/// use tower::{Layer, Service};
///
/// # #[derive(Clone)]
/// # struct Echo;
/// # impl Service<u32> for Echo {
/// #     type Response = u32;
/// #     type Error = std::convert::Infallible;
/// #     type Future = std::future::Ready<Result<u32, Self::Error>>;
/// #     fn poll_ready(&mut self, _: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), Self::Error>> { std::task::Poll::Ready(Ok(())) }
/// #     fn call(&mut self, request: u32) -> Self::Future { std::future::ready(Ok(request)) }
/// # }
/// let backoff = ExponentialBackoff::new(
///     Duration::from_millis(1),
///     Duration::from_millis(1),
///     1,
///     Jitter::None,
/// ).unwrap();
/// let policy = RetryPolicy::new(NonZeroU32::new(2).unwrap(), backoff);
/// let layer = TowerRetryLayer::new(policy, |_: &std::convert::Infallible| RetryDecision::DoNotRetry);
/// let mut service = layer.layer(Echo);
/// let budget = TokioRequestBudget::start(LogicalRequestBudget::bounded(Duration::from_secs(1)));
/// let request = TowerRequestFactory::new(|| 7, budget, Duration::from_millis(50));
/// std::future::poll_fn(|cx| service.poll_ready(cx)).await.unwrap();
/// assert_eq!(service.call(request).await.unwrap(), 7);
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct TowerRetryLayer<C> {
    policy: RetryPolicy,
    classify: C,
}

impl<C> TowerRetryLayer<C> {
    pub const fn new(policy: RetryPolicy, classify: C) -> Self {
        Self { policy, classify }
    }
}

impl<S, C> Layer<S> for TowerRetryLayer<C>
where
    C: Clone,
{
    type Service = TowerRetryService<S, C>;

    fn layer(&self, inner: S) -> Self::Service {
        TowerRetryService {
            inner,
            policy: self.policy.clone(),
            classify: self.classify.clone(),
        }
    }
}

/// Tower service produced by [`TowerRetryLayer`].
#[derive(Clone, Debug)]
pub struct TowerRetryService<S, C> {
    inner: S,
    policy: RetryPolicy,
    classify: C,
}

impl<S, C, F, Request> Service<TowerRequestFactory<F>> for TowerRetryService<S, C>
where
    S: Service<Request> + Clone + Send + 'static,
    S::Response: Send + 'static,
    S::Error: Send + 'static,
    S::Future: Send + 'static,
    C: Fn(&S::Error) -> RetryDecision + Clone + Send + 'static,
    F: FnMut() -> Request + Send + 'static,
    Request: Send + 'static,
{
    type Response = S::Response;
    type Error = TowerRetryError<S::Error>;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match self.inner.poll_ready(cx) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
            Poll::Ready(Err(error)) => Poll::Ready(Err(TowerRetryError::Readiness(error))),
            Poll::Pending => Poll::Pending,
        }
    }

    fn call(&mut self, mut request: TowerRequestFactory<F>) -> Self::Future {
        // Preserve the exact instance whose readiness permit was observed by the caller. The
        // replacement clone remains in `self` for the next outer readiness cycle.
        let replacement = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, replacement);
        let policy = self.policy.clone();
        let classify = self.classify.clone();

        Box::pin(async move {
            let started = Instant::now();
            let mut attempt = 1;

            loop {
                let call = async {
                    let physical_request = (request.make)();
                    inner.call(physical_request).await
                };

                let result = request
                    .budget
                    .timeout(request.attempt_timeout, call)
                    .await
                    .map_err(TowerRetryError::Runtime)?;

                match result {
                    Ok(response) => return Ok(response),
                    Err(error) => {
                        let decision = classify(&error);
                        let Some(delay) = policy.next_delay(attempt, started.elapsed(), decision)
                        else {
                            return Err(TowerRetryError::Call(error));
                        };

                        request
                            .budget
                            .sleep_backoff(delay)
                            .await
                            .map_err(TowerRetryError::Runtime)?;
                        attempt += 1;

                        let readiness = poll_fn(|cx| inner.poll_ready(cx));
                        if let Some(remaining) = request.budget.remaining() {
                            request
                                .budget
                                .timeout(remaining, readiness)
                                .await
                                .map_err(TowerRetryError::Runtime)?
                                .map_err(TowerRetryError::Readiness)?;
                        } else {
                            readiness.await.map_err(TowerRetryError::Readiness)?;
                        }
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExponentialBackoff, Jitter, LogicalRequestBudget};
    use std::num::NonZeroU32;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Clone)]
    struct TestService {
        calls: Arc<AtomicUsize>,
        ready_polls: Arc<AtomicUsize>,
        fail_calls: usize,
        fail_ready_after: Option<usize>,
    }

    impl Service<usize> for TestService {
        type Response = usize;
        type Error = &'static str;
        type Future = std::future::Ready<Result<usize, Self::Error>>;

        fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            let poll = self.ready_polls.fetch_add(1, Ordering::SeqCst) + 1;
            if self.fail_ready_after.is_some_and(|limit| poll > limit) {
                Poll::Ready(Err("not ready"))
            } else {
                Poll::Ready(Ok(()))
            }
        }

        fn call(&mut self, request: usize) -> Self::Future {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call <= self.fail_calls {
                std::future::ready(Err("downstream"))
            } else {
                std::future::ready(Ok(request))
            }
        }
    }

    fn policy(delay: Duration, attempts: u32) -> RetryPolicy {
        let backoff = ExponentialBackoff::new(delay, delay, 1, Jitter::None).unwrap();
        RetryPolicy::new(NonZeroU32::new(attempts).unwrap(), backoff)
    }

    fn budget(duration: Duration) -> TokioRequestBudget {
        TokioRequestBudget::start(LogicalRequestBudget::bounded(duration))
    }

    #[tokio::test]
    async fn retries_use_explicit_factory_and_preserve_readiness() {
        let calls = Arc::new(AtomicUsize::new(0));
        let ready_polls = Arc::new(AtomicUsize::new(0));
        let service = TestService {
            calls: calls.clone(),
            ready_polls: ready_polls.clone(),
            fail_calls: 2,
            fail_ready_after: None,
        };
        let layer = TowerRetryLayer::new(policy(Duration::from_millis(1), 3), |_| {
            RetryDecision::Retry
        });
        let mut service = layer.layer(service);
        poll_fn(|cx| service.poll_ready(cx)).await.unwrap();

        let made = Arc::new(AtomicUsize::new(0));
        let made_for_factory = made.clone();
        let request = TowerRequestFactory::new(
            move || made_for_factory.fetch_add(1, Ordering::SeqCst) + 1,
            budget(Duration::from_secs(1)),
            Duration::from_millis(100),
        );

        assert_eq!(service.call(request).await.unwrap(), 3);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(made.load(Ordering::SeqCst), 3);
        assert_eq!(ready_polls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retry_readiness_failure_is_not_classified_as_call_failure() {
        let service = TestService {
            calls: Arc::new(AtomicUsize::new(0)),
            ready_polls: Arc::new(AtomicUsize::new(0)),
            fail_calls: usize::MAX,
            fail_ready_after: Some(1),
        };
        let layer = TowerRetryLayer::new(policy(Duration::from_millis(1), 2), |_| {
            RetryDecision::Retry
        });
        let mut service = layer.layer(service);
        poll_fn(|cx| service.poll_ready(cx)).await.unwrap();

        let result = service
            .call(TowerRequestFactory::new(
                || 1,
                budget(Duration::from_secs(1)),
                Duration::from_millis(100),
            ))
            .await;

        assert!(matches!(result, Err(TowerRetryError::Readiness("not ready"))));
    }

    #[tokio::test]
    async fn logical_budget_suppresses_retry_before_sleeping_past_deadline() {
        let calls = Arc::new(AtomicUsize::new(0));
        let service = TestService {
            calls: calls.clone(),
            ready_polls: Arc::new(AtomicUsize::new(0)),
            fail_calls: usize::MAX,
            fail_ready_after: None,
        };
        let layer = TowerRetryLayer::new(policy(Duration::from_millis(20), 2), |_| {
            RetryDecision::Retry
        });
        let mut service = layer.layer(service);
        poll_fn(|cx| service.poll_ready(cx)).await.unwrap();

        let result = service
            .call(TowerRequestFactory::new(
                || 1,
                budget(Duration::from_millis(10)),
                Duration::from_millis(5),
            ))
            .await;

        assert!(matches!(
            result,
            Err(TowerRetryError::Runtime(
                TokioExecutionStop::BackoffWouldExhaustBudget
            ))
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
