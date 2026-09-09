#![cfg(feature = "envoy")]

use softwheel_resilience::envoy::{ClusterLimits, Priority, Resource, RetryOutcome, RoutePolicy};
use std::time::Duration;

fn route(extra: &str) -> RoutePolicy {
    RoutePolicy::from_json(&format!(r#"{{"cluster":"orders",{extra}}}"#)).unwrap()
}

#[test]
fn retries_count_additional_attempts_and_zero_disables() {
    let p = route(r#""retry_policy":{"retry_on":"5xx","num_retries":2}"#);
    assert!(p.retry_delay(0, RetryOutcome::HttpStatus(503)).is_none());
    assert!(p.retry_delay(1, RetryOutcome::HttpStatus(503)).is_some());
    assert!(p.retry_delay(2, RetryOutcome::HttpStatus(503)).is_some());
    assert!(p.retry_delay(3, RetryOutcome::HttpStatus(503)).is_none());
    let disabled = route(r#""retry_policy":{"retry_on":"5xx","num_retries":0}"#);
    assert!(
        disabled
            .retry_delay(1, RetryOutcome::HttpStatus(503))
            .is_none()
    );
    let default = route(r#""retry_policy":{"retry_on":"5xx"}"#);
    assert_eq!(default.num_retries(), 1);
    let empty = route(r#""retry_policy":{}"#);
    assert!(
        empty
            .retry_delay(1, RetryOutcome::HttpStatus(503))
            .is_none()
    );
}

#[test]
fn envoy_backoff_uses_exclusive_25_75_175_250ms_caps() {
    let p = route(r#""retry_policy":{"retry_on":"5xx","num_retries":4294967295}"#);
    for (n, expected) in [
        (1, 25),
        (2, 75),
        (3, 175),
        (4, 250),
        (32, 250),
        (64, 250),
        (u32::MAX, 250),
    ] {
        assert_eq!(p.retry_backoff_cap(n), Duration::from_millis(expected));
        for _ in 0..100 {
            assert!(
                p.retry_delay(n, RetryOutcome::HttpStatus(503)).unwrap()
                    < Duration::from_millis(expected)
            );
        }
    }
    let p = route(
        r#""retry_policy":{"retry_on":"5xx","retry_back_off":{"base_interval":"0.000000001s"}}"#,
    );
    assert_eq!(p.retry_backoff_cap(1), Duration::from_millis(1));
    assert_eq!(p.retry_backoff_cap(4), Duration::from_millis(10));
}

#[test]
fn timeout_defaults_disable_and_per_try_rules() {
    let defaults = RoutePolicy::from_json(r#"{"cluster":"orders"}"#).unwrap();
    assert_eq!(defaults.timeout(), Some(Duration::from_secs(15)));
    assert_eq!(defaults.num_retries(), 0);
    let disabled = route(r#""timeout":"0s","retry_policy":{"per_try_timeout":"1s"}"#);
    assert_eq!(disabled.timeout(), None);
    assert_eq!(disabled.per_try_timeout(), Some(Duration::from_secs(1)));
    for child in ["0s", "2s", "3s"] {
        let p = route(&format!(
            r#""timeout":"2s","retry_policy":{{"per_try_timeout":"{child}"}}"#
        ));
        assert_eq!(p.per_try_timeout(), None);
    }
    assert_eq!(
        route(r#""timeout":"1.000000001s""#).timeout(),
        Some(Duration::new(1, 1))
    );
}

#[test]
fn classifies_only_enabled_retry_conditions() {
    let gateway = route(r#""retry_policy":{"retry_on":"gateway-error"}"#);
    assert!(
        gateway
            .retry_delay(1, RetryOutcome::HttpStatus(500))
            .is_none()
    );
    assert!(
        gateway
            .retry_delay(1, RetryOutcome::HttpStatus(503))
            .is_some()
    );
    assert!(
        gateway
            .retry_delay(1, RetryOutcome::PerTryTimeout)
            .is_some()
    );
    assert!(gateway.retry_delay(1, RetryOutcome::Other).is_none());
    let status = route(
        r#""retry_policy":{"retry_on":"retriable-status-codes","retriable_status_codes":[429]}"#,
    );
    assert!(
        status
            .retry_delay(1, RetryOutcome::HttpStatus(429))
            .is_some()
    );
    assert!(
        status
            .retry_delay(1, RetryOutcome::HttpStatus(503))
            .is_none()
    );
    let reset = route(r#""retry_policy":{"retry_on":"reset"}"#);
    for outcome in [
        RetryOutcome::Reset,
        RetryOutcome::ConnectFailure,
        RetryOutcome::RefusedStream,
        RetryOutcome::PerTryTimeout,
    ] {
        assert!(reset.retry_delay(1, outcome).is_some());
    }
    assert!(
        reset
            .retry_delay(1, RetryOutcome::HttpStatus(503))
            .is_none()
    );
    let connect = route(r#""retry_policy":{"retry_on":"connect-failure"}"#);
    assert!(
        connect
            .retry_delay(1, RetryOutcome::ConnectFailure)
            .is_some()
    );
    assert!(connect.retry_delay(1, RetryOutcome::Reset).is_none());
}

#[test]
fn invalid_or_unsupported_config_is_rejected() {
    for json in [
        r#"{}"#,
        r#"{"cluster":""}"#,
        r#"{"cluster":"a","weighted_clusters":{"clusters":[{"name":"a","weight":1}]}}"#,
        r#"{"cluster":"a","timeout":"25ms"}"#,
        r#"{"cluster":"a","timeout":"-1s"}"#,
        r#"{"cluster":"a","timeout":"1.0000000001s"}"#,
        r#"{"cluster":"a","timeout":"315576000001s"}"#,
        r#"{"cluster":"a","retry_policy":{"num_retries":-1}}"#,
        r#"{"cluster":"a","retry_policy":{"retry_on":"unavailable"}}"#,
        r#"{"cluster":"a","retry_policy":{"retry_on":"5xx,"}}"#,
        r#"{"cluster":"a","retry_policy":{"retry_back_off":{"base_interval":"0s"}}}"#,
        r#"{"cluster":"a","retry_policy":{"retry_back_off":{"base_interval":"1s","max_interval":"0.5s"}}}"#,
        r#"{"cluster":"a","retry_policy":{"retry_on":"5xx","retryOn":"reset"}}"#,
        r#"{"cluster":"a","request_mirror_policies":[{"cluster":"b","runtime_fraction":{"runtime_key":"mirror"}}]}"#,
        r#"{"cluster":"a","request_mirror_policies":[{"cluster":"b","runtime_fraction":{"default_value":{"numerator":101}}}]}"#,
        r#"{"cluster":"a","hedge_policy":{}}"#,
        r#"{"weighted_clusters":{"clusters":[{"name":"a","weight":0}]}}"#,
        r#"{"weighted_clusters":{"clusters":[{"name":"a","weight":1},{"name":"a","weight":1}]}}"#,
        r#"{"weighted_clusters":{"clusters":[{"name":"a","weight":1}],"total_weight":100}}"#,
    ] {
        assert!(RoutePolicy::from_json(json).is_err(), "accepted {json}");
    }
    for json in [
        r#"{"thresholds":[{"max_requests":-1}]}"#,
        r#"{"thresholds":[{"priority":"DEFAULT"},{"priority":"DEFAULT"}]}"#,
        r#"{"thresholds":[{"retry_budget":{}}]}"#,
        r#"{"thresholds":[{"track_remaining":true}]}"#,
    ] {
        assert!(ClusterLimits::from_json(json).is_err(), "accepted {json}");
    }
}

#[test]
fn weighted_routing_excludes_zero_and_uses_named_mirrors() {
    let p = RoutePolicy::from_json(r#"{
        "weighted_clusters":{"clusters":[{"name":"off","weight":0},{"name":"live","weight":100}]},
        "request_mirror_policies":[{"cluster":"mirror"},{"cluster":"disabled","runtime_fraction":{"default_value":{"numerator":0}}}]
    }"#).unwrap();
    for _ in 0..100 {
        let plan = p.plan();
        assert_eq!(plan.cluster(), "live");
        assert_eq!(plan.mirrors().len(), 1);
        assert_eq!(plan.mirrors()[0].cluster(), "mirror");
        assert!(plan.mirrors()[0].append_shadow_host_suffix());
    }
}

#[test]
fn all_fraction_denominators_have_exact_equivalent_sampling() {
    for (denominator, numerator) in [("HUNDRED", 5), ("TEN_THOUSAND", 500), ("MILLION", 50_000)] {
        let p = route(&format!(
            r#""request_mirror_policies":[{{"cluster":"m","runtime_fraction":{{"default_value":{{"numerator":{numerator},"denominator":"{denominator}"}}}}}}]"#
        ));
        assert_eq!(p.mirror_targets()[0].sampling().parts_per_million(), 50_000);
        assert!(
            p.mirror_targets()[0]
                .sampling()
                .sample_with(|_| 49_999)
                .unwrap()
        );
        assert!(
            !p.mirror_targets()[0]
                .sampling()
                .sample_with(|_| 50_000)
                .unwrap()
        );
        let roundtrip = RoutePolicy::from_json(&p.to_json().unwrap()).unwrap();
        assert_eq!(roundtrip.timeout(), p.timeout());
    }
}

#[test]
fn cluster_limits_share_clones_release_on_drop_and_separate_priorities() {
    let limits = ClusterLimits::from_json(
        r#"{"thresholds":[{"max_requests":1,"max_retries":0,"max_pending_requests":0}]}"#,
    )
    .unwrap();
    let copy = limits.clone();
    let permit = limits
        .try_acquire(Priority::Default, Resource::Request)
        .unwrap();
    assert!(
        copy.try_acquire(Priority::Default, Resource::Request)
            .is_err()
    );
    assert!(copy.try_acquire(Priority::High, Resource::Request).is_ok());
    assert!(
        copy.try_acquire(Priority::Default, Resource::Retry)
            .is_err()
    );
    assert!(
        copy.try_acquire(Priority::Default, Resource::PendingRequest)
            .is_err()
    );
    drop(permit);
    assert!(
        copy.try_acquire(Priority::Default, Resource::Request)
            .is_ok()
    );
}

#[test]
fn absent_cluster_limits_apply_envoy_defaults() {
    let limits = ClusterLimits::from_json("{}").unwrap();
    let mut permits = Vec::new();
    for _ in 0..3 {
        permits.push(
            limits
                .try_acquire(Priority::Default, Resource::Retry)
                .unwrap(),
        );
    }
    assert!(
        limits
            .try_acquire(Priority::Default, Resource::Retry)
            .is_err()
    );
    for _ in 0..1024 {
        permits.push(
            limits
                .try_acquire(Priority::Default, Resource::Request)
                .unwrap(),
        );
    }
    assert!(
        limits
            .try_acquire(Priority::Default, Resource::Request)
            .is_err()
    );
}

#[test]
fn accepts_proto_and_json_names_and_exports_equivalent_fragments() {
    let p = route(r#""retryPolicy":{"retryOn":"5xx","numRetries":0,"perTryTimeout":"0.25s"}"#);
    let exported = p.to_json().unwrap();
    assert!(exported.contains("num_retries"));
    let reloaded = RoutePolicy::from_json(&exported).unwrap();
    assert_eq!(reloaded.num_retries(), 0);
    assert_eq!(reloaded.per_try_timeout(), Some(Duration::from_millis(250)));
    let limits = ClusterLimits::from_json(r#"{"thresholds":[{"maxRequests":0}]}"#).unwrap();
    let reloaded = ClusterLimits::from_json(&limits.to_json().unwrap()).unwrap();
    assert!(
        reloaded
            .try_acquire(Priority::Default, Resource::Request)
            .is_err()
    );
}

#[test]
fn shared_envoy_fixtures_load_and_export() {
    let route = RoutePolicy::from_json(include_str!("../examples/envoy/route.json")).unwrap();
    let limits =
        ClusterLimits::from_json(include_str!("../examples/envoy/circuit-breakers.json")).unwrap();
    assert_eq!(route.num_retries(), 2);
    assert!(RoutePolicy::from_json(&route.to_json().unwrap()).is_ok());
    assert!(ClusterLimits::from_json(&limits.to_json().unwrap()).is_ok());
}
