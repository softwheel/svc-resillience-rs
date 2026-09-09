# Spec 0004 — Envoy v3 policy API

Status: Implemented; verification evidence in the implementing PR. This is a configuration/policy milestone, not complete Envoy runtime compatibility.

## Problem and decision

Adopting the crate currently requires assembling many constructors, explicit random hooks,
route generations, eligibility snapshots and retry/shadow budgets. Expose Envoy v3 policy
vocabulary as the recommended entry point for Envoy users. Keep verified primitives as
implementation building blocks and advanced APIs; avoid a breaking rewrite during migration.

The contract is a strict, documented subset of Envoy v3 RouteAction and CircuitBreakers.
Two validated, immutable handles (`RoutePolicy`, `ClusterLimits`) hide deserialization types.
Serde is optional behind `envoy`. The core retains its runtime/transport independence.

## Compatibility matrix

| Concern | Envoy field | Local behavior / boundary |
| --- | --- | --- |
| Retry attempts | `retry_policy.num_retries` | Additional attempts; default 1 when policy exists, explicit 0 disables. No policy or no retry conditions means no retries. |
| Retry classification | `retry_policy.retry_on` | Supports `5xx`, `gateway-error`, `connect-failure`, `reset`, `refused-stream`, `retriable-status-codes`. Transport supplies a typed outcome; no arbitrary error guessing. |
| Backoff | `retry_policy.retry_back_off` | Fully jittered milliseconds in `[0, min((2^N-1)B, M))`; default B=25ms, M=250ms; custom M defaults to 10B. Positive values below 1ms round up to 1ms. |
| Timeouts | `timeout`, `retry_policy.per_try_timeout` | Exposes global and effective per-try durations. Global default 15s; 0 disables. Per-try 0 or >= enabled global timeout is ignored. These getters do not run timers. |
| Routing | `cluster`, `weighted_clusters.clusters` | Exactly one selector, non-empty names, unique cluster names, positive uint32 total; zero-weight entries excluded. Optional positive `total_weight` must match the sum. |
| Mirroring | `request_mirror_policies` | Named targets sampled independently of primary weights, including targets outside the primary list. Omitted fraction means 100%; HUNDRED/TEN_THOUSAND/MILLION supported. Host suffix intent exposed. Planning only. |
| Overload admission | `circuit_breakers.thresholds` | Independent DEFAULT/HIGH resource pools. Defaults: 1024 connections/pending/requests, 3 concurrent retries. Explicit zero rejects every admission. |
| Failure breaker | kernel `CircuitBreaker` | Existing failure state machine retained. It is neither Envoy resource circuit breaking nor endpoint outlier detection. |
| Serialization | JSON fragments | Accept snake_case and lowerCamelCase field names; export snake_case accepted by Envoy. Preserve option presence and explicit zeros. Strict scalar subset: numeric uint32 and named enums; quoted numbers and numeric enums unsupported. JSON fragments can also be embedded in Envoy YAML. |
| Wire protocol | xDS / protobuf / extension ABI | Not implemented. No ADS, discovery server, ACK/NACK, Proxy-Wasm, ext_proc or Envoy native filter claims. |

Supported fields are explicitly modeled, with unknown fields rejected recursively. For example,
`retry_budget`, `track_remaining`, `max_connection_pools`, runtime keys, hedging, custom retry
plugins, gRPC retry conditions and host/header selectors return `ConfigError`. Do not drop
unsupported fields from an existing deployment just to make it parse: keep that policy in
Envoy until an equivalent implementation is available. Full Bootstrap/Cluster/RouteConfiguration
objects are not fragment inputs.

## Runtime ownership and invariants

1. Call `plan()` once for each logical request and retain the result throughout retries. Calling
   it again intentionally creates a new random selection and new mirror samples.
2. Resolve the primary cluster before dispatch; a missing primary must suppress shadow dispatch.
   Planning does not resolve or health-check cluster endpoints. Every configured cluster must
   be mapped to its transport and its own shared `ClusterLimits` handle.
