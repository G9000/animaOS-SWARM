import type { TaskResult } from '@animaOS-SWARM/core';

export function getErrorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function extractResultText(result: TaskResult): string | null {
  if (
    result.data &&
    typeof result.data === 'object' &&
    'text' in result.data &&
    typeof result.data.text === 'string'
  ) {
    return result.data.text;
  }

  if (typeof result.data === 'string') {
    return result.data;
  }

  return null;
}
