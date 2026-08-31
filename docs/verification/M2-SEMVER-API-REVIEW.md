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

## Publication blockers and follow-ups

- [ ] Rename/redirect the GitHub repository from `svc-resillience-rs` to intended `svc-resilience-rs`, or temporarily align package metadata with the canonical location before publishing.
- [ ] Decide whether extension-oriented public enums should be marked `#[non_exhaustive]` before the first crates.io publication; this is a publication/API policy decision, not a correctness blocker for M2 verification.
- [x] Produce changelog/release automation and crates.io publishing checklist in M2.9 (#45, #46).
- [x] Complete the final M2 verification record and latest-stable verification PR.

## Verification conclusion

The current API shape is verified for the M2 pre-1.0 milestone. Runtime/ecosystem dependencies remain optional, primary/shadow isolation remains type-visible, and no API change was required to satisfy the correctness contract. Repository naming and enum extensibility remain explicit pre-publication decisions and must be resolved before the first crates.io release, but they do not weaken the verified concurrency, budget, routing, or shadow-isolation semantics.
