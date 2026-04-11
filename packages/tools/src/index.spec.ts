import { afterEach, describe, expect, it } from 'vitest';
import {
  TOOL_ACTION_MAP,
  MOD_TOOL_MAP,
  executeTodoRead,
  executeTodoWrite,
  executeTool,
  registerModTool,
  resetTodos,
  truncateOutput,
} from './index.js';

afterEach(() => {
  resetTodos();
  MOD_TOOL_MAP.clear();
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

describe('executor mod tool dispatch', () => {
  it('dispatches to a registered mod tool and returns success', async () => {
    registerModTool({
      name: 'mod_hello',
      description: 'A test mod tool',
      parameters: { type: 'object', properties: {} },
      execute: async (_args) => ({ greeting: 'hello from mod' }),
    });

    const result = await executeTool({
      tool_call_id: 'call-1',
      tool_name: 'mod_hello',
      args: {},
    });

    expect(result.tool_call_id).toBe('call-1');
    expect(result.status).toBe('success');
    expect(result.result).toContain('hello from mod');
  });

  it('returns an error when a required mod tool parameter is missing', async () => {
    registerModTool({
      name: 'mod_greet',
      description: 'Greet by name',
      parameters: {
        type: 'object',
        properties: { name: { type: 'string' } },
        required: ['name'],
      },
      execute: async (args) => ({ greeting: `hello ${(args as { name: string }).name}` }),
    });

    const result = await executeTool({
      tool_call_id: 'call-3',
      tool_name: 'mod_greet',
      args: {},
    });

    expect(result.tool_call_id).toBe('call-3');
    expect(result.status).toBe('error');
    expect(result.result).toContain("missing required parameter 'name'");
  });

  it('returns an error result when the tool name is not in TOOL_ACTION_MAP or MOD_TOOL_MAP', async () => {
    const result = await executeTool({
      tool_call_id: 'call-2',
      tool_name: 'totally_unknown_tool',
      args: {},
    });

    expect(result.tool_call_id).toBe('call-2');
    expect(result.status).toBe('error');
    expect(result.result).toBe('Unknown tool: totally_unknown_tool');
  });
});
