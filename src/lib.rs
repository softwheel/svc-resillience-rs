#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

//! The implementation is intentionally split into small policy primitives so callers can
//! compose them without coupling their transport, async runtime, or service framework.

pub mod bulkhead;
pub mod circuit_breaker;
pub mod failover;
pub mod retry;
pub mod routing;
pub mod shadow;

pub use bulkhead::{Bulkhead, BulkheadCallError, BulkheadPermit, BulkheadRejected};
pub use circuit_breaker::{
    CircuitBreaker, CircuitBreakerCallError, CircuitBreakerConfig, CircuitBreakerConfigError,
    CircuitBreakerRejected, CircuitState,
};
pub use failover::{
    RouteAttempt, RouteAttemptBudget, RouteDecision, RouteFailover, RouteFailoverError,
    RouteOutcome, RouteStopReason,
};
pub use retry::{
    BackoffConfigError, ExponentialBackoff, Jitter, RetryDecision, RetryPolicy, retry,
};
pub use routing::{Route, RouteId, RouteTable, RouteTableError, RouteTableStore};
pub use shadow::{
    RoutePlan, RoutePlanError, RoutePlanner, SHADOW_PARTS_PER_MILLION, ShadowSampling,
    ShadowSamplingError,
};

#[cfg(feature = "tokio")]
pub use retry::retry_async;
