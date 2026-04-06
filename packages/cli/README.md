# @animaOS-SWARM/cli

Command-line interface for animaOS-SWARM.

This package builds the local `animaos` binary used for agency scaffolding and daemon-backed commands such as `run`, `chat`, `launch`, and `agents`.

`launch` is the primary TUI workflow. Use `--no-tui` only for automation, CI, or plain-text runs.

## Build

Run `bun run build:cli-sdk` to build the CLI and its SDK dependency, or `bun x nx build @animaOS-SWARM/cli` to build only this package.

## Run

Run `bun run animaos --help` or `bun run animaos launch "your task"`.

## Test

Run `bun x nx test @animaOS-SWARM/cli`.
