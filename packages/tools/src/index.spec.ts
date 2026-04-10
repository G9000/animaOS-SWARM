import { afterEach, describe, expect, it } from 'vitest';
import {
  TOOL_ACTION_MAP,
  executeTodoRead,
  executeTodoWrite,
  resetTodos,
  truncateOutput,
} from './index.js';

afterEach(() => {
  resetTodos();
});

describe('tools package root exports', () => {
  it('supports a minimal consumer flow from the package entrypoint', () => {
    const writeResult = executeTodoWrite({
      todos: [
        {
          content: 'Ship maturity parity',
          status: 'in_progress',
          activeForm: 'Shipping maturity parity',
        },
      ],
    });

    expect(writeResult.status).toBe('success');
    expect(executeTodoRead().result).toContain('Ship maturity parity');
    expect(TOOL_ACTION_MAP.get('todo_write')?.name).toBe('todo_write');

    const truncated = truncateOutput('x'.repeat(40_000), {
      maxChars: 100,
      overflow: false,
      toolName: 'test',
    });

    expect(truncated.truncated).toBe(true);
    expect(truncated.content).toContain('[Truncated:');
  });
});
