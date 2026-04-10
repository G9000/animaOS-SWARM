# @animaOS-SWARM/ui-e2e

Cross-browser Playwright smoke coverage for the web app.

This project validates the served browser surface for `@animaOS-SWARM/ui`. It is the consumer-boundary check for the current web app, not a reusable library.

Current e2e coverage includes:

- booting the preview server through Nx before browser tests start
- checking the top-level control-grid heading in the browser
- booting an isolated live server plus Vite dev server pair for websocket browser validation
- proving that externally created agents and swarms appear in the dashboard without a manual refresh
- proving that externally started agent tasks stream progress and land in the selected agent output pane
- proving that externally started swarm tasks stream completion plus structured result payloads into the selected dashboard output pane
- proving that recorded task history can be queried from the browser search workflow after live agent execution
- proving that ingested documents can be queried from the browser search workflow after local knowledge indexing
- proving that multi-chunk documents return the matching indexed excerpt instead of a raw payload fallback
- running the same smoke path across Playwright browser projects

The isolated live harness sets `UI_SUPPRESS_WS_PROXY_RESET=1` so harmless websocket shutdown noise from Playwright browser teardown does not pollute the live-suite output.

## Quick Example

```bash
bun x nx e2e @animaOS-SWARM/ui-e2e
bun x nx run @animaOS-SWARM/ui-e2e:e2e-live
```

## Typecheck

Run `bun x nx typecheck @animaOS-SWARM/ui-e2e`.
