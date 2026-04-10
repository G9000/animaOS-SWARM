import type { IncomingMessage, ServerResponse } from 'node:http';
import type { AgentConfig } from '@animaOS-SWARM/core';
import type { SwarmConfig, SwarmStrategy } from '@animaOS-SWARM/swarm';
import type { AppState } from '../state.js';

const SWARM_STRATEGIES = new Set<SwarmStrategy>([
  'supervisor',
  'dynamic',
  'round-robin',
]);

export type RouteHandler = (
  req: IncomingMessage,
  res: ServerResponse,
  state: AppState,
  body: Record<string, unknown>,
  params: Record<string, string>
) => Promise<void>;

export interface Route {
  method: string;
  pattern: RegExp;
  paramNames: string[];
  handler: RouteHandler;
}

export function route(
  method: string,
  path: string,
  handler: RouteHandler
): Route {
  const paramNames: string[] = [];
  const pattern = path.replace(/:(\w+)/g, (_match, name) => {
    paramNames.push(name);
    return '([^/]+)';
  });
  return { method, pattern: new RegExp(`^${pattern}$`), paramNames, handler };
}

export function json(res: ServerResponse, status: number, data: unknown) {
  res.writeHead(status, {
    'Content-Type': 'application/json',
    'Access-Control-Allow-Origin': '*',
  });
  res.end(JSON.stringify(data));
}

function isAgentConfigLike(value: unknown): value is AgentConfig {
  if (!value || typeof value !== 'object') {
    return false;
  }

  const candidate = value as Record<string, unknown>;

  return (
    typeof candidate.name === 'string' &&
    candidate.name.length > 0 &&
    typeof candidate.model === 'string' &&
    candidate.model.length > 0
  );
}

export function isAgentConfigBody(body: unknown): body is AgentConfig {
  return isAgentConfigLike(body);
}

export function isSwarmConfigBody(body: unknown): body is SwarmConfig {
  if (!body || typeof body !== 'object') {
    return false;
  }

  const candidate = body as Record<string, unknown>;

  if (!SWARM_STRATEGIES.has(candidate.strategy as SwarmStrategy)) {
    return false;
  }

  if (!isAgentConfigLike(candidate.manager)) {
    return false;
  }

  if (!Array.isArray(candidate.workers)) {
    return false;
  }

  return candidate.workers.every((worker) => isAgentConfigLike(worker));
}

export function getTaskText(body: Record<string, unknown>): string | undefined {
  if (typeof body.task !== 'string') {
    return undefined;
  }

  return body.task.trim().length > 0 ? body.task : undefined;
}
