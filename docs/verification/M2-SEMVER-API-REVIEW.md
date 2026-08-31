# M2 SemVer and Public API Review

Status: **Verified**

Scope: public API and release-surface review before M2 release engineering. This review does not change runtime behavior, routing, retry budgets, concurrency semantics, or primary/shadow isolation.

## Baseline

- Package: `softwheel-resilience` `0.1.0`
- Rust edition: 2024
- MSRV/toolchain policy: Rust 1.98 / latest stable at review time
- Default features: empty; core remains runtime-agnostic
- Optional integrations: `tokio`, `tower`, `tracing`, `metrics`

## Public-surface findings

1. **Core/runtime boundary is explicit.** Tokio, Tower, tracing, and metrics APIs are feature-gated; the default public surface remains independent of an async runtime.
2. **Primary/shadow resource isolation is type-visible.** `PrimaryRouteResources` / `ShadowRouteResources` and `PrimaryRetryBudget` / `ShadowRetryBudget` are separate public types. Do not collapse these into a shared public type without a new spec and compatibility review.
3. **Routing identity is part of the compatibility surface.** `RouteId`, routing generation metadata, and shadow outcome vocabulary are externally observable and must be treated as semver-relevant.
4. **Extension-oriented observability enums are non-exhaustive.** `RejectionReason`, `RetrySuppressionReason`, `OutcomeClass`, `BudgetStage`, and `ResilienceEvent` may gain variants as policies and observability evolve, so they are marked `#[non_exhaustive]` before the first crates.io publication. This lets downstream users remain source-compatible when new variants are added.
5. **Intentionally closed enums remain exhaustive.** `TrafficRole` and `ShadowSamplingOutcome` model closed binary domains and remain exhaustive. Circuit-breaker state/configuration enums likewise remain closed unless a future design change requires expansion; changing that policy requires an explicit API review.
6. **Concrete public structs expose construction policy.** Any future decision to expose fields directly, change constructor invariants, or replace concrete resource types with traits is API-significant and must not be bundled with synchronization optimization.
7. **Feature relationships are API contracts.** `tower` currently implies `tokio`; default features are empty. Changing those relationships affects dependency/runtime behavior and requires release-note treatment.
8. **Repository metadata requires rename coordination.** `Cargo.toml` points at the intended `softwheel/svc-resilience-rs` URL while the canonical GitHub repository is still `softwheel/svc-resillience-rs`. Do not publish until the repository rename/redirect is confirmed so crates.io metadata does not ship a broken canonical link.

## Compatibility policy for the next release

- Keep the crate pre-1.0 until Spec 0003 and M2 verification close.
- Preserve empty default features and runtime-agnostic core semantics.
- Treat primary/shadow isolation types and budget boundaries as correctness contracts, not convenience APIs.
- Keep synchronization/performance changes behavior-preserving and benchmarked separately from API changes.
- Record every public type/function/feature removal or signature change in the changelog.
- Use `#[non_exhaustive]` for public taxonomies expected to grow; keep only semantically closed domains exhaustive.
- Once a crates.io baseline exists, add automated API compatibility checking against the latest published release; before the first publish, a source/API review is the meaningful baseline.

## Publication blockers and follow-ups

- [ ] Rename/redirect the GitHub repository from `svc-resillience-rs` to intended `svc-resilience-rs`, or temporarily align package metadata with the canonical location before publishing.
- [x] Decide whether extension-oriented public enums should be marked `#[non_exhaustive]` before the first crates.io publication. The observability taxonomies that may grow are non-exhaustive; intentionally closed domains remain exhaustive.
- [x] Add an external compile-fail doctest proving downstream exhaustive matching is rejected for the non-exhaustive rejection taxonomy.
- [x] Produce changelog/release automation and crates.io publishing checklist in M2.9 (#45, #46).
- [x] Complete the final M2 verification record and latest-stable verification PR.

## Verification conclusion

The current API shape is verified for the M2 pre-1.0 milestone. Runtime/ecosystem dependencies remain optional, primary/shadow isolation remains type-visible, and the extensibility policy is now explicit before first publication. Repository naming remains the final administrative pre-publication blocker. The enum-policy change does not alter concurrency, budget, routing, or shadow-isolation semantics and must pass the complete latest-stable verification suite before merge.
