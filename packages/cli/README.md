# @animaOS-SWARM/cli

Command-line interface for animaOS-SWARM.

This package builds the local `animaos` binary used for agency scaffolding and daemon-backed commands such as `run`, `chat`, `launch`, and `agents`.

`launch` is the primary TUI workflow. Use `--no-tui` only for automation, CI, or plain-text runs.

Plain-text launch supports both one-shot and interactive flows. In interactive `--no-tui` mode, the prompt supports `exit`, `/help`, and `/health`, and it prints daemon warning or recovery lines when connectivity changes.

Current CLI coverage includes:

- agency scaffolding through `create`
- daemon-backed execution through `run`, `chat`, `launch`, and `agents`
- TUI and plain-text launch flows, including `/help` and `/health` in interactive `--no-tui` mode
- provider and API-key forwarding into daemon-backed runs
- exported commander entrypoints via `buildProgram()` and `main()`

## Quick Example

```bash
# Scaffold a new agency
bun run animaos create demo-team --yes

# Run a daemon-backed task
bun run animaos run "Summarize the current agency setup"

# Inspect daemon-backed agents
bun run animaos agents list
```

## Build

Run `bun run build:cli-sdk` to build the CLI and its SDK dependency, or `bun x nx build @animaOS-SWARM/cli` to build only this package.

## Run

Run `bun run animaos --help` or `bun run animaos launch "your task"`.

```bash
# Single-shot plain-text launch
bun run animaos launch --no-tui "your task"

# Interactive plain-text launch
bun run animaos launch --no-tui
```

## Test

Run `bun x nx test @animaOS-SWARM/cli`.
