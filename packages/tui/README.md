# @animaOS-SWARM/tui

Primary operator surface for animaOS-SWARM.

This package powers the Ink-based terminal experience behind `animaos launch`. It exports the terminal app shell, agent and tool panels, result views, status components, and event-log hooks used to inspect and steer swarm execution without leaving the terminal.

Current TUI coverage includes:

- the `App` shell that powers interactive launch sessions
- reusable panels for agents, history, trace, result, message, and tool inspection
- keyboard-first input and resume history surfaces for saved runs and slash commands
- daemon-aware operator states including preflight warnings, `/health`, reconnect recovery, and command-only paused input while the daemon is down
- small pure helpers and hooks that keep event-log, command-surface, and displayed-agent logic directly testable

## Quick Example

```tsx
import React from 'react';
import { render } from 'ink';
import { EventBus } from '@animaOS-SWARM/core';
import { App } from '@animaOS-SWARM/tui';

const eventBus = new EventBus();

render(
  <App
    eventBus={eventBus}
    strategy="supervisor"
    interactive
    agentProfiles={[{ name: 'manager', role: 'orchestrator' }]}
    onTask={async (task) => ({
      status: 'success',
      data: { text: `Handled: ${task}` },
      durationMs: 0,
    })}
  />
);
```

Near-term focus is making the TUI strong enough for daily use: keyboard-first navigation, trace inspection, inline approvals, memory drill-down, and session resume.

## Build

Run `bun x nx build @animaOS-SWARM/tui`.

## Test

Run `bun x nx test @animaOS-SWARM/tui`.
