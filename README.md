# softwheel-resilience

Composable resilience primitives for production Rust distributed systems.

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

## Implemented in v0 foundation

| Primitive | Status | Key behavior |
| --- | --- | --- |
| Retry | Implemented | bounded attempts, optional elapsed-time budget |
| Exponential backoff | Implemented | bounded exponential growth |
| Jitter | Implemented | full/equal/none; full is default |
| Circuit breaker | Implemented | Closed/Open/HalfOpen, bounded probes, stale-result protection |
| Bulkhead | Implemented | non-blocking RAII concurrency permits |
| Dynamic routing | Specified next | immutable routing snapshots + atomic replacement |
| Traffic shadowing | Specified next | isolated budget, never changes primary result |

## Retry example

```rust
use std::num::NonZeroU32;
use std::time::Duration;
use softwheel_resilience::{retry, ExponentialBackoff, Jitter, RetryDecision, RetryPolicy};

#[derive(Debug, PartialEq)]
struct CallError {
    transient: bool,
}

let backoff = ExponentialBackoff::new(
    Duration::from_millis(1),
    Duration::from_millis(4),
    2,
    Jitter::None,
).unwrap();
let policy = RetryPolicy::new(NonZeroU32::new(3).unwrap(), backoff);
let mut calls = 0;

let result = retry(
    &policy,
    || {
        calls += 1;
        if calls < 2 {
            Err(CallError { transient: true })
        } else {
            Ok("ok")
        }
    },
    |error| {
        if error.transient {
            RetryDecision::Retry
        } else {
            RetryDecision::DoNotRetry
        }
    },
);

assert_eq!(result, Ok("ok"));
assert_eq!(calls, 2);
```

The classifier is deliberately application-owned. A resilience library cannot safely assume that
an arbitrary error is retryable; retrying non-idempotent operations or permanent failures can
amplify an incident.

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

- `tokio`: enables `retry_async` using `tokio::time::sleep`.

## Project process

The contract and invariants live under `docs/specs/`. Changes to behavior should update the spec,
tests, and public docs in the same pull request. The roadmap is in `docs/ROADMAP.md`.
