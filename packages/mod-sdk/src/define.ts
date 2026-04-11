import type { ModToolHandler } from './types.js';

/**
 * Define a mod tool with full TypeScript type inference.
 * Pass a complete ModToolHandler spec; get back the same object typed correctly.
 *
 * Analogous to `defineConfig` in Vite — a no-op identity function that exists
 * purely to give the TypeScript compiler enough context to infer and validate
 * the spec shape. Zero runtime cost; safe to tree-shake.
 */
export function defineModTool(spec: ModToolHandler): ModToolHandler {
  return spec;
}
