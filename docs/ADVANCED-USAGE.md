# Advanced primitive composition

These APIs remain supported for transport implementors and custom policies. For Envoy v3 policy configuration, start with the README.

## Retry example

```rust
use std::num::NonZeroU32;
use std::time::Duration;
use softwheel_resilience::{retry, ExponentialBackoff, Jitter, RetryDecision, RetryPolicy};

#[derive(Debug, PartialEq)]
struct CallError {
    transient: bool,
}

let backoff = ExponentialBackoff::new(
    Duration::from_millis(1),
    Duration::from_millis(4),
    2,
    Jitter::None,
).unwrap();
let policy = RetryPolicy::new(NonZeroU32::new(3).unwrap(), backoff);
let mut calls = 0;

let result = retry(
    &policy,
    || {
        calls += 1;
        if calls < 2 {
            Err(CallError { transient: true })
        } else {
            Ok("ok")
        }
    },
    |error| {
        if error.transient {
            RetryDecision::Retry
        } else {
            RetryDecision::DoNotRetry
        }
    },
);

assert_eq!(result, Ok("ok"));
assert_eq!(calls, 2);
```

The classifier is deliberately application-owned. A resilience library cannot safely assume that
an arbitrary error is retryable; retrying non-idempotent operations or permanent failures can
amplify an incident.

## Routing and shadowing example

The M1 APIs keep one immutable routing generation and one health/policy eligibility decision for a
logical request. Shadow planning is diagnostic only and cannot replace the primary result.

```rust
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;
use softwheel_resilience::{
    Route, RouteAttemptBudget, RouteDecision, RouteEligibility, RouteFailover, RouteId,
    RouteOutcome, RoutePlanner, RouteTable, RouteTableStore, ShadowExecutionPolicy,
    ShadowObservation, ShadowOutcome, ShadowSampling,
};

let table = RouteTable::new(
    7,
    vec![
        Route::new(RouteId::new("primary-a").unwrap(), 3),
        Route::new(RouteId::new("primary-b").unwrap(), 2),
        Route::new(RouteId::new("draining").unwrap(), 100),
    ],
)
.unwrap();
let store = RouteTableStore::new(table);
let snapshot = store.snapshot();

let eligibility = RouteEligibility::from_predicate(&snapshot, |route| {
    route.id().as_str() != "draining"
});

let plan = RoutePlanner::plan_eligible_with(
    &snapshot,
    &eligibility,
    ShadowSampling::always(),
    |_| 0,
    |_| unreachable!(),
    |_| 0,
)
.unwrap();
assert_eq!(plan.primary().as_str(), "primary-a");
assert_eq!(plan.shadow().unwrap().as_str(), "primary-b");
assert_eq!(plan.generation(), 7);

let observation = ShadowObservation::from_plan(&plan).unwrap();
assert_eq!(observation.route_id().as_str(), "primary-b");
let shadow_outcome: ShadowOutcome<&str> = ShadowOutcome::Failed("diagnostic failure");
assert_eq!(shadow_outcome.error(), Some(&"diagnostic failure"));

let shadow_policy = ShadowExecutionPolicy::new(Duration::from_millis(50)).unwrap();
assert!(!shadow_policy.retries_enabled());
assert_eq!(
    shadow_policy.effective_deadline(Duration::from_millis(20)),
    Duration::from_millis(20)
);

let budget = RouteAttemptBudget::new(NonZeroUsize::new(2).unwrap());
let mut failover = RouteFailover::from_primary_eligibility(
    Arc::clone(&snapshot),
    eligibility,
    budget,
    plan.primary().clone(),
)
.unwrap();

let decision = failover.classify_with(RouteOutcome::Failover, |_| 0).unwrap();
match decision {
    RouteDecision::Failover(next) => {
        assert_eq!(next.ordinal(), 1);
        assert_eq!(next.route_id().as_str(), "primary-b");
    }
    other => panic!("unexpected decision: {other:?}"),
}
```
