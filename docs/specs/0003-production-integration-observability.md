# Spec 0003: Production Integration, Observability, and Hardening

Status: **Verified**

Verification record: [`docs/verification/0003-m2-final-verification.md`](../verification/0003-m2-final-verification.md)

## Problem

Specs 0001 and 0002 define and verify the runtime-agnostic resilience kernel, dynamic routing, bounded failover, and strictly isolated traffic shadowing. Production users now need adapters that compose those semantics with async runtimes and service stacks without weakening the verified invariants.

The main risk in M2 is accidental semantic coupling: an adapter can hide retries, extend a logical deadline, consume the wrong bulkhead, let shadow failures influence the primary result, or emit telemetry whose cardinality becomes an operational incident. M2 therefore treats adapters and observability as translations of already-explicit core decisions, not as new policy engines.

## Goals

1. Keep the core runtime-agnostic and dependency-light.
2. Add optional production adapters without changing verified core semantics.
3. Propagate one explicit logical-request deadline through retry and route failover.
4. Add a cross-request retry budget to bound retry amplification during incidents.
5. Provide a per-route resource registry for breakers, bulkheads, and adapter state.
6. Define zero-dependency core observability hooks with bounded-cardinality metadata.
7. Add optional ecosystem adapters such as `tracing` and Tower behind features.
8. Strengthen concurrency, property, fuzz, and benchmark verification before synchronization optimizations.
9. Establish a release-quality semver, dependency, license, changelog, and publishing gate.

## Non-goals

- Replacing Tower, Tokio, Hyper, tonic, or service-specific transports.
- Introducing a mandatory async runtime.
- Hiding physical attempts or route failovers inside opaque adapter behavior.
- Distributed retry budgets shared across processes.
- Built-in service discovery, health probing, or configuration distribution.
- High-cardinality telemetry labels such as request IDs, arbitrary URLs, or user-controlled strings.
- A 1.0 stability commitment during M2.

## Compatibility baseline

The crate follows the latest stable Rust toolchain and current stable edition policy. At proposal time the repository uses Rust 1.98 and edition 2024.

When stable Rust advances:

1. update toolchain/MSRV metadata in one focused change;
2. run the complete verification suite on the new stable toolchain;
3. do not merge the bump until fmt, Clippy with warnings denied, all-feature tests, rustdoc tests, and Loom/model checks pass;
4. record any required source or dependency changes in the PR.

Optional integration dependencies must remain feature-gated. Core semantics must compile without Tokio, Tower, or tracing enabled.

## Adapter boundary

Adapters translate between ecosystem-specific request/execution types and core resilience decisions. They must not invent policy that is invisible to the core API.

An adapter may:

- obtain the current immutable routing snapshot;
- build one immutable eligibility decision and route plan;
- look up per-route execution resources;
- enforce deadlines and sleeps using its runtime;
- execute physical attempts;
- classify outcomes through caller-provided policy;
- emit core events/observations;
- perform explicit bounded retry and explicit bounded route failover.

An adapter must not:

- retry without consuming the configured physical-attempt policy;
- fail over without consuming the route-attempt budget;
- select a route from a newer generation during one logical request;
- extend work beyond the logical-request deadline;
- allow shadow work to consume primary breaker/bulkhead/retry-budget resources;
- allow shadow completion or failure to alter the primary result.

## Logical-request deadline

Every production execution starts with one optional absolute logical deadline or equivalent remaining-budget representation.

The adapter computes remaining time before each potentially blocking step. The remaining budget is monotonically non-increasing and is shared by:

- primary physical attempts;
- retry backoff sleeps;
- explicit route failover;
- any adapter-level timeout wrapper.

No child operation may create a later deadline than its parent. A retry or failover that cannot begin and complete within the remaining policy budget stops normally with an explicit deadline/budget outcome rather than sleeping past the outer deadline.

Shadow execution receives its own `ShadowExecutionPolicy`, but its effective deadline is clamped to the primary request's remaining budget as required by Spec 0002.

## Retry budget across requests

Per-request retry limits prevent a single request from retrying forever but do not prevent a retry storm when many requests fail simultaneously. M2 therefore introduces an optional shared retry budget.

Initial semantics should use a bounded token-bucket or equivalent deterministic accounting model:

