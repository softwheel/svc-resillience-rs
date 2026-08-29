# Spec 0001 Toolchain and Dependency Review

Date: 2026-08-29

This record supersedes the original Rust 1.75 MSRV policy for Spec 0001. The project now intentionally tracks the latest stable Rust release during pre-1.0 development.

## Rust policy

- Current stable compiler: Rust 1.98.0.
- Crate edition: Rust 2024.
- `Cargo.toml` records `rust-version = "1.98"`.
- `rust-toolchain.toml` follows the `stable` channel so contributors automatically use the newest stable toolchain.
- CI uses stable Rust for formatting, Clippy, tests, rustdoc, and Loom verification.
- The project does not promise compatibility with older Rust releases during pre-1.0 development.

When a new stable Rust release is adopted, `rust-version` should be advanced in the same pull request and the full verification suite must pass before merge.

## Runtime dependencies

| Dependency | Manifest line | License | Role |
| --- | --- | --- | --- |
| `fastrand` | `2.5` | MIT OR Apache-2.0 | jitter sampling |
| `tokio` | `1.51` | MIT | optional async sleep adapter |

Dependency requirements remain caret-compatible so downstream applications can unify compatible versions. This is especially important for Tokio: a reusable library should not unnecessarily force a specific Tokio minor into an application.

## Verification-only dependencies

| Dependency | Manifest line | License | Role |
| --- | --- | --- | --- |
| `loom` | `=0.7.2` | MIT | exhaustive bulkhead CAS model checking |
| `tokio` | `1.51` | MIT | async retry integration tests |

Loom remains exact-pinned because reproducible model-checking semantics are more important than resolver flexibility for a verification-only dependency.

## CI policy

1. **Stable job:** runs on the latest stable Rust toolchain and executes formatting, Clippy with warnings denied, all-feature tests, and rustdoc.
2. **Loom job:** runs the exhaustive ignored bulkhead model separately on stable Rust so normal test latency remains bounded.

This means the crate optimizes for current compiler capabilities and a small maintenance surface instead of carrying an old-compiler compatibility matrix before the public API is mature.

## Review conclusion

- The project intentionally targets latest stable Rust rather than an old MSRV.
- Rust 2024 is the active edition.
- Runtime dependencies remain optional/minimal where appropriate.
- Dependency licenses remain compatible with the crate's MIT license.
- Any future stable-Rust bump must pass the same M0 verification suite.
