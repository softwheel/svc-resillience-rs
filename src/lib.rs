#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

//! The implementation is intentionally split into small policy primitives so callers can
//! compose them without coupling their transport, async runtime, or service framework.

pub mod bulkhead;
pub mod circuit_breaker;
pub mod retry;
pub mod routing;

pub use bulkhead::{Bulkhead, BulkheadCallError, BulkheadPermit, BulkheadRejected};
pub use circuit_breaker::{
    CircuitBreaker, CircuitBreakerCallError, CircuitBreakerConfig, CircuitBreakerConfigError,
    CircuitBreakerRejected, CircuitState,
};
pub use retry::{
    BackoffConfigError, ExponentialBackoff, Jitter, RetryDecision, RetryPolicy, retry,
};
pub use routing::{Route, RouteId, RouteTable, RouteTableError, RouteTableStore};

#[cfg(feature = "tokio")]
pub use retry::retry_async;
