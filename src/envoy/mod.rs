//! Envoy v3 policy configuration for embedded Rust callers.
//!
//! Accepts a strict subset of `RouteAction` and `CircuitBreakers` JSON fragments,
//! with both proto field names and lowerCamelCase aliases. This is configuration
//! compatibility, not an xDS client, HTTP proxy, or Envoy extension ABI.
//! See `docs/specs/0004-envoy-policy-api.md` for the supported contract.

mod config;
mod limits;
mod route;

pub use config::ConfigError;
pub use limits::{ClusterLimits, Priority, Resource};
pub use route::{MirrorTarget, RequestPlan, RetryOutcome, RoutePolicy};