3. Acquire each resource for its actual lifetime. Resource counters are independent: an active
   retry needs retry admission as well as any active request/connection resources it consumes.
   A retry permit covers a pending backoff as well as the outstanding retry when matching Envoy's
   retry accounting. Clones share counters. Re-parsing creates new counters and must not be used
   per request or as an unsafe live reconfiguration strategy.
4. Drop releases permits, including cancellation/unwinding. Reuse the proven Bulkhead RAII
   implementation; explicit zero maps to no admissible permit, never unlimited capacity.
5. Check request replay safety, the global remaining deadline, and concurrent retry admission
   before scheduling the returned delay. `retry_delay` only makes the policy decision; it does
   not execute, sleep or enforce timers. A global timeout or cancellation must be classified
   as `Other`, never a retryable per-try timeout. Local overload and responses carrying
   `x-envoy-ratelimited` also map to `Other`; the transport must inspect this suppression header.
6. Mirror planning cannot mutate primary admission or results. A transport must use separate
   shadow resources, deadlines and outcome accounting, preserve Host/:authority suffix intent,
   and return the primary result without waiting for mirrors. No automatic shadow retries.
7. JSON load failure publishes nothing. Callers can prepare an immutable new policy before
   atomically publishing it. Atomic policy/resource reconfiguration is a later transport task.

## Migration

| Previous public setup | Envoy-facing setup |
| --- | --- |
| `ExponentialBackoff` + `NonZeroU32` + `RetryPolicy` | `RoutePolicy::from_json` and `retry_delay`; count is retries, not total attempts |
| `Route` + `RouteTable` + `RoutePlanner` | `RoutePolicy::from_json` and `plan()` |
| Random alternate-route shadow selection | Explicit `request_mirror_policies[].cluster` and fraction |
| Manual bulkhead capacity setup | `ClusterLimits::from_json` and `try_acquire(priority, resource)` |
| Closed/Open/HalfOpen `CircuitBreaker` | Retain advanced API; no automatic conversion to Envoy outlier detection |

New API users start with `examples/envoy_policy.rs`. Existing APIs are not removed or marked
obsolete because their policies differ intentionally. README moves constructor-heavy examples
to `docs/ADVANCED-USAGE.md`.

## Verification and completion criteria

- Rust tests cover retry count/zero/overflow boundaries, Envoy backoff caps, timeout defaults,
  retry classification, bad duration precision, unsupported nested fields, duplicate aliases,
  weighted routing, exact fractional boundaries, clone-shared limits, zero rejection, priority
  isolation, defaults and import/export.
- Existing concurrency/Loom tests continue to cover the reused admission algorithm. No new
  lock-free algorithm or concurrency optimization is introduced.
- `export_envoy` exports through the real public Rust API. `scripts/envoy_bootstrap.py` embeds
  those exports into a complete bootstrap. CI runs `envoy --mode validate` in Envoy 1.35.0.
- Schema validation proves acceptance of shared fixtures in that version. It does not prove
  live retry timing, request/body accounting, mirror dispatch or general Envoy parity.
- Required Rust gates remain formatting, Clippy/all targets/all features, all-feature tests,
  docs with warnings denied, benchmark compilation, Loom and dependency/license audit.

## Follow-up interoperability milestones

Build a transport executor around the simpler policies; add endpoint outlier detection with
proper ejection semantics; then add generated protobuf conversion and xDS subscriptions with
version/nonce, ACK/NACK, removal, reconnect and atomic-publication tests. Each capability gets
its own traffic-level evidence before expanding compatibility claims.

## Primary references

- [Envoy RouteAction / RetryPolicy / RequestMirrorPolicy](https://www.envoyproxy.io/docs/envoy/latest/api-v3/config/route/v3/route_components.proto)
- [Envoy retry behavior](https://www.envoyproxy.io/docs/envoy/latest/configuration/http/http_filters/router_filter)
- [Envoy circuit breaker configuration](https://www.envoyproxy.io/docs/envoy/latest/api-v3/config/cluster/v3/circuit_breaker.proto)
- [Envoy outlier detection](https://www.envoyproxy.io/docs/envoy/latest/intro/arch_overview/upstream/outlier)
- [xDS protocol](https://www.envoyproxy.io/docs/envoy/latest/api-docs/xds_protocol)
