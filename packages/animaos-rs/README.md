# animaOS Rust Workspace

Canonical runtime core for animaOS.

This workspace owns runtime execution, swarm coordination, memory services, and the daemon API boundary. The TypeScript packages are the SDK, CLI, UI, and shared-support layer around that runtime.

Current runtime coverage includes:

- `anima-core` primitives, events, runtime, and model execution helpers
- `anima-memory` ranking, storage, retrieval, and manager behavior
- `anima-swarm` coordination, message routing, and task lifecycle behavior
- `anima-daemon` health, agent, memory, and swarm HTTP boundaries

## Workspace Layout

- `crates/anima-core`: runtime primitives and execution model
- `crates/anima-memory`: canonical memory services and retrieval logic
- `crates/anima-swarm`: canonical swarm coordination runtime
- `crates/anima-daemon`: HTTP daemon and API surface used by TypeScript clients

## Quick Example

```bash
cargo run --manifest-path packages/animaos-rs/Cargo.toml -p anima-daemon
curl http://127.0.0.1:8080/health
```

## Build

Run `cargo build --manifest-path packages/animaos-rs/Cargo.toml --workspace`.

## Test

Run `cargo test --manifest-path packages/animaos-rs/Cargo.toml --workspace`.
