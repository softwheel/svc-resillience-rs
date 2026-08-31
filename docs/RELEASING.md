# Release process

This crate is correctness-first. A release must not bypass the verification gates used for concurrency, budgets, routing, or shadow isolation.

## Preconditions

- The working tree is based on the latest `main`.
- The canonical repository is `softwheel/svc-resilience-rs`, or the current misspelled repository has been renamed with a verified GitHub redirect.
- `Cargo.toml` package metadata resolves to the canonical repository.
- `rust-toolchain.toml` and `package.rust-version` match the latest stable Rust release required by project policy.
- The current Rust edition remains the project edition unless an explicit spec/review changes it.
- Spec 0001 and Spec 0002 remain Verified, and the current milestone/spec verification record is complete.
- `CHANGELOG.md` contains the release notes and no unresolved release blockers.

## Required verification

Run the complete gate on the exact release candidate commit:

```text
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo bench --no-run --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features
cargo deny check
```

Run the repository's dedicated Loom/model-checking CI job as well. A normal test pass is not a substitute for the bounded concurrency models.

If stable Rust has advanced since the previous merge, first update toolchain metadata in a dedicated PR and require this entire gate again before any release PR merges.

## Package verification

Before publication:

```text
cargo package
cargo publish --dry-run
```

Inspect the packaged file list and generated metadata. Verify that examples/docs do not depend on repository-only files and that optional runtime integrations remain optional.

## Correctness release checks

Confirm that no release change weakens these contracts:

- primary execution is never controlled by shadow execution;
- primary and shadow retry budgets/resources remain isolated;
- shadow failures, saturation, timeout, cancellation, or observer failures cannot change the primary result;
- logical-request budgets remain explicit and monotonic;
- core semantics remain runtime-agnostic;
- dynamic routing snapshots remain immutable for readers and generation/route identity remain observable;
- public observability labels remain bounded and do not admit arbitrary request-derived cardinality.

## Publication

1. Freeze the release candidate commit after the full gate passes.
2. Change the `Unreleased` changelog section to the release version/date.
3. Confirm the package version and repository metadata.
4. Create the signed/reviewed release tag according to repository policy.
5. Run `cargo publish --dry-run` again on the tagged contents or exact tag commit.
6. Publish to crates.io only after all preceding checks are green.
7. Create the GitHub release from the changelog entry.
8. Re-open an `Unreleased` section for subsequent development.

## Roll-forward policy

Do not rewrite or replace an already published crate version. If a release defect is discovered, fix it on `main`, run the entire verification gate, and publish a new semver-compatible patch version when appropriate.
