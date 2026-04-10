import {
  createServer as createHttpServer,
  type IncomingMessage,
  type ServerResponse,
} from 'node:http';
import { agentRoutes } from './routes/agents.js';
import { swarmRoutes } from './routes/swarms.js';
import { searchRoutes } from './routes/search.js';
import { healthRoutes } from './routes/health.js';
import { json, type Route } from './routes/helpers.js';
import { AppState } from './state.js';
import { attachWebSocketServer } from './ws.js';

function parseBody(req: IncomingMessage): Promise<Record<string, unknown>> {
  return new Promise((resolve, reject) => {
    let body = '';
    req.on('data', (chunk: Buffer) => {
      body += chunk.toString();
    });
    req.on('end', () => {
      try {
        resolve(body ? JSON.parse(body) : {});
      } catch {
        reject(new Error('Invalid JSON'));
      }
    });
    req.on('error', reject);
  });
}

function cors(res: ServerResponse) {
  res.setHeader('Access-Control-Allow-Origin', '*');
  res.setHeader(
    'Access-Control-Allow-Methods',
    'GET, POST, PUT, DELETE, OPTIONS'
  );
  res.setHeader('Access-Control-Allow-Headers', 'Content-Type, Authorization');
}

function matchRoute(
  routes: Route[],
  method: string,
  url: string
): { route: Route; params: Record<string, string> } | null {
  for (const r of routes) {
    if (r.method !== method) continue;
    const match = url.match(r.pattern);
    if (match) {
      const params: Record<string, string> = {};
      r.paramNames.forEach((name, i) => {
        params[name] = match[i + 1];
      });
      return { route: r, params };
    }
  }
  return null;
}

export function createServer() {
  const state = new AppState();
  const routes: Route[] = [
    ...healthRoutes,
    ...agentRoutes,
    ...swarmRoutes,
    ...searchRoutes,
  ];

  const server = createHttpServer(async (req, res) => {
    cors(res);

    if (req.method === 'OPTIONS') {
      res.writeHead(204);
      res.end();
      return;
    }

    const url = (req.url ?? '/').split('?')[0];

    const matched = matchRoute(routes, req.method ?? 'GET', url);
    if (!matched) {
      json(res, 404, { error: 'Not found' });
      return;
    }

    try {
      const body =
        req.method === 'POST' || req.method === 'PUT'
          ? await parseBody(req)
          : {};
      await matched.route.handler(req, res, state, body, matched.params);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      json(res, 500, { error: message });
    }
  });

  attachWebSocketServer(server, state);
  return server;
}

export { json };
