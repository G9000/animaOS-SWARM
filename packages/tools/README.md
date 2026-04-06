# @animaOS-SWARM/tools

TypeScript tool execution and policy utilities for animaOS-SWARM.

This package exports the tool registry, executor, hook system, permission checks, secret handling, validation, truncation, and shell helpers used by local workflows and test harnesses.

The Rust daemon owns canonical production tool execution, but this package still carries the workspace's shared tool contracts and local utilities.

## Build

Run `bun x nx build @animaOS-SWARM/tools`.

## Test

Run `bun x nx test @animaOS-SWARM/tools`.
