# M2 SemVer and Public API Review

Status: **Proposed verification record**

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
4. **Closed observability enums are semver-sensitive.** Adding variants to public enums such as outcome/rejection/suppression classes can break exhaustive downstream matches. Before 1.0 this remains permitted under Cargo semver rules, but release notes must call it out; after 1.0 prefer `#[non_exhaustive]` where extension is expected.
5. **Concrete public structs expose construction policy.** Any future decision to expose fields directly, change constructor invariants, or replace concrete resource types with traits is API-significant and must not be bundled with synchronization optimization.
6. **Feature relationships are API contracts.** `tower` currently implies `tokio`; default features are empty. Changing those relationships affects dependency/runtime behavior and requires release-note treatment.
7. **Repository metadata requires rename coordination.** `Cargo.toml` points at the intended `softwheel/svc-resilience-rs` URL while the canonical GitHub repository is still `softwheel/svc-resillience-rs`. Do not publish until the repository rename/redirect is confirmed so crates.io metadata does not ship a broken canonical link.

## Compatibility policy for the next release

- Keep the crate pre-1.0 until Spec 0003 and M2 verification close.
- Preserve empty default features and runtime-agnostic core semantics.
- Treat primary/shadow isolation types and budget boundaries as correctness contracts, not convenience APIs.
- Keep synchronization/performance changes behavior-preserving and benchmarked separately from API changes.
- Record every public type/function/feature removal or signature change in the changelog.
- Once a crates.io baseline exists, add automated API compatibility checking against the latest published release; before the first publish, a source/API review is the meaningful baseline.

## Release blockers discovered by this review

- [ ] Rename/redirect the GitHub repository from `svc-resillience-rs` to intended `svc-resilience-rs`, or temporarily align package metadata with the canonical location before publishing.
- [ ] Decide whether extension-oriented public enums should be marked `#[non_exhaustive]` before the first public release; changing this after downstream users exist is more disruptive.
- [ ] Produce changelog/release automation and crates.io publishing checklist in M2.9.
- [ ] Mark Spec 0003 Verified only after its final verification record is complete.

## Verification conclusion

The current API shape is suitable to proceed to release engineering without production-code changes. The review intentionally identifies repository naming and enum extensibility as pre-publish decisions rather than making speculative semantic changes. Existing correctness-first concurrency verification, explicit budgets, runtime-agnostic core semantics, and strict shadow isolation remain unchanged.
