# sdk

Public TypeScript SDK for animaOS.

This package builds agent and swarm configs, re-exports shared TypeScript types, and talks to the Rust daemon over HTTP and SSE. It does not embed the execution runtime.

## Building

Run `nx build sdk` to build the library.

## Running unit tests

Run `nx test sdk` to execute the unit tests via [Vitest](https://vitest.dev/).
