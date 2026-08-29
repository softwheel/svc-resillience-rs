# Spec 0001: Resilience Kernel

Status: **Verified**

Verified: 2026-08-29

## Problem

Distributed clients fail in coupled ways: a transient downstream fault triggers retries; retries
increase load; increased load pushes latency up; queued work consumes resources; synchronized
clients retry together; and a recovering dependency can be overwhelmed by probes. Treating retry,
circuit breaking, bulkheads, routing, and shadowing as unrelated utilities makes these feedback
loops harder to reason about.

The crate therefore needs one small policy kernel with explicit budgets and composable failure
semantics.

## Goals

1. Bounded retries with exponential backoff and jitter.
2. A thread-safe circuit breaker with Closed, Open, and HalfOpen states.
3. A non-blocking concurrency bulkhead.
4. Runtime-agnostic core APIs and optional runtime/service adapters.
5. Deterministic behavioral contracts that can be property-tested and model-tested.
6. A foundation for dynamic routing and traffic shadowing without corrupting primary accounting.

## Non-goals for this spec

- Rate limiting.
- Distributed/global breaker state shared across processes.
- Persistent policy state.
- Service discovery.
- HTTP/gRPC-specific error classification.

## Retry invariants

- `max_attempts` includes the first physical attempt and is non-zero.
- A retry happens only when the caller classifies the error as retryable.
- Delay is capped by the configured maximum.
- Full jitter samples within `[0, cap]`; equal jitter samples within `[cap/2, cap]`.
- When `max_elapsed` is configured, a retry whose sleep would exceed the budget is rejected.
- Retry policy does not infer idempotency. The caller owns that decision.

## Circuit-breaker invariants

- Closed calls are admitted.
- `failure_threshold` consecutive failures transition Closed -> Open.
- A success in Closed resets the consecutive-failure counter.
- Open calls are rejected until `open_timeout` expires.
- Expiry transitions Open -> HalfOpen.
- HalfOpen admits at most `half_open_success_threshold` simultaneous probes.
- Any failed HalfOpen probe transitions immediately back to Open.
- Enough successful HalfOpen probes transition to Closed.
- Every state transition increments a generation. Results from older generations are ignored.
- Dropping an unfinished HalfOpen permit releases its probe slot without counting success/failure.

## Bulkhead invariants

- At most `capacity` permits are held at once.
- Admission is non-blocking.
- Excess calls are rejected immediately.
- Dropping or explicitly releasing a permit returns one slot exactly once.
- The implementation must not use `unsafe`.

## Composition semantics

Recommended order for a client request:

1. Establish a logical request deadline/budget.
2. Select a route from an immutable routing snapshot.
3. Acquire a bulkhead permit for the selected route/pool.
4. Execute a bounded retry loop.
5. Acquire a circuit-breaker permit for each physical downstream attempt.
6. Apply an attempt timeout and execute transport I/O.
7. Classify the result exactly once for breaker/retry purposes.
8. Return the primary result.
9. Optionally dispatch shadow traffic under a separate budget and isolation domain.

A circuit-breaker rejection is not a transport failure and should normally terminate retries for
that route. A future routing policy may explicitly choose another healthy route, but that is route
failover, not a blind retry.

## Verification plan

- Unit tests for every transition and budget boundary.
- Concurrency tests for HalfOpen probe limits and stale-result rejection.
- Property tests for backoff bounds and bulkhead capacity.
- Loom/model tests for permit/state races where practical.
- Tokio tests under the optional runtime feature.
- `cargo fmt --check`, `cargo clippy --all-targets --all-features -D warnings`, tests, and docs in CI.
- Benchmarks before optimizing synchronization; correctness is the first release gate.

## Verification evidence

- Stable CI runs formatting, Clippy with warnings denied, all-feature tests, and rustdoc against the
  newest dependency versions allowed by the manifest.
- Rust 1.75 CI resolves the reviewed dependency baseline and runs all-feature tests, making the
  declared MSRV reproducible.
- Retry/backoff tests cover attempt budgets, elapsed-time boundaries, deterministic exponential
  caps, and full/equal jitter bounds.
- Circuit-breaker tests cover Closed/Open/HalfOpen transitions, concurrent HalfOpen admission,
  stale-generation results, dropped permits, and poisoned-mutex recovery.
- The lock-free bulkhead has both high-contention stress coverage and an exhaustive Loom model over
  the same CAS reservation/release core used by production.
- Tokio integration tests cover successful async retries and immediate stop for non-retryable
  failures.
- Dependency/MSRV review is recorded in
  `docs/verification/0001-dependency-msrv-review.md`.

## Exit criteria for Verified

- CI green on stable Rust and MSRV.
- All listed invariants covered by tests.
- No panics on poisoned mutex recovery paths.
- Public API docs contain runnable examples.
- Miri/loom or equivalent concurrency validation added for the state machine.
- Routing/shadowing behavior is covered by a separate spec before implementation.

All exit criteria above are satisfied for the M0 kernel. Routing and shadowing remain explicitly
out of scope until Spec 0002 is written and accepted.
