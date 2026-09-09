# Envoy policy API verification

Scope: Spec 0004 configuration/policy milestone, verified locally on 2026-09-09.
Compiler: Rust 1.98.1 stable. The merge gate additionally requires the PR's CI results.

Passed:

- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --all-features` (140 passing tests across suites, including 11 adapter contract tests; 3 Loom tests ignored here)
- `cargo test --features envoy --test envoy_policy` (no runtime feature required)
- `RUSTDOCFLAGS='-D warnings' cargo doc --no-deps --all-features`
- `cargo bench --no-run --all-features`
- `cargo test --release loom_models_ -- --ignored` (3 passing models)
- Rust public API export -> `scripts/envoy_bootstrap.py` -> official Envoy 1.35.0 Linux x86_64 release binary: `configuration '/tmp/envoy-bootstrap.json' OK`.

Dependency/license audit is delegated to the existing CI job. Do not describe it as locally run.

Envoy binary validation proves schema acceptance of the shared fixture after Rust export.
No live traffic, xDS connection, endpoint ejection, mirror execution or full proxy parity was tested
or implemented. The public APIs expose policy decisions and resource permits; transport ownership
and unsupported features are enumerated in Spec 0004.
