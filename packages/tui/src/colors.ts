import { createContext, useContext } from 'react';

/**
 * Whether color output should be emitted. Honors the `NO_COLOR` convention
 * (https://no-color.org) — any non-empty value disables color.
 *
 * The check is module-scoped so the value is captured once on first import.
 * Tests can override per-render via `<ColorContext.Provider value={...}>`.
 */
const NO_COLOR_ENV =
  typeof process !== 'undefined' &&
  typeof process.env === 'object' &&
  process.env !== null &&
  typeof process.env['NO_COLOR'] === 'string' &&
  process.env['NO_COLOR'].length > 0;

export const COLOR_ENABLED_DEFAULT = !NO_COLOR_ENV;

export const ColorContext = createContext<boolean>(COLOR_ENABLED_DEFAULT);

export function useColorEnabled(): boolean {
  return useContext(ColorContext);
}

/**
 * Wraps a candidate Ink `<Text>` color value. Returns the input when colour
 * output is enabled, `undefined` otherwise — passing `undefined` to Ink's
 * `color` prop is the canonical way to opt out.
 */
export function maybeColor(
  enabled: boolean,
  color: string | undefined
): string | undefined {
  return enabled ? color : undefined;
}
