use std::time::Duration;

use super::ConfigError;
use super::config::{RouteConfig, invalid};
use crate::{Route, RouteId, RouteTable, ShadowSampling};

/// Transport-owned classification; the policy never inspects arbitrary application errors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RetryOutcome {
    HttpStatus(u16),
    ConnectFailure,
    Reset,
    RefusedStream,
    PerTryTimeout,
    /// Includes successful results, permanent errors, cancellation, global timeout,
    /// local overload and responses carrying x-envoy-ratelimited.
    Other,
}

/// A named mirror target, independent of weighted primary cluster selection.
#[derive(Clone, Debug)]
pub struct MirrorTarget {
    cluster: String,
    sampling: ShadowSampling,
    append_shadow_host_suffix: bool,
}
impl MirrorTarget {
    pub fn cluster(&self) -> &str {
        &self.cluster
    }
    pub fn sampling(&self) -> ShadowSampling {
        self.sampling
    }
    /// The HTTP transport must apply this to Host/:authority when dispatching.
    pub fn append_shadow_host_suffix(&self) -> bool {
        self.append_shadow_host_suffix
    }
}

/// One logical request's primary cluster and independently sampled mirror targets.
///
/// This is a plan, not dispatched traffic. Resolve the primary cluster before dispatching
/// mirrors, and give mirrors separate resource/deadline accounting. Retain this plan for
/// retries: retries do not select another weighted cluster or sample mirrors again.
#[derive(Clone, Debug)]
pub struct RequestPlan {
    cluster: String,
    mirrors: Vec<MirrorTarget>,
}
impl RequestPlan {
    pub fn cluster(&self) -> &str {
        &self.cluster
    }
    pub fn mirrors(&self) -> &[MirrorTarget] {
        &self.mirrors
    }
}

/// Validated Envoy v3 RouteAction policy with simple planning and retry decisions.
///
/// Loading/exporting configuration does not implement xDS transport or HTTP execution.
/// The caller owns endpoint resolution, request replay safety, timers and mirror dispatch.
#[derive(Clone, Debug)]
pub struct RoutePolicy {
    config: RouteConfig,
    routes: RouteTable,
    timeout: Option<Duration>,
    per_try_timeout: Option<Duration>,
    base_ms: u64,
    max_ms: u64,
    mirrors: Vec<MirrorTarget>,
}
impl RoutePolicy {
    /// Parse a strict supported `envoy.config.route.v3.RouteAction` JSON fragment.
    pub fn from_json(json: &str) -> Result<Self, ConfigError> {
        let config: RouteConfig = serde_json::from_str(json)?;
        let routes = match (&config.cluster, &config.weighted_clusters) {
            (Some(name), None) => vec![Route::new(
                RouteId::new(name.clone()).map_err(|e| invalid(e.to_string()))?,
                1,
            )],
            (None, Some(weighted)) => {
                let total: u64 = weighted.clusters.iter().map(|c| u64::from(c.weight)).sum();
                if total == 0 || total > u64::from(u32::MAX) {
                    return Err(invalid("weighted clusters require a positive uint32 total"));
                }
                if weighted
                    .total_weight
                    .is_some_and(|t| t > 0 && u64::from(t) != total)
                {
                    return Err(invalid(
                        "total_weight must equal the sum of cluster weights",
                    ));
                }
                weighted
                    .clusters
                    .iter()
                    .map(|c| {
                        Ok(Route::new(
                            RouteId::new(c.name.clone()).map_err(|e| invalid(e.to_string()))?,
                            u64::from(c.weight),
                        ))
                    })
                    .collect::<Result<Vec<_>, ConfigError>>()?
            }
            _ => {
                return Err(invalid(
                    "exactly one of cluster and weighted_clusters is required",
                ));
            }
        };
        let routes = RouteTable::new(0, routes).map_err(|e| invalid(e.to_string()))?;
        let timeout = config
            .timeout
            .as_ref()
            .map(|t| t.duration())
            .transpose()?
            .unwrap_or(Duration::from_secs(15));
        let timeout = (!timeout.is_zero()).then_some(timeout);
        let mut base = Duration::from_millis(25);
        let mut maximum = Duration::from_millis(250);
        let mut per_try_timeout = None;
        if let Some(retry) = &config.retry_policy {
            for condition in retry.retry_on.split(',').filter(|s| !s.is_empty()) {
                if !matches!(
                    condition,
                    "5xx"
                        | "gateway-error"
                        | "connect-failure"
                        | "reset"
                        | "refused-stream"
                        | "retriable-status-codes"
                ) {
                    return Err(invalid(format!(
                        "unsupported retry_on condition: {condition}"
                    )));
                }
            }
            if retry.retry_on.starts_with(',')
                || retry.retry_on.ends_with(',')
                || retry.retry_on.contains(",,")
            {
                return Err(invalid("retry_on contains an empty condition"));
            }
            if retry
                .retriable_status_codes
                .iter()
                .any(|c| !(100..=599).contains(c))
            {
                return Err(invalid("retriable_status_codes must be HTTP status codes"));
            }
            if let Some(backoff) = &retry.retry_back_off {
                base = backoff.base_interval.duration()?;
                if base.is_zero() {
                    return Err(invalid("base_interval must be positive"));
                }
                maximum = backoff
                    .max_interval
                    .as_ref()
                    .map(|d| d.duration())
                    .transpose()?
                    .unwrap_or(base.saturating_mul(10));
                if maximum < base {
                    return Err(invalid("max_interval is below base_interval"));
                }
            }
            per_try_timeout = retry
                .per_try_timeout
                .as_ref()
                .map(|d| d.duration())
                .transpose()?;
            // Envoy disables a zero per-try timeout and ignores one >= route timeout.
            per_try_timeout =
                per_try_timeout.filter(|d| !d.is_zero() && timeout.is_none_or(|t| *d < t));
        }
        let mut mirrors = Vec::new();
        for mirror in &config.request_mirror_policies {
            if mirror.cluster.is_empty() {
                return Err(invalid("mirror cluster must not be empty"));
            }
            let ppm = match &mirror.runtime_fraction {
                None => 1_000_000,
                Some(fraction) => {
                    let fraction = &fraction.default_value;
                    let denominator = fraction.denominator.value();
                    if fraction.numerator > denominator {
                        return Err(invalid("mirror numerator exceeds denominator"));
                    }
                    fraction.numerator * (1_000_000 / denominator)
                }
            };
            mirrors.push(MirrorTarget {
                cluster: mirror.cluster.clone(),
                sampling: ShadowSampling::new(ppm).map_err(|e| invalid(e.to_string()))?,
                append_shadow_host_suffix: !mirror.disable_shadow_host_suffix_append,
            });
        }
        // Envoy timers use milliseconds. Sub-millisecond positive backoff rounds to 1ms.
        let base_ms = u64::try_from(base.as_millis().max(1))
            .map_err(|_| invalid("base_interval overflow"))?;
        let explicit_maximum = config
            .retry_policy
            .as_ref()
            .and_then(|r| r.retry_back_off.as_ref())
            .is_some_and(|b| b.max_interval.is_some());
        let max_ms = if explicit_maximum {
            u64::try_from(maximum.as_millis().max(1))
                .map_err(|_| invalid("max_interval overflow"))?
        } else {
            base_ms.saturating_mul(10)
        };
        Ok(Self {
            config,
            routes,
            timeout,
            per_try_timeout,
            base_ms,
            max_ms,
            mirrors,
        })
    }

