import { afterEach, describe, expect, it, vi } from 'vitest';

import {
  createDaemonClient,
  isApprovalEvent,
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

  it('warns about a malformed payload instead of dropping it silently', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const client = createDaemonClient({
      baseUrl: '',
      fetch: async () =>
        sseResponse([
          'id: 1\nevent: step.delta\ndata: {"type":"step.delta",\n\n',
          'id: 2\nevent: run.completed\ndata: {"agentId":"agent/a"}\n\n',
          'id: 3\nevent: stream.snapshot\ndata: {"type":"stream.snapshot","agentId":"agent/a","seq":3,"at":5,"runs":[],"approvals":[]}\n\n',
        ]),
    });

    const received: AgentEvent[] = [];
    for await (const event of client.events.stream('agent/a'))
      received.push(event);

    expect(received.map((event) => event.type)).toEqual(['stream.snapshot']);
    expect(warn).toHaveBeenCalledTimes(2);
    expect(warn.mock.calls.map(([, detail]) => detail)).toEqual([
      expect.objectContaining({ event: 'step.delta', id: '1' }),
      expect.objectContaining({ event: 'run.completed', id: '2' }),
    ]);
  });

  it('types approval events and the approvals a snapshot carries', async () => {
    const approval = {
      id: 'apr_1',
      agentId: 'agent/a',
      sessionId: 'chat:1',
      runId: 'run_1',
      toolCallId: 'call-1',
      tool: 'bash',
      class: 'exec',
      arguments: '{"command":"ls"}',
      argumentsTruncated: false,
      suggestedMatcher: { kind: 'command_prefix', value: 'ls' },
      matcherKinds: ['command_prefix', 'any'],
      createdAtMs: 5,
      expiresAtMs: 1_800_005,
      status: 'pending',
      revision: 1,
      resolution: null,
    };
    const resolved = {
      ...approval,
      status: 'allowed',
      revision: 2,
      resolution: {
        decision: 'allow_once',
        note: null,
        matcher: null,
        ruleId: null,
        resolvedBy: 'owner',
        resolvedAtMs: 9,
      },
    };
    const client = createDaemonClient({
      baseUrl: '',
      fetch: async () =>
        sseResponse([
          `id: 1\nevent: stream.snapshot\ndata: ${JSON.stringify({ type: 'stream.snapshot', agentId: 'agent/a', seq: 1, at: 5, runs: [], approvals: [approval] })}\n\n`,
          `id: 2\nevent: approval.resolved\ndata: ${JSON.stringify({ type: 'approval.resolved', agentId: 'agent/a', sessionId: 'chat:1', runId: 'run_1', seq: 2, at: 9, approval: resolved })}\n\n`,
        ]),
    });

    const received: AgentEvent[] = [];
    for await (const event of client.events.stream('agent/a'))
      received.push(event);

    const [snapshot, decided] = received;
    expect(
      snapshot.type === 'stream.snapshot' && snapshot.approvals[0].id,
    ).toBe('apr_1');
    expect(isApprovalEvent(decided)).toBe(true);
    expect(isApprovalEvent(snapshot)).toBe(false);
    expect(
      decided.type === 'approval.resolved' &&
        decided.approval.resolution?.resolvedBy,
    ).toBe('owner');
  });
});

afterEach(() => {
  vi.restoreAllMocks();
});
