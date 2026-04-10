# @animaOS-SWARM/tools

TypeScript tool execution and policy utilities for animaOS-SWARM.

This package exports the tool registry, executor, hook system, permission checks, secret handling, validation, truncation, and shell helpers used by local workflows and test harnesses.

The Rust daemon owns canonical production tool execution, but this package still carries the workspace's shared tool contracts and local utilities.

Current tools coverage includes:

- the root tool registry and schema maps
- guarded execution through `executeTool`
- permission checks, secret substitution, and output redaction helpers
- truncation, shell-launch, and edit-hint utilities used by local workflows

## Quick Example

```ts
import { executeTool } from '@animaOS-SWARM/tools';

const result = await executeTool({
  tool_call_id: 'call-1',
  tool_name: 'todo_write',
  args: {
    todos: [
      {
        content: 'Validate tool output',
        status: 'in_progress',
        activeForm: 'Validating tool output',
      },
    ],
  },
});

console.log(result.status);
```

## Build

Run `bun x nx build @animaOS-SWARM/tools`.

## Test

Run `bun x nx test @animaOS-SWARM/tools`.