- one initial physical attempt never requires a retry token;
- each retry attempt requires one token before it starts;
- tokens replenish at an explicit bounded rate or via an explicit success-based policy;
- lack of a token suppresses the retry rather than failing the already-completed attempt retroactively;
- accounting uses checked arithmetic and defined saturation behavior;
- budget state is independent from route-failover budget;
- shadow traffic uses a separate retry budget or no retry budget and never consumes the primary retry budget.

The core budget/accounting type must not require Tokio timers. Time is supplied by caller/runtime adapters through explicit instants, elapsed durations, or deterministic test inputs.

## Per-route resource registry

Runtime resources are keyed by stable route identity and live outside immutable routing snapshots.

The registry may contain:

- circuit breaker state;
- primary bulkhead state;
- shadow bulkhead state;
- primary retry budget state;
- shadow retry budget state;
- adapter-specific pools or handles when held by the adapter layer.

Route-table publication must not partially mutate registry state. A new route may lazily create resources; removal may retire resources according to an explicit lifecycle policy. An already-created `RoutePlan` must remain executable against the resources associated with its route IDs for the lifetime required by the adapter.

Registry APIs must make primary and shadow resource namespaces distinct enough that accidental sharing is difficult to express.

## Tower adapter

Tower integration is optional and feature-gated.

The first Tower adapter should be deliberately small:

- implement a `Layer`/`Service` boundary around the verified core policy;
- avoid requiring Tower types in core modules;
- preserve Tower readiness/backpressure semantics rather than treating `poll_ready` failure as a downstream transport failure;
- surface overload/admission rejection distinctly from downstream failures;
- avoid holding locks or permits across unrelated await points;
- keep request cloning/replay requirements explicit because retries cannot assume arbitrary requests are cloneable.

If request replay cannot be expressed generically without unsafe or surprising cloning, the initial adapter should require an explicit attempt factory/closure rather than hiding request duplication.

## Tokio adapter

Tokio support remains optional and is responsible only for runtime mechanics such as:

- deadline/timeout enforcement;
- backoff sleeping;
- cancellation propagation;
- optional detached shadow execution when the caller explicitly provides ownership and lifecycle semantics.

Core policy types must continue to compile with the Tokio feature disabled.

Cancellation is not automatically a downstream failure. The adapter must distinguish at least caller cancellation, logical-deadline expiry, overload/admission rejection, breaker rejection, and transport/application failure where the core classifier requires that distinction.

## Core observability model

The core exposes zero-dependency observations/events as plain Rust values. It does not initialize a metrics recorder or tracing subscriber.

Events should cover at least:

- attempt admitted/rejected;
- retry scheduled/suppressed;
- breaker state transition;
- bulkhead admission/rejection;
- routing generation and selected route;
- route failover ordinal/outcome;
- shadow sampled/not-sampled and isolated shadow outcome;
- logical deadline/budget exhaustion.

Events should carry bounded metadata suitable for an adapter to map to metrics or spans. Stable route IDs are allowed; arbitrary request-derived strings are not emitted as built-in metric labels.

## Optional tracing adapter

A `tracing` integration may translate core observations to events/spans behind an optional feature.

Requirements:

- no tracing dependency in the default feature set;
- preserve the core outcome vocabulary rather than flattening every stop condition to `error`;
- do not record request bodies, secrets, or arbitrary headers;
- document which fields are safe as span fields versus metric labels;
- keep routing generation, failover ordinal, retry ordinal, and shadow outcome available for correlation.

## Metrics/cardinality policy

Built-in metric adapters must have bounded label sets.

Allowed dimensions should be enumerated and reviewed. Candidate dimensions include operation class chosen by the application, stable route ID, outcome class, breaker transition, and primary-versus-shadow role.

Request ID, full URL, raw error string, customer ID, trace ID, and other unbounded values must not be built-in metric labels.

Histograms/timing hooks should observe values such as retry backoff and downstream attempt latency without forcing a particular metrics backend.

## Concurrency and correctness verification

M2 expands correctness-first verification rather than using load tests as a substitute for race reasoning.

Required checks include:

