# softwheel-resilience

Composable resilience primitives for production Rust distributed systems.

**Toolchain:** latest stable Rust (currently Rust 1.98.0), Edition 2024.

This crate grows out of the original **Rusty Circuit Breaker** implementation and the Softwheel
*Microservice Governance – Resilience Patterns* series. The original implementation modeled the
core Closed -> Open -> Half-Open state machine. This crate keeps that model, then builds a coherent
resilience layer around it.

## Design goals

- **Fail boundedly.** Retries always have an attempt/time budget; backoff is capped.
- **Avoid synchronized retry storms.** Full jitter is the default backoff strategy.
- **Make overload explicit.** Bulkheads reject instead of creating hidden, unbounded queues.
- **Keep failure accounting correct under concurrency.** Circuit-breaker generations prevent stale in-flight results from mutating a newer state.
- **Keep policy separate from transport.** The core does not depend on HTTP, gRPC, Tower, or a specific async runtime. Tokio convenience is opt-in.
- **Compose without amplification.** Shadow traffic must not consume primary retry budgets or trip the primary circuit breaker; routing and mirroring are built on top of the same policy kernel.
- **No unsafe code.** The crate forbids `unsafe`.

## Implemented foundation

| Primitive | Status | Key behavior |
| --- | --- | --- |
| Retry | Implemented | bounded attempts, optional elapsed-time budget |
| Exponential backoff | Implemented | bounded exponential growth |
| Jitter | Implemented | full/equal/none; full is default |
| Circuit breaker | Implemented | Closed/Open/HalfOpen, bounded probes, stale-result protection |
| Bulkhead | Implemented | non-blocking RAII concurrency permits |
| Dynamic routing | Implemented | immutable snapshots, atomic replacement, eligibility filtering, bounded failover |
| Traffic shadowing | Implemented | deterministic sampling, isolated budgets, observable non-propagating outcomes |

## Envoy v3 policy API (recommended for Envoy users)

Enable the `envoy` feature and reuse supported Envoy **RouteAction** and
**CircuitBreakers** JSON fragments. Two entry points replace manual policy construction:
`RoutePolicy` for routing, retry decisions, timeouts and mirror planning; `ClusterLimits`
for shared resource admission. `to_json()` exports the fragments back to Envoy.

```rust
# #[cfg(feature = "envoy")]
# fn example() -> Result<(), Box<dyn std::error::Error>> {
use softwheel_resilience::envoy::{ClusterLimits, Priority, Resource, RoutePolicy};

let route = RoutePolicy::from_json(r#"{
  "cluster": "orders",
  "timeout": "2s",
  "retry_policy": { "retry_on": "5xx", "num_retries": 2 }
}"#)?;
let limits = ClusterLimits::from_json(r#"{
  "thresholds": [{ "max_requests": 100, "max_retries": 10 }]
}"#)?;

let plan = route.plan();
let permit = limits.try_acquire(Priority::Default, Resource::Request)?;
assert_eq!(plan.cluster(), "orders");
// Transport dispatches to plan.cluster() while holding the permit.
drop(permit);
# Ok(())
# }
```

This release provides **configuration and policy compatibility**, not an HTTP proxy,
xDS client/server, or Proxy-Wasm plugin. Transport code still owns replay safety, deadline
enforcement, endpoint resolution and isolated mirror dispatch. Unsupported configuration
is rejected, never silently ignored. Envoy resource circuit breakers are distinct from the
kernel's failure-based Closed/Open/HalfOpen breaker; outlier detection is not implemented.

See the [compatibility contract and migration guide](docs/specs/0004-envoy-policy-api.md),
[shared Envoy configuration](examples/envoy/route.json), and runnable
[`envoy_policy` example](examples/envoy_policy.rs). CI validates Rust-exported configuration
using an actual Envoy 1.35.0 binary. This checks schema acceptance, not traffic-level parity.

The existing primitives remain available for custom integrations; their longer examples
are in [advanced composition](docs/ADVANCED-USAGE.md).

## Circuit-breaker semantics

The circuit starts **Closed**. Consecutive failures trip it **Open**. After the configured timeout,
it moves to **HalfOpen** and allows only a bounded number of probes. A failed probe immediately
re-opens the circuit; enough successful probes close it. Permits carry a generation number so a
late result from an older state cannot overwrite a newer decision.

## Composition rule

A recommended client-side pipeline is:

```text
logical request budget
  -> route selection
  -> bulkhead admission
  -> retry loop
       -> circuit-breaker permit (per physical attempt)
       -> per-attempt timeout
       -> transport
  -> primary result
  -> optional isolated shadow dispatch
```

Retries are *physical attempts*, not new logical requests. Shadow traffic gets a separate bulkhead,
breaker, and deadline budget. It must never delay or alter the primary response.

## Feature flags

- `envoy`: enables strict Envoy v3 JSON policy import/export and policy/admission helpers.
- `tokio`: enables `retry_async` using `tokio::time::sleep`.

## Project process

The contract and invariants live under `docs/specs/`. Changes to behavior should update the spec,
tests, and public docs in the same pull request. The roadmap is in `docs/ROADMAP.md`.
