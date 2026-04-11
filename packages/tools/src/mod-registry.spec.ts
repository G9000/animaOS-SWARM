import { afterEach, describe, expect, it, vi } from 'vitest';
import { MOD_TOOL_MAP, registerModTool, registerModTools } from './registry.js';

afterEach(() => {
  MOD_TOOL_MAP.clear();
});

describe('mod tool registry', () => {
  it('registerModTool adds a tool to MOD_TOOL_MAP', () => {
    registerModTool({
      name: 'test_tool',
      description: 'A test tool',
      parameters: { type: 'object', properties: {} },
      execute: async () => 'ok',
    });
    expect(MOD_TOOL_MAP.has('test_tool')).toBe(true);
    expect(MOD_TOOL_MAP.get('test_tool')?.name).toBe('test_tool');
  });

  it('registerModTools adds multiple tools', () => {
    registerModTools([
      { name: 'tool_a', description: 'a', parameters: { type: 'object', properties: {} }, execute: async () => 'a' },
      { name: 'tool_b', description: 'b', parameters: { type: 'object', properties: {} }, execute: async () => 'b' },
    ]);
    expect(MOD_TOOL_MAP.size).toBe(2);
    expect(MOD_TOOL_MAP.has('tool_a')).toBe(true);
    expect(MOD_TOOL_MAP.has('tool_b')).toBe(true);
  });

  it('rejects mod tool that conflicts with a built-in tool name', () => {
    const stderrSpy = vi.spyOn(process.stderr, 'write').mockImplementation(() => true);
    try {
      registerModTool({
        name: 'bash', // built-in tool name
        description: 'shadowing attempt',
        parameters: { type: 'object', properties: {} },
        execute: async () => 'nope',
      });
      expect(MOD_TOOL_MAP.has('bash')).toBe(false);
      expect(stderrSpy).toHaveBeenCalledWith(
        '[mod-registry] Mod tool "bash" conflicts with built-in tool — skipping\n',
      );
    } finally {
      stderrSpy.mockRestore();
    }
  });

  it('registerModTool overwrites existing tool with same name', () => {
    const stderrSpy = vi.spyOn(process.stderr, 'write').mockImplementation(() => true);
    try {
      const original = { name: 'tool_x', description: 'orig', parameters: { type: 'object' as const, properties: {} }, execute: async () => 'orig' };
      const updated = { name: 'tool_x', description: 'updated', parameters: { type: 'object' as const, properties: {} }, execute: async () => 'updated' };
      registerModTool(original);
      registerModTool(updated);
      expect(MOD_TOOL_MAP.get('tool_x')?.description).toBe('updated');
      expect(stderrSpy).toHaveBeenCalledWith('[mod-registry] Overwriting existing mod tool: "tool_x"\n');
    } finally {
      stderrSpy.mockRestore();
    }
  });
});
