#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

//! The implementation is intentionally split into small policy primitives so callers can
//! compose them without coupling their transport, async runtime, or service framework.

pub mod bulkhead;
pub mod circuit_breaker;
pub mod deadline;
pub mod eligibility;
pub mod failover;
pub mod observability;
pub mod resource_registry;
pub mod retry;
pub mod retry_budget;
pub mod routing;
pub mod shadow;
pub mod shadow_outcome;
pub mod shadow_policy;
#[cfg(feature = "tokio")]
pub mod tokio_runtime;
#[cfg(feature = "tower")]
pub mod tower_adapter;

pub use bulkhead::{Bulkhead, BulkheadCallError, BulkheadPermit, BulkheadRejected};
pub use circuit_breaker::{
    CircuitBreaker, CircuitBreakerCallError, CircuitBreakerConfig, CircuitBreakerConfigError,
    CircuitBreakerRejected, CircuitState,
};
pub use deadline::LogicalRequestBudget;
pub use eligibility::{RouteEligibility, RouteEligibilityError};
pub use failover::{
    RouteAttempt, RouteAttemptBudget, RouteDecision, RouteFailover, RouteFailoverError,
    RouteOutcome, RouteStopReason,
};
pub use observability::{
    BudgetStage, NoopObserver, Observer, OutcomeClass, RejectionReason, ResilienceEvent,
    RetrySuppressionReason, ShadowSamplingOutcome, TrafficRole,
};
pub use resource_registry::{
    PrimaryRouteResources, RouteResourcePolicy, RouteResourceRegistry, RouteResources,
    ShadowRouteResources,
};
pub use retry::{
    BackoffConfigError, ExponentialBackoff, Jitter, RetryDecision, RetryPolicy, retry,
};
pub use retry_budget::{
    PrimaryRetryBudget, RetryBudgetConfig, RetryBudgetDecision, ShadowRetryBudget,
};
pub use routing::{Route, RouteId, RouteTable, RouteTableError, RouteTableStore};
pub use shadow::{
    RoutePlan, RoutePlanError, RoutePlanner, SHADOW_PARTS_PER_MILLION, ShadowSampling,
    ShadowSamplingError,
};
pub use shadow_outcome::{ShadowObservation, ShadowOutcome};
pub use shadow_policy::{ShadowExecutionPolicy, ShadowPolicyError};

#[cfg(feature = "tokio")]
pub use retry::retry_async;
#[cfg(feature = "tokio")]
pub use tokio_runtime::{TokioExecutionStop, TokioRequestBudget};
#[cfg(feature = "tower")]
pub use tower_adapter::{
    TowerRequestFactory, TowerRetryError, TowerRetryLayer, TowerRetryService,
};
