# animaOS Rust Workspace

This workspace contains the canonical runtime core for animaOS.

Phase 0 guarantees only:

- a contained Cargo workspace under `packages/animaos-rs`
- four compilable crates: `anima-core`, `anima-memory`, `anima-swarm`, `anima-daemon`
- a daemon milestone that responds to `GET /health`

The Rust crates now own runtime execution, swarm coordination, memory services, and the daemon API boundary. The TypeScript packages are the SDK, CLI, UI, and shared-support layer around that runtime.
