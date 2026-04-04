import { route, json } from './helpers.js';
import type { AgentConfig } from '@animaOS-SWARM/core';

export const agentRoutes = [
  // Create agent
  route('POST', '/api/agents', async (_req, res, state, body) => {
    const config = body as AgentConfig;
    if (!config.name || !config.model) {
      json(res, 400, { error: 'name and model are required' });
      return;
    }
    const agent = await state.createAgent(config);
    json(res, 201, { id: agent.agentId, name: config.name, status: 'idle' });
  }),

  // List agents
  route('GET', '/api/agents', async (_req, res, state) => {
    const agents = Array.from(state.agents.values()).map((a) => {
      const s = a.getState();
      return {
        id: s.id,
        name: s.name,
        status: s.status,
        tokenUsage: s.tokenUsage,
      };
    });
    json(res, 200, { agents });
  }),

  // Get agent
  route('GET', '/api/agents/:id', async (_req, res, state, _body, params) => {
    const agent = state.agents.get(params.id);
    if (!agent) {
      json(res, 404, { error: 'Agent not found' });
      return;
    }
    json(res, 200, agent.getState());
  }),

  // Run task on agent
  route(
    'POST',
    '/api/agents/:id/run',
    async (_req, res, state, body, params) => {
      const task = body.task as string;
      if (!task) {
        json(res, 400, { error: 'task is required' });
        return;
      }
      const result = await state.runAgent(params.id, task);
      json(res, 200, result);
    }
  ),

  // Delete agent
  route(
    'DELETE',
    '/api/agents/:id',
    async (_req, res, state, _body, params) => {
      await state.deleteAgent(params.id);
      json(res, 200, { deleted: true });
    }
  ),
];
