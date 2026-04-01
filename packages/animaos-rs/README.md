# animaOS Rust Workspace

This workspace contains the Rust rewrite of the animaOS engine.

Phase 0 guarantees only:

- a contained Cargo workspace under `packages/animaos-rs`
- four compilable crates: `anima-core`, `anima-memory`, `anima-swarm`, `anima-daemon`
- a daemon milestone that responds to `GET /health`

The existing TypeScript packages remain the reference implementation while the Rust engine reaches parity.
