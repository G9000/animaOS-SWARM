# animaOS Rust Core

Reusable Rust runtime crates for animaOS. This package owns the core execution, memory, and swarm libraries.

It does not own the runnable HTTP daemon. The current Rust host remains at [`hosts/rust-daemon/crates/anima-daemon`](../../hosts/rust-daemon/crates/anima-daemon).

## Crates

| Crate | What it does |
|---|---|
| [`anima-core`](crates/anima-core) | Agent execution loop, trait interfaces, and runtime primitives. No HTTP or database implementation details. |
| [`anima-memory`](crates/anima-memory) | BM25-backed memory storage and retrieval helpers. |
| [`anima-swarm`](crates/anima-swarm) | Multi-agent coordination strategies built on top of `anima-core`. |

## Build

From the repo root:

```bash
cargo build -p anima-core
cargo build -p anima-memory
cargo build -p anima-swarm
```

## Test

From the repo root:

```bash
cargo test -p anima-core
cargo test -p anima-memory
cargo test -p anima-swarm
```
