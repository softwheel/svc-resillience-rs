# Contributing

Behavior changes are spec changes. Keep the spec, implementation, tests, and public documentation
in the same pull request whenever possible.

## Rust toolchain policy

Development tracks the latest stable Rust release. `rust-toolchain.toml` follows the `stable`
channel, and `Cargo.toml` records the current stable compiler floor. Before 1.0, the project may
raise that floor when stable Rust advances rather than carrying compatibility work for older
compilers.

## Local checks

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo doc --no-deps --all-features
```

## Design rules

- Prefer explicit budgets and rejection over hidden queues or unbounded retries.
- Do not classify application errors inside generic middleware.
- Keep the core runtime-agnostic; integrations belong behind features/adapters.
- Do not add `unsafe` without a new spec and a compelling measured reason. The crate currently forbids it.
- Concurrency optimizations require benchmarks and race/model tests.
- Shadow traffic must be isolated from primary traffic accounting and results.

## Pull requests

A PR changing resilience semantics should state:

1. Which spec/invariant changes.
2. Which failure mode the change handles.
3. How it is tested under concurrency and cancellation.
4. Whether it changes retry amplification, admission, or routing behavior.
5. Any compatibility or observability impact.
