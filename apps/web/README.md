# @animaOS-SWARM/web

Browser playground for the animaOS workspace.

This app is the active browser surface for local daemon-driven workflows. It currently exposes:

- health status for the daemon control plane
- live lists of registered agents and swarms
- an agency playground that turns a plain-language brief into a generated agency workspace
- optional CLI-style seed memory generation when creating that workspace
- one-click spawning of the returned team as a live swarm through the Rust daemon

The playground talks to the daemon REST API, including `/api/health`, `/api/providers`, `/api/agencies/create`, and `/api/swarms`.

## Quick Example

```bash
bun dev --host rust
```

Open the web URL printed by `workspace-dev`, or run the app directly with `bun x nx serve @animaOS-SWARM/web`.

## Build

Run `bun x nx build @animaOS-SWARM/web`.

## Typecheck

Run `bun x nx typecheck @animaOS-SWARM/web`.

## Test

Run `bun x nx test @animaOS-SWARM/web`.
