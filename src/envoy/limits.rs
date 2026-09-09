use std::num::NonZeroUsize;

use serde::{Deserialize, Serialize};

use super::ConfigError;
use super::config::invalid;
use crate::{Bulkhead, BulkheadPermit, BulkheadRejected};

/// Independent Envoy resource accounting priorities.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum Priority {
    #[default]
    #[serde(rename = "DEFAULT")]
    Default,
    #[serde(rename = "HIGH")]
    High,
}
impl Priority {
    fn index(self) -> usize {
        match self {
            Self::Default => 0,
            Self::High => 1,
        }
    }
}

/// Resource whose lifetime the returned RAII permit must cover.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum Resource {
    Connection,
    PendingRequest,
    Request,
    Retry,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Thresholds {
    #[serde(default)]
    priority: Priority,
    #[serde(
        default,
        alias = "maxConnections",
        skip_serializing_if = "Option::is_none"
    )]
    max_connections: Option<u32>,
    #[serde(
        default,
        alias = "maxPendingRequests",
        skip_serializing_if = "Option::is_none"
    )]
    max_pending_requests: Option<u32>,
    #[serde(
        default,
        alias = "maxRequests",
        skip_serializing_if = "Option::is_none"
    )]
    max_requests: Option<u32>,
    #[serde(default, alias = "maxRetries", skip_serializing_if = "Option::is_none")]
    max_retries: Option<u32>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(default)]
    thresholds: Vec<Thresholds>,
}

#[derive(Clone, Debug)]
struct Limits([Option<Bulkhead>; 4]);
impl Limits {
    fn new(config: &Thresholds) -> Result<Self, ConfigError> {
        let values = [
            config.max_connections.unwrap_or(1024),
            config.max_pending_requests.unwrap_or(1024),
            config.max_requests.unwrap_or(1024),
            config.max_retries.unwrap_or(3),
        ];
        let mut limits = [None, None, None, None];
        for (slot, value) in limits.iter_mut().zip(values) {
            let value = usize::try_from(value)
                .map_err(|_| invalid("resource limit exceeds platform usize"))?;
            *slot = NonZeroUsize::new(value).map(Bulkhead::new);
        }
        Ok(Self(limits))
    }
}

/// Envoy's concurrent resource limits, shared across clones of this cluster handle.
///
/// Create one handle per upstream cluster and reuse it across routes. Explicit zero
/// rejects all admissions for that resource. A retry must also acquire the resources
/// it actually uses (for example a Request permit); resource counters are independent.
/// This does not implement failure-based Closed/Open/HalfOpen state transitions.
#[derive(Clone, Debug)]
pub struct ClusterLimits {
    config: Config,
    limits: [Limits; 2],
}
impl ClusterLimits {
    /// Parse an `envoy.config.cluster.v3.CircuitBreakers` JSON fragment.
    pub fn from_json(json: &str) -> Result<Self, ConfigError> {
        let config: Config = serde_json::from_str(json)?;
        let defaults = Thresholds::default();
        let mut limits = [Limits::new(&defaults)?, Limits::new(&defaults)?];
        let mut seen = [false; 2];
        for thresholds in &config.thresholds {
            let index = thresholds.priority.index();
            if seen[index] {
                return Err(invalid("duplicate circuit breaker priority"));
            }
            seen[index] = true;
            limits[index] = Limits::new(thresholds)?;
        }
        Ok(Self { config, limits })
    }

    /// Export a reusable Envoy fragment, retaining explicit zero and omitted fields.
    pub fn to_json(&self) -> Result<String, ConfigError> {
        Ok(serde_json::to_string_pretty(&self.config)?)
    }

    /// Acquire without queueing; dropping the permit releases exactly one resource.
    pub fn try_acquire(
        &self,
        priority: Priority,
        resource: Resource,
    ) -> Result<BulkheadPermit, BulkheadRejected> {
        let index = match resource {
            Resource::Connection => 0,
            Resource::PendingRequest => 1,
            Resource::Request => 2,
            Resource::Retry => 3,
        };
        self.limits[priority.index()].0[index]
            .as_ref()
            .ok_or(BulkheadRejected)?
            .try_acquire()
    }
}
