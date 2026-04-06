# @animaOS-SWARM/sdk

Public TypeScript client for the animaOS Rust daemon.

This package exports `createDaemonClient`, `AgentsClient`, `SwarmsClient`, and the `agent()`, `action()`, `plugin()`, and `swarm()` helpers. It talks to the daemon over HTTP and SSE and does not embed the execution runtime.

## Build

Run `bun run build:cli-sdk` to build the SDK and CLI together, or `bun x nx build @animaOS-SWARM/sdk` to build only this package.

## Test

Run `bun x nx test @animaOS-SWARM/sdk`.
