import { describe, expect, it } from 'vitest';

import {
  createDaemonClient,
  isRunLifecycleEvent,
  type AgentEvent,
} from './index.js';

function sseResponse(chunks: string[]): Response {
  const encoder = new TextEncoder();
  return new Response(
    new ReadableStream({
      start(controller) {
        for (const chunk of chunks) controller.enqueue(encoder.encode(chunk));
        controller.close();
      },
    }),
    { headers: { 'content-type': 'text/event-stream' } },
  );
}

describe('agent events client', () => {
  it('streams typed events and skips keep-alives', async () => {
    const requests: string[] = [];
    const client = createDaemonClient({
      baseUrl: '',
      fetch: async (url) => {
        requests.push(String(url));
        return sseResponse([
          'id: 1\nevent: stream.snapshot\ndata: {"type":"stream.snapshot","agentId":"agent/a","seq":1,"at":5,"runs":[],"approvals":[]}\n\n',
          ': keep-alive\n\n',
          'id: 2\nevent: step.delta\ndata: {"type":"step.delta","agentId":"agent/a","sessionId":"chat:1","runId":"run_1","seq":2,"at":6,"stepId":"run_1:1","offset":0,"text":"Hel"}\n\n',
          'id: 3\nevent: run.completed\ndata: {"type":"run.completed","agentId":"agent/a","runId":"run_1","seq":3,"at":7,"run":{"id":"run_1","status":"completed"}}\n\n',
        ]);
      },
    });

    const received: AgentEvent[] = [];
    for await (const event of client.events.stream('agent/a'))
      received.push(event);

    expect(requests).toEqual(['/api/agents/agent%2Fa/events']);
    expect(received.map((event) => event.type)).toEqual([
      'stream.snapshot',
      'step.delta',
      'run.completed',
    ]);
    const delta = received[1];
    expect(delta.type === 'step.delta' && delta.text).toBe('Hel');
    expect(isRunLifecycleEvent(received[2])).toBe(true);
    expect(isRunLifecycleEvent(received[1])).toBe(false);
  });
});
