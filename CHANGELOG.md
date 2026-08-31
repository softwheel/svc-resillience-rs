# Changelog

All notable changes to this project will be documented in this file.

The format is based on Keep a Changelog, and this project follows Semantic Versioning once public releases begin.

## [Unreleased]

### Added

- Runtime-agnostic resilience primitives for retry, circuit breaking, bulkheading, dynamic routing, failover, and isolated traffic shadowing.
- Explicit logical-request deadline and remaining-budget semantics.
- Shared retry budgets with distinct primary and shadow budget types.
- Per-route resource registries with strict primary/shadow namespace separation.
- Zero-dependency observability events plus optional `tracing` and `metrics` adapters.
- Optional Tokio runtime mechanics and Tower `Layer`/`Service` integration.
- Deterministic property/state-transition tests, Loom concurrency models, Criterion baselines, and dependency/license auditing.

### Changed

- Mark extension-oriented observability taxonomies (`RejectionReason`, `RetrySuppressionReason`, `OutcomeClass`, `BudgetStage`, and `ResilienceEvent`) as `#[non_exhaustive]` before the first crates.io release so future variants can be added without breaking downstream exhaustive matches.

### Verification

- Spec 0001 / M0 is Verified.
- Spec 0002 routing and shadowing semantics are Verified.
- Spec 0003 / M2 production integration and observability is Verified.
- Release-boundary API changes require the full latest-stable Rust gate before merge: formatting, Clippy with warnings denied, all-feature tests, benchmark compilation, rustdoc with warnings denied, Loom models, and dependency/license audit.

### Release blockers

- Rename the canonical GitHub repository from `svc-resillience-rs` to `svc-resilience-rs`, or otherwise establish and verify the intended canonical redirect, before crates.io publication.