- Loom/model tests for breaker transition races that can affect admission correctness;
- Loom/model tests for bulkhead permit accounting and release races;
- deterministic retry-budget accounting tests, including saturation and replenishment boundaries;
- cancellation/deadline tests proving no hidden extra attempt begins after budget expiry;
- adapter tests proving route generation stays constant for one logical request;
- adapter tests proving route failover and physical retry consume distinct budgets;
- adapter tests proving shadow resources and outcomes remain isolated from primary resources/results;
- property/fuzz tests for public configuration constructors and state-transition inputs.

Any synchronization optimization must preserve these invariants and must be justified by benchmark evidence.

## Benchmarks

Criterion or an equivalent dev-only benchmark harness may be added in M2.

Benchmark before replacing simple synchronization with lock-free structures. At minimum measure:

- uncontended and contended breaker admission/state transitions;
- bulkhead acquire/release overhead;
- immutable route snapshot reads and publication;
- route planning over representative route counts;
- retry-budget token accounting;
- observability hook overhead with hooks disabled and enabled.

Benchmarks are evidence for optimization decisions, not CI pass/fail gates unless a stable regression methodology is later defined.

## Dependency and feature policy

- Default features remain minimal.
- Runtime/ecosystem integrations are optional.
- Every new runtime dependency requires a concrete adapter use case.
- Avoid duplicate abstraction layers that reproduce existing Tower/Tokio behavior without preserving a resilience-specific invariant.
- New mandatory dependencies require dependency/license review and a clear benefit over `std`.

## Proposed feature layout

The exact names may change during implementation, but the intended boundary is:

- default: runtime-agnostic core only;
- `tokio`: Tokio runtime mechanics;
- `tower`: Tower adapter;
- `tracing`: tracing adapter;
- development-only dependencies for Loom, fuzz/property testing, and benchmarks.

Feature combinations must compile independently where meaningful, and `--all-features` remains part of the merge gate.

## API and semver hardening

Before any public release candidate:

- audit public types for accidental runtime coupling;
- audit constructors for invalid-state prevention;
- document cancellation, retry, failover, and shadow composition rules;
- ensure errors distinguish configuration errors, admission/policy stops, and execution failures where callers need different behavior;
- run a semver/public-API diff against the previous published baseline once releases exist;
- maintain a changelog for user-visible changes.

## Release/publishing gate

M2 is not Verified until:

- Spec 0003 requirements implemented or explicitly deferred by spec amendment;
- required deterministic, concurrency/model, adapter, and documentation tests pass;
- dependency and license review is documented;
- default-features and all-features builds pass on latest stable Rust;
- rustdoc examples for production-facing APIs are runnable;
- changelog/release automation and crates.io checklist exist;
- no unresolved known correctness bug exists in retry, breaker, bulkhead, routing, failover, shadow isolation, deadline propagation, or retry-budget accounting.

A 1.0 release remains out of scope until concurrency semantics, cancellation behavior, observability contracts, and composition rules are verified under tests.

## Proposed implementation sequence

1. **M2.1 — Core execution context and deadline budget**: runtime-agnostic logical-request deadline/remaining-budget semantics plus deterministic tests.
2. **M2.2 — Shared retry budget**: bounded cross-request token accounting with deterministic clock/input hooks and separate primary/shadow namespaces.
3. **M2.3 — Per-route resource registry**: explicit primary/shadow resource separation and lifecycle semantics.
4. **M2.4 — Zero-dependency observability hooks**: bounded event vocabulary and metadata/cardinality contract.
5. **M2.5 — Tokio integration**: runtime timeout/backoff/cancellation mechanics preserving M2.1 budgets.
6. **M2.6 — Tower integration**: `Layer`/`Service` adapter with explicit replay/attempt-factory semantics and readiness correctness.
7. **M2.7 — Optional tracing/metrics adapters**: translations from core observations without high-cardinality defaults.
8. **M2.8 — Hardening**: Loom/model expansion, fuzz/property tests, criterion benchmarks, dependency/license audit, semver/API review.
9. **M2.9 — Release engineering**: changelog, release automation, crates.io checklist, runnable public examples, and final verification record.

## Exit criteria

Spec 0003 can be promoted to **Verified** only when the complete latest-stable Rust verification gate passes and evidence demonstrates that production adapters preserve the verified invariants of Specs 0001 and 0002 rather than introducing hidden retries, hidden failover, deadline extension, resource sharing, or shadow-to-primary failure propagation.
