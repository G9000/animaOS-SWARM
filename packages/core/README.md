# core

Shared TypeScript contracts and utilities for the SDK, CLI, and UI.

This package is not the canonical execution runtime. Runtime execution, swarm coordination, memory services, and daemon-backed streaming live in the Rust workspace under `packages/animaos-rs`.

## Building

Run `nx build core` to build the library.

## Running unit tests

Run `nx test core` to execute the unit tests via [Vitest](https://vitest.dev/).
