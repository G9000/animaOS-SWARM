/**
 * A tool contributed by a mod. Intentionally simpler than `Action`:
 * - No access to the agent runtime or current message — mods are stateless.
 * - Returns raw data (`Promise<unknown>`); the executor wraps it in a TaskResult.
 */
export interface ModToolHandler {
  name: string;
  description: string;
  /** JSON Schema object shape, serialised to the LLM for tool calling. */
  parameters: {
    type: 'object';
    properties: Record<string, unknown>;
    required?: string[];
  };
  execute(args: Record<string, unknown>): Promise<unknown>;
}

/**
 * A mod plugin. Intentionally simpler than `Plugin`:
 * - No lifecycle hooks (`init`/`cleanup`) — mods have no server-side state.
 * - `tools` instead of `actions` to distinguish from the built-in action system.
 */
export interface ModPlugin {
  name: string;
  description: string;
  tools: ModToolHandler[];
}