    /// Export a fragment suitable for the `route` field of an Envoy HTTP route.
    pub fn to_json(&self) -> Result<String, ConfigError> {
        Ok(serde_json::to_string_pretty(&self.config)?)
    }

    /// Select once per logical request. This does not resolve clusters or send traffic.
    pub fn plan(&self) -> RequestPlan {
        RequestPlan {
            cluster: self.routes.select().id().as_str().to_owned(),
            mirrors: self
                .mirrors
                .iter()
                .filter(|m| m.sampling.sample())
                .cloned()
                .collect(),
        }
    }

    /// Global route timeout, including all attempts and backoff. None means disabled.
    pub fn timeout(&self) -> Option<Duration> {
        self.timeout
    }
    /// Effective per-try timeout; None means use the remaining route timeout.
    pub fn per_try_timeout(&self) -> Option<Duration> {
        self.per_try_timeout
    }
    /// Configured mirrors before sampling, for transport setup and cluster resolution.
    pub fn mirror_targets(&self) -> &[MirrorTarget] {
        &self.mirrors
    }
    /// Retries beyond the initial attempt; absent policy disables retries.
    pub fn num_retries(&self) -> u32 {
        self.config
            .retry_policy
            .as_ref()
            .map_or(0, |r| r.num_retries.unwrap_or(1))
    }

    /// Exclusive jitter cap for a one-based retry number. This differs from the
    /// original kernel backoff: min((2^N - 1) * base, maximum), in milliseconds.
    pub fn retry_backoff_cap(&self, retry_number: u32) -> Duration {
        let multiplier = 1_u64.checked_shl(retry_number).map_or(u64::MAX, |n| n - 1);
        Duration::from_millis(self.base_ms.saturating_mul(multiplier).min(self.max_ms))
    }

    /// Return a bounded, fully jittered delay for the next retry, or None to stop.
    /// `retry_number=1` is the first retry (second physical attempt). The caller must
    /// also enforce replay safety, remaining global deadline and cluster retry admission.
    pub fn retry_delay(&self, retry_number: u32, outcome: RetryOutcome) -> Option<Duration> {
        if retry_number == 0 || retry_number > self.num_retries() || !self.matches(outcome) {
            return None;
        }
        let cap_ms = self.retry_backoff_cap(retry_number).as_millis() as u64;
        Some(Duration::from_millis(fastrand::u64(0..cap_ms)))
    }

    fn matches(&self, outcome: RetryOutcome) -> bool {
        let Some(retry) = &self.config.retry_policy else {
            return false;
        };
        retry.retry_on.split(',').any(|condition| match condition {
            "5xx" => matches!(outcome, RetryOutcome::HttpStatus(500..=599) | RetryOutcome::ConnectFailure | RetryOutcome::Reset | RetryOutcome::RefusedStream | RetryOutcome::PerTryTimeout),
            "gateway-error" => matches!(outcome, RetryOutcome::HttpStatus(502..=504) | RetryOutcome::ConnectFailure | RetryOutcome::Reset | RetryOutcome::RefusedStream | RetryOutcome::PerTryTimeout),
            "connect-failure" => outcome == RetryOutcome::ConnectFailure,
            "reset" => matches!(outcome, RetryOutcome::ConnectFailure | RetryOutcome::Reset | RetryOutcome::RefusedStream | RetryOutcome::PerTryTimeout),
            "refused-stream" => outcome == RetryOutcome::RefusedStream,
            "retriable-status-codes" => matches!(outcome, RetryOutcome::HttpStatus(code) if retry.retriable_status_codes.contains(&u32::from(code))),
            _ => false,
        })
    }
}
