import type { AgentSettings, TaskResult } from '@animaOS-SWARM/core';
import type { DaemonTaskResult } from '@animaOS-SWARM/sdk';
import {
  PROVIDER_HELP_TEXT,
  resolveProviderConfig,
} from '../provider-config.js';

export function getErrorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function toRuntimeTaskResult<T>(
  result: DaemonTaskResult<T>
): TaskResult<T> {
  return {
    status: result.status,
    ...(result.data == null ? {} : { data: result.data }),
    ...(result.error == null ? {} : { error: result.error }),
    durationMs: result.durationMs,
  };
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

export const DAEMON_PROVIDER_HELP = PROVIDER_HELP_TEXT;

export function resolveDaemonModelSettings(
  provider: string | undefined,
  apiKey?: string
): AgentSettings | undefined {
  const resolved = resolveProviderConfig(provider, apiKey);
  if (!resolved) return undefined;

  const settings: AgentSettings = {};

  if (resolved.apiKey) {
    settings.apiKey = resolved.apiKey;
  }
  if (resolved.baseUrl) {
    settings.baseUrl = resolved.baseUrl;
  }

  return Object.keys(settings).length > 0 ? settings : undefined;
}
