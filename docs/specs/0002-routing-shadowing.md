# Spec 0002: Dynamic Routing and Isolated Traffic Shadowing

Status: **Verified**

## Problem

A resilient client needs more than retry, circuit breaking, and concurrency limits. It also needs to select among eligible destinations, react to policy/health changes without blocking readers, and optionally mirror a bounded fraction of requests for validation or migration.

Routing and shadowing introduce failure-coupling risks that must be explicit before implementation. A route failover is not a retry, a shadow request is not a second primary, and mutable routing configuration must not create partially-observed snapshots.

## Goals

1. Immutable route-table snapshots with cheap concurrent reads.
2. Atomic replacement of the complete routing snapshot.
3. Weighted primary selection with deterministic test hooks.
4. Health/policy filtering before weighting.
5. Explicit route-failover semantics distinct from retry semantics.
6. Optional traffic shadowing with strict isolation from the primary path.
7. Bounded, deterministic shadow sampling.
8. Generation metadata suitable for observability and stale-result analysis.
9. Runtime-agnostic core planning APIs; execution/adapters remain separate.

## Non-goals

- Service discovery or endpoint resolution.
- Active health checking.
- Distributed consensus for route-table updates.
- Global load balancing across processes.
- HTTP/gRPC-specific routing policy.
- Executing background tasks in the core crate.
- Making shadow results visible to the primary caller.

## Core model

A routing snapshot is an immutable value containing:

- monotonically increasing `generation: u64`;
- a non-empty set of route definitions;
- route identity and weight;
- route eligibility/policy state;
- optional shadow configuration.

Readers obtain one snapshot and make the complete primary/shadow plan from that snapshot. They must never combine fields from different generations.

A route definition contains only planning metadata. Runtime resources such as breakers, bulkheads, connection pools, and timers live in an external per-route policy/resource registry keyed by stable route identity.

## Snapshot publication invariants

- Snapshot construction validates all configuration before publication.
- Publication is all-or-nothing: readers observe either the old complete snapshot or the new complete snapshot.
- Reads do not take a global write lock.
- Replacing a snapshot does not invalidate an already-created `RoutePlan`.
- Generation increases on every successful replacement.
- Generation wraparound must not silently make an old snapshot appear newer; initial implementation should reject replacement at `u64::MAX` rather than wrap.
- A rejected/invalid update leaves the current snapshot unchanged.

The implementation may initially use `RwLock<Arc<RouteTable>>` if it preserves these semantics. Lock-free/ArcSwap-style publication is an optimization, not an M1 correctness requirement, and requires benchmark evidence before adding another mandatory dependency.

## Route eligibility

Filtering happens before weighted selection. A route is eligible only when all configured planning predicates accept it.

Core route eligibility represents declarative policy only (for example enabled/disabled or caller-supplied health classification). Circuit-breaker admission remains per physical attempt during execution and is not mutated during planning.

If no route is eligible, planning returns `NoEligibleRoute` without consuming retry budget and without manufacturing a transport failure.

## Weighted primary selection

For eligible routes with positive integer weights:

- total weight uses checked arithmetic;
- zero-weight routes are never selected;
- selection draws one integer in `[0, total_weight)`;
- deterministic RNG/source injection is available for tests;
- route ordering is stable for a given snapshot;
- selection must not retry internally.

Weight is relative, not a percentage. `[1, 1]` and `[50, 50]` express the same distribution.

Invalid tables (all eligible routes weight zero, duplicate route IDs, overflowed total weight) are rejected at construction/update time.

## Route failover semantics

A logical request has one routing snapshot generation and an explicit route-attempt policy.

Default M1 behavior is **single-route planning**: select one eligible primary route. The existing retry policy may retry physical attempts against that selected route according to caller classification.

Optional failover, when introduced in M1, must be explicit and bounded:

1. select a primary route;
2. execute that route's bounded retry/attempt policy;
3. only caller-classified route-terminal outcomes may trigger failover;
4. select another eligible route without revisiting a previously attempted route;
5. each failover consumes a separate route-attempt budget;
6. no route selection step performs hidden transport retries.

Circuit-breaker rejection is normally route-terminal and may permit explicit failover, but it is not counted as a downstream transport failure.

## Route plan

Planning returns an immutable `RoutePlan` containing at least:

- routing `generation`;
- selected primary route ID;
- optional selected shadow route ID;
- shadow sampling decision;
- enough metadata for an executor to look up isolated policy resources.

The plan contains no futures, tasks, sockets, or runtime handles.

## Shadow sampling

Shadowing is opt-in and disabled by default.

Sampling is represented as an integer fraction rather than floating point. Initial API should use parts-per-million (`0..=1_000_000`) or an equivalent exact integer representation.

- `0` never samples;
- maximum always samples;
- sampling uses a caller-injectable deterministic source for tests;
- the primary route decision does not depend on whether the request is sampled for shadowing;
- an invalid sampling configuration is rejected before publication.

## Shadow route selection

A shadow route must be distinct from the primary route unless a future spec explicitly adds same-destination duplication.

