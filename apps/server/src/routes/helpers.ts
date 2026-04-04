import type { IncomingMessage, ServerResponse } from 'node:http';
import type { AppState } from '../state.js';

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
