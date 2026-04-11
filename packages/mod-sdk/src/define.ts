import type { ModToolHandler } from './types.js';

/**
 * Define a mod tool. Pass a full ModToolHandler spec; get back the same object
 * with TypeScript type inference applied. Analogous to `vscode.commands.registerCommand`.
 */
export function defineModTool(spec: ModToolHandler): ModToolHandler {
  return spec;
}
