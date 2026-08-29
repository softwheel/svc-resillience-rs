# Roadmap

## M0 — resilience kernel

- [x] Runtime-agnostic retry policy.
- [x] Exponential backoff with full/equal jitter.
- [x] Closed/Open/HalfOpen circuit breaker.
- [x] Generation-based stale-result protection.
- [x] Non-blocking bulkhead.
- [x] Tokio retry convenience behind a feature.
- [x] CI verification on latest stable Rust.
- [x] Property/concurrency testing sufficient to mark Spec 0001 Verified.

## M1 — dynamic routing and shadow traffic

Spec: `docs/specs/0002-routing-shadowing.md`

- [x] Define snapshot, weighted-selection, failover, shadow-isolation, cancellation, and verification semantics before implementation.
- [x] M1.1 immutable route model, validation, generation metadata, and deterministic weighted primary selection. Merged in #13.
- [x] M1.2 whole-snapshot concurrent publication with no mixed-generation reads. Merged in #14.
- [x] M1.3 deterministic bounded shadow sampling and primary + shadow route planning. Merged in #15.
- [x] M1.4 execution-isolation verification for separate breaker/bulkhead accounting and non-blocking primary completion. Merged in #16.
- [x] M1.5 explicit bounded route failover distinct from physical-attempt retry. Implemented in #17.
- [ ] M1.6 close remaining Spec 0002 gaps: health/policy filtering, explicit shadow retry/deadline policy, cancellation/overload observability, and public API examples.
- [ ] Verify Spec 0002 and close M1.

## M2 — production integration

- Tower `Layer`/`Service` adapters behind an optional feature.
- Tokio timeout/sleep integration without requiring Tokio in the core.
- Metrics/event hooks with zero mandatory telemetry dependencies.
- OpenTelemetry/tracing adapter crates or features.
- Retry budget/token-bucket protection across logical requests.
- Per-route policy registry.

## M3 — hardening and release

- Additional model/fuzz testing for routing and state-machine races.
- Property tests for routing weights and configuration spaces.
- Fault-injection/chaos integration tests.
- Criterion benchmarks for hot paths.
- Semver/API review.
- Security and dependency audit.
- crates.io release automation and changelog discipline.
