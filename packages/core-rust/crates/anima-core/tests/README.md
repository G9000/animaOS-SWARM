`anima-core` integration test layout:

- `runtime_success.rs` covers public happy-path runtime behavior.
- `runtime_failures.rs` covers public failure-path runtime behavior.
- `support/mod.rs` holds shared adapters, providers, evaluators, and helpers for the public boundary suite.

Keep narrow implementation checks in `src/*.rs` unit tests and put consumer-facing runtime scenarios in `tests/*.rs`.