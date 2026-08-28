# Roadmap

## M0 — resilience kernel

- [x] Runtime-agnostic retry policy.
- [x] Exponential backoff with full/equal jitter.
- [x] Closed/Open/HalfOpen circuit breaker.
- [x] Generation-based stale-result protection.
- [x] Non-blocking bulkhead.
- [x] Tokio retry convenience behind a feature.
- [ ] CI verification on stable + MSRV.
- [ ] Property/concurrency testing sufficient to mark Spec 0001 Verified.

## M1 — dynamic routing and shadow traffic

- Immutable route table snapshots with atomic replacement.
- Weighted routing with deterministic test hooks.
- Health/policy filtering before weighted selection.
- Primary + shadow route plan.
- Separate shadow deadline, breaker, bulkhead, and retry budget.
- Shadow cancellation/drop policy and observability hooks.
- Explicit rule: shadow failure can never change the primary result.

## M2 — production integration

- Tower `Layer`/`Service` adapters behind an optional feature.
- Tokio timeout/sleep integration without requiring Tokio in the core.
- Metrics/event hooks with zero mandatory telemetry dependencies.
- OpenTelemetry/tracing adapter crates or features.
- Retry budget/token-bucket protection across logical requests.
- Per-route policy registry.

## M3 — hardening and release

- Loom/model tests for breaker and bulkhead races.
- Property tests for jitter/backoff bounds and routing weights.
- Fault-injection/chaos integration tests.
- Criterion benchmarks for hot paths.
- Semver/API review.
- Security and dependency audit.
- crates.io release automation and changelog discipline.
