import { getTaskText, isSwarmConfigBody, json, route } from './helpers.js';

export const swarmRoutes = [
  // Create swarm
  route('POST', '/api/swarms', async (_req, res, state, body) => {
    if (!isSwarmConfigBody(body)) {
      json(res, 400, { error: 'strategy, manager, and workers are required' });
      return;
    }

    const swarm = await state.createSwarm(body);
    json(res, 201, { id: swarm.id, strategy: body.strategy });
  }),

  // List swarms
  route('GET', '/api/swarms', async (_req, res, state) => {
    const swarms = Array.from(state.swarms.values()).map((s) => s.getState());
    json(res, 200, { swarms });
  }),

  // Get swarm
  route('GET', '/api/swarms/:id', async (_req, res, state, _body, params) => {
    const swarm = state.swarms.get(params.id);
    if (!swarm) {
      json(res, 404, { error: 'Swarm not found' });
      return;
    }
    json(res, 200, swarm.getState());
  }),

  // Run task on swarm
  route(
    'POST',
    '/api/swarms/:id/run',
    async (_req, res, state, body, params) => {
      const swarm = state.swarms.get(params.id);
      if (!swarm) {
        json(res, 404, { error: 'Swarm not found' });
        return;
      }
      const task = getTaskText(body);
      if (!task) {
        json(res, 400, { error: 'task is required' });
        return;
      }
      const result = await swarm.run(task);
      json(res, 200, result);
    }
  ),
];
