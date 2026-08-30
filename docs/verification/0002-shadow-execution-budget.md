# Spec 0002 verification: shadow retry/deadline budget

This note records the M1.6c shadow-execution budget implemented by `ShadowExecutionPolicy`.

## Contract

- Shadow execution has its own hard duration budget.
- A zero shadow deadline is rejected.
- The effective shadow deadline is `min(configured_shadow_deadline, primary_remaining_budget)`, so shadow work can never be authorized to outlive the primary request budget.
- Retries are disabled by default. The conservative policy therefore performs at most one physical shadow attempt unless the caller explicitly supplies an independent `RetryPolicy`.
- If retries are enabled, retry admission is bounded by both the supplied retry policy and the outer shadow deadline.
- The core policy only decides retry admission/sleep. Runtime adapters remain responsible for enforcing the effective deadline around transport execution and cancellation.
- Shadow retry state is a separate value from the primary retry policy; the API does not provide a shared mutable retry-accounting path.

## Verification

Unit tests cover:

- zero-deadline rejection;
- no-retry conservative behavior;
- clamping to the primary remaining budget;
- explicit retry opt-in;
- rejection of a retry whose delay would exceed the outer shadow deadline;
- caller-classified `DoNotRetry` remaining terminal.

The public type also includes a runnable rustdoc example demonstrating the conservative default and deadline clamping.
