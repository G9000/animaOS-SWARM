import { route, json } from './helpers.js';
import type { IncomingMessage } from 'node:http';

function getQuery(req: IncomingMessage): Record<string, string> {
  const url = new URL(
    req.url ?? '/',
    `http://${req.headers.host ?? 'localhost'}`
  );
  const params: Record<string, string> = {};
  url.searchParams.forEach((v, k) => {
    params[k] = v;
  });
  return params;
}

export const searchRoutes = [
  // Search task history
  route('GET', '/api/search', async (req, res, state) => {
    const query = getQuery(req);
    const q = query.q;
    if (!q) {
      json(res, 400, { error: 'q query parameter is required' });
      return;
    }
    const limit = Number(query.limit ?? '10');
    const results = state.taskHistory.search(q, limit);
    json(res, 200, { results });
  }),

  // Ingest document
  route('POST', '/api/documents', async (_req, res, state, body) => {
    const id = body.id as string;
    const text = body.text as string;
    if (!id || !text) {
      json(res, 400, { error: 'id and text are required' });
      return;
    }
    const chunks = state.documentStore.ingest(
      id,
      text,
      body.metadata as Record<string, unknown>
    );
    json(res, 201, { id, chunks });
  }),

  // Search documents
  route('GET', '/api/documents/search', async (req, res, state) => {
    const query = getQuery(req);
    const q = query.q;
    if (!q) {
      json(res, 400, { error: 'q query parameter is required' });
      return;
    }
    const limit = Number(query.limit ?? '10');
    const results = state.documentStore.search(q, limit);
    json(res, 200, { results });
  }),
];
