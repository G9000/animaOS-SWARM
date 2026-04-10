import type { Server } from 'node:http';
import { WebSocket, WebSocketServer } from 'ws';
import type { EventType } from '@animaOS-SWARM/core';
import type { AppState } from './state.js';

const EVENT_TYPES: EventType[] = [
  'agent:spawned',
  'agent:started',
  'agent:completed',
  'agent:failed',
  'agent:terminated',
  'agent:message',
  'task:started',
  'task:completed',
  'task:failed',
  'tool:before',
  'tool:after',
  'agent:tokens',
  'swarm:created',
  'swarm:completed',
  'swarm:stopped',
];

function broadcast(clients: Set<WebSocket>, payload: string) {
  for (const client of clients) {
    if (client.readyState !== WebSocket.OPEN) {
      continue;
    }

    try {
      client.send(payload);
    } catch (error) {
      console.error('[ws] Failed to send event payload:', error);
    }
  }
}

export function attachWebSocketServer(server: Server, state: AppState) {
  const webSocketServer = new WebSocketServer({
    server,
    path: '/ws',
  });

  const detachHandlers = EVENT_TYPES.map((type) =>
    state.eventBus.on(type, async (event) => {
      broadcast(webSocketServer.clients, JSON.stringify(event));
    })
  );

  const cleanup = () => {
    for (const detach of detachHandlers) {
      detach();
    }
    webSocketServer.close();
  };

  server.once('close', cleanup);
  return webSocketServer;
}
