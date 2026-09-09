use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Invalid or unsupported Envoy configuration. Nothing is installed on error.
#[derive(Debug)]
pub struct ConfigError(pub(crate) String);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ConfigError {}
impl From<serde_json::Error> for ConfigError {
    fn from(value: serde_json::Error) -> Self {
        Self(value.to_string())
    }
}

pub(super) fn invalid(message: impl Into<String>) -> ConfigError {
    ConfigError(message.into())
}

/// Non-negative protobuf Duration JSON; reject lossy units and excess precision.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub(super) struct ProtoDuration(pub String);

impl ProtoDuration {
    pub fn duration(&self) -> Result<Duration, ConfigError> {
        let value = self
            .0
            .strip_suffix('s')
            .ok_or_else(|| invalid("duration must end in s"))?;
        let (seconds, fraction) = value.split_once('.').unwrap_or((value, ""));
        if seconds.is_empty()
            || !seconds.bytes().all(|c| c.is_ascii_digit())
            || !fraction.bytes().all(|c| c.is_ascii_digit())
            || fraction.len() > 9
            || (value.contains('.') && fraction.is_empty())
        {
            return Err(invalid(
                "duration must be non-negative seconds with at most 9 decimal places",
            ));
        }
        let seconds: u64 = seconds
            .parse()
            .map_err(|_| invalid("duration seconds overflow"))?;
        if seconds > 315_576_000_000 {
            return Err(invalid("duration exceeds protobuf range"));
        }
        let nanos = if fraction.is_empty() {
            0
        } else {
            fraction
                .parse::<u32>()
                .map_err(|_| invalid("invalid duration"))?
                * 10_u32.pow(9 - fraction.len() as u32)
        };
        Ok(Duration::new(seconds, nanos))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RetryBackOff {
    #[serde(alias = "baseInterval")]
    pub base_interval: ProtoDuration,
    #[serde(
        default,
        alias = "maxInterval",
        skip_serializing_if = "Option::is_none"
    )]
    pub max_interval: Option<ProtoDuration>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RetryConfig {
    #[serde(default, alias = "retryOn")]
    pub retry_on: String,
    #[serde(default, alias = "numRetries", skip_serializing_if = "Option::is_none")]
    pub num_retries: Option<u32>,
    #[serde(
        default,
        alias = "perTryTimeout",
        skip_serializing_if = "Option::is_none"
    )]
    pub per_try_timeout: Option<ProtoDuration>,
    #[serde(
        default,
        alias = "retryBackOff",
        skip_serializing_if = "Option::is_none"
    )]
    pub retry_back_off: Option<RetryBackOff>,
    #[serde(
        default,
        alias = "retriableStatusCodes",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub retriable_status_codes: Vec<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ClusterWeight {
    pub name: String,
    pub weight: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WeightedClusters {
    pub clusters: Vec<ClusterWeight>,
    #[serde(
        default,
        alias = "totalWeight",
        skip_serializing_if = "Option::is_none"
    )]
    pub total_weight: Option<u32>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub(super) enum Denominator {
    #[default]
    #[serde(rename = "HUNDRED")]
    Hundred,
    #[serde(rename = "TEN_THOUSAND")]
    TenThousand,
    #[serde(rename = "MILLION")]
    Million,
}
impl Denominator {
    pub fn value(self) -> u32 {
        match self {
            Self::Hundred => 100,
            Self::TenThousand => 10_000,
            Self::Million => 1_000_000,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Fraction {
    #[serde(default)]
    pub numerator: u32,
    #[serde(default)]
    pub denominator: Denominator,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RuntimeFraction {
    #[serde(default, alias = "defaultValue")]
    pub default_value: Fraction,
    // runtime_key deliberately unsupported: there is no runtime-key provider.
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MirrorConfig {
    pub cluster: String,
    #[serde(
        default,
        alias = "runtimeFraction",
        skip_serializing_if = "Option::is_none"
    )]
    pub runtime_fraction: Option<RuntimeFraction>,
    #[serde(default, alias = "disableShadowHostSuffixAppend")]
    pub disable_shadow_host_suffix_append: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RouteConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster: Option<String>,
    #[serde(
        default,
        alias = "weightedClusters",
        skip_serializing_if = "Option::is_none"
    )]
    pub weighted_clusters: Option<WeightedClusters>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<ProtoDuration>,
    #[serde(
        default,
        alias = "retryPolicy",
        skip_serializing_if = "Option::is_none"
    )]
    pub retry_policy: Option<RetryConfig>,
    #[serde(
        default,
        alias = "requestMirrorPolicies",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub request_mirror_policies: Vec<MirrorConfig>,
}
