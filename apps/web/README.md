# @animaOS-SWARM/ui

Browser app shell for animaOS-SWARM.

This app is the current browser entrypoint for the workspace. It now exposes an operator-facing control grid for agents, swarms, search, and health using the local server REST surface plus the `/ws` live event stream, while still degrading cleanly into preview mode when the server is not running.

Current UI coverage includes:

- the top-level React `App` render path through the Vite entrypoint
- the operator dashboard shell, navigation, and primary section structure
- browser-facing branding through the `ANIMAOS CONTROL GRID` heading
- a directly tested live-event reducer that applies `/ws` updates to dashboard state
- graceful preview-mode rendering even when `/api` is unavailable

## Quick Example

```bash
bun x nx serve @animaOS-SWARM/server
bun x nx serve @animaOS-SWARM/ui
```

Open `http://localhost:4200` in a browser.

## Build

Run `bun x nx build @animaOS-SWARM/ui`.

## Serve

Run `bun x nx serve @animaOS-SWARM/ui`.

## Test

Run `bun x nx test @animaOS-SWARM/ui`.