Shadow eligibility and weighting may reuse the same route metadata but are evaluated independently. Failure to find an eligible shadow route degrades to `shadow = None`; it never fails primary planning.

## Isolation contract

Shadow traffic is diagnostic/validation traffic, not a second primary path.

The executor that consumes a `RoutePlan` must enforce:

- separate shadow bulkhead capacity;
- separate breaker/failure accounting;
- separate retry policy/budget;
- a shadow deadline no later than the logical primary deadline and preferably shorter;
- no primary permit/resource is held solely for shadow execution;
- shadow saturation cannot consume primary bulkhead permits;
- shadow breaker state cannot affect primary breaker state;
- shadow errors are observable but never returned as the primary result;
- primary completion never waits for shadow completion.

The core planning crate does not spawn the shadow task. Runtime adapters own dispatch and cancellation semantics while conforming to this contract.

## Cancellation and overload

If shadow work cannot be admitted immediately to its isolated bulkhead, it is dropped/rejected as a shadow outcome.

If the primary operation completes, cancellation of an already-dispatched shadow is adapter policy, but primary completion must not await that cancellation.

A runtime adapter must be able to enforce a bounded lifetime for shadow work. Detached unbounded shadow tasks are forbidden.

## Composition semantics

Primary execution remains:

`logical deadline -> route plan -> primary bulkhead -> retry loop -> breaker per physical attempt -> attempt timeout -> transport`

Shadow execution is a sibling branch from the immutable route plan:

`shadow sample -> shadow bulkhead -> shadow retry loop -> shadow breaker -> shadow timeout -> shadow transport`

The two branches share routing metadata only. They do not share mutable resilience accounting.

## Error taxonomy

Planning errors are distinct from execution errors. Initial planning errors should include at least:

- invalid route table/configuration;
- no eligible primary route;
- route-generation exhaustion.

A missing/invalid shadow choice is non-fatal to primary planning unless the snapshot itself is structurally invalid.

## Observability contract

Core types should expose data required for adapters to emit events without depending on a telemetry crate:

- routing generation;
- primary route ID;
- optional shadow route ID;
- sampled/not-sampled decision;
- planning rejection reason;
- route-failover ordinal when failover is enabled.

No built-in API should encourage unbounded/high-cardinality labels beyond caller-controlled route identifiers.

## Proposed implementation slices

### M1.1 — immutable route model and validation

- `RouteId` and route definition types;
- `RouteTable` validation;
- generation metadata;
- deterministic weighted primary selection;
- unit/property tests for weight bounds and invalid tables.

### M1.2 — concurrent snapshot publication

- route-table holder with whole-snapshot replacement;
- concurrent readers/update test proving no mixed-generation observations;
- rejected updates preserve current snapshot.

### M1.3 — shadow planning

- integer sampling configuration;
- deterministic sampling hook;
- primary + optional shadow `RoutePlan`;
- tests proving shadow planning failure cannot fail the primary plan.

### M1.4 — execution isolation contract

- runtime-agnostic executor-facing interfaces only if needed;
- Tokio integration tests may live behind the existing optional feature;
- prove separate breaker/bulkhead accounting and non-blocking primary completion.

### M1.5 — explicit route failover

- bounded route-attempt budget;
- no route revisits within one logical request;
- classification API separating retryable attempt errors from failover-eligible route-terminal outcomes;
- deterministic tests for route ordering/failover.

## Verification plan

- property tests or exhaustive deterministic tests for weighted selection boundaries;
- deterministic distribution sanity tests across a large fixed sample;
- snapshot replacement concurrency tests;
- generation monotonicity and invalid-update tests;
- sampling boundary tests (`0`, maximum, intermediate deterministic values);
- tests proving shadow selection failure cannot alter primary planning;
- integration tests proving shadow failure cannot trip the primary breaker;
- integration tests proving shadow saturation cannot consume primary bulkhead capacity;
- integration tests proving primary completion does not await shadow completion;
- stable Rust fmt, Clippy, all-feature tests, rustdoc, and existing Loom checks remain green.

## Exit criteria for Verified

- all snapshot, selection, failover, and isolation invariants above have automated coverage;
- no hidden retries exist in route planning;
- concurrent snapshot replacement cannot expose mixed generations;
- shadow failures cannot change the primary result or mutable primary resilience accounting;
- primary latency is not coupled to shadow completion;
- public APIs have runnable rustdoc examples;
- latest stable Rust CI is green.

## Verification record

Verified on 2026-08-30 after M1.1 through M1.6c3 landed in PRs #13 through #22.

The verification evidence includes deterministic weighted-selection and sampling tests, whole-snapshot concurrency tests, bounded route-failover tests, immutable per-generation eligibility tests, shadow breaker/bulkhead isolation tests, primary-latency isolation tests, bounded shadow retry/deadline semantics, observable non-propagating shadow outcomes, and runnable public rustdoc examples.

The complete CI gate passed on Rust 1.98.0: `cargo fmt --check`, Clippy with `-D warnings`, all-feature tests, rustdoc tests, and Loom. Rust 1.98.0 is the latest stable Rust release at verification time.
