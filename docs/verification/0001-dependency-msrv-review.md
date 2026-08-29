# Spec 0001 Dependency and MSRV Review

Date: 2026-08-29

This review closes the dependency/MSRV verification gate for Spec 0001. The crate declares Rust 1.75 as its minimum supported Rust version and verifies that floor in CI with all features and dev-only verification dependencies enabled.

## Runtime dependencies

| Dependency | Reviewed baseline | Upstream MSRV | License | Role |
| --- | --- | --- | --- | --- |
| `fastrand` | 2.5.0 | Rust 1.63 | MIT OR Apache-2.0 | jitter sampling |
| `tokio` | 1.51.0 | Rust 1.71 | MIT | optional async sleep adapter |

The manifest uses caret-compatible requirements (`2.5` and `1.51`) so downstream applications can unify compatible dependency versions. This is especially important for Tokio: pinning one Tokio minor in a library can force multiple runtime versions into an application, while `retry_async` should run on the application's compatible Tokio 1.x runtime.

## Verification-only dependencies

| Dependency | Reviewed version | Upstream MSRV | License | Role |
| --- | --- | --- | --- | --- |
| `loom` | 0.7.2 | Rust 1.65 | MIT | exhaustive bulkhead CAS model checking |
| `tokio` | 1.51.0 baseline | Rust 1.71 | MIT | async retry integration tests |

Loom is exact-pinned because it is a verification tool rather than a runtime dependency; reproducible model semantics are more valuable here than broad resolver flexibility.

## CI policy

Two dependency views are intentional:

1. **Stable job:** resolves the newest versions allowed by the manifest and runs formatting, Clippy, all-feature tests, and rustdoc. This catches incompatibilities with current dependency releases.
2. **MSRV job:** resolves the reviewed direct-dependency baseline (`fastrand 2.5.0`, `tokio 1.51.0`, `loom 0.7.2`) and runs all-feature tests on Rust 1.75. This makes the minimum-Rust claim reproducible instead of depending on whatever upstream minor release happens to be newest.
3. **Loom job:** runs the exhaustive ignored model separately so normal unit-test latency stays bounded.

Rust 2021 uses Cargo resolver v2, which does not automatically make dependency resolution Rust-version-aware. Keeping an explicit MSRV baseline avoids relying on newer Cargo resolver behavior that is unavailable on Rust 1.75.

## Review conclusion

- All direct dependencies have an upstream MSRV below Rust 1.75 at the reviewed baseline.
- Runtime dependency licenses are compatible with the crate's MIT license.
- Tokio remains optional and is not required by the runtime-agnostic core.
- The stable job remains the compatibility canary for newer dependency releases.
- The Rust 1.75 job is the release gate for the declared MSRV baseline.

Any change to the declared Rust floor, direct dependency major/minor baseline, or Tokio integration semantics must update this review or add a successor verification record.
