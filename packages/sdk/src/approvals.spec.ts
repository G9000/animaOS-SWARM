import { describe, expect, it } from 'vitest';

import {
  createDaemonClient,
  DaemonHttpError,
  DEFAULT_APPROVAL_POLICY,
  MAX_APPROVAL_NOTE_CHARS,
  POLICY_CLASSES,
  type Approval,
} from './index.js';

function transport(respond: (url: string, init?: RequestInit) => Response) {
  const requests: { url: string; init?: RequestInit }[] = [];
  const client = createDaemonClient({
    baseUrl: '',
    fetch: async (url, init) => {
      requests.push({ url: String(url), init });
      return respond(String(url), init);
    },
  });
  return { approvals: client.approvals, requests };
}

const approval: Approval = {
  id: 'apr_1',
  agentId: 'agent/a',
  sessionId: 'chat:1',
  runId: 'run_1',
  toolCallId: 'call-1',
  tool: 'bash',
  class: 'exec',
  arguments: '{"command":"git status"}',
  argumentsTruncated: false,
  suggestedMatcher: { kind: 'command_prefix', value: 'git status' },
  matcherKinds: ['command_prefix', 'any'],
  createdAtMs: 10,
  expiresAtMs: 1_800_010,
  status: 'pending',
  revision: 1,
  resolution: null,
};

describe('approvals client', () => {
  it('lists pending and decided approvals', async () => {
    const { approvals, requests } = transport(() =>
      Response.json({ approvals: [approval], nextCursor: '10:apr_1' }),
    );

    const pending = await approvals.list({
      status: 'pending',
      agentId: 'agent/a',
    });
    const decided = await approvals.list({
      status: 'decided',
      cursor: '20:apr_2',
      limit: 10,
    });

    expect(pending.approvals).toEqual([approval]);
    expect(decided.nextCursor).toBe('10:apr_1');
    expect(requests.map(({ url }) => url)).toEqual([
      '/api/approvals?status=pending&agentId=agent%2Fa',
      '/api/approvals?status=decided&cursor=20%3Aapr_2&limit=10',
    ]);
  });

  it('sends a decision with its revision and returns the decided approval', async () => {
    const decided = { ...approval, status: 'allowed', revision: 2 };
    const { approvals, requests } = transport(() =>
      Response.json({ approval: decided }),
    );

    const result = await approvals.decide('apr/1', {
      decision: 'allow_always',
      note: 'fine',
      matcher: { kind: 'command_prefix', value: 'git' },
      revision: 1,
    });

    expect(result).toEqual(decided);
    const [request] = requests;
    expect(request.url).toBe('/api/approvals/apr%2F1/decision');
    expect(request.init?.method).toBe('POST');
    expect(JSON.parse(String(request.init?.body))).toEqual({
      decision: 'allow_always',
      note: 'fine',
      matcher: { kind: 'command_prefix', value: 'git' },
      revision: 1,
    });
  });

  it('sends only the fields the daemon accepts', async () => {
    const { approvals, requests } = transport(() =>
      Response.json({ approval }),
    );
    const formState = {
      decision: 'deny' as const,
      revision: 1,
      open: true,
      matcher: { kind: 'any' as const, value: '', label: 'Any' },
    };

    await approvals.decide('apr_1', formState);
    await approvals.decide('apr_1', { decision: 'deny', revision: 1 });

    expect(JSON.parse(String(requests[0].init?.body))).toEqual({
      decision: 'deny',
      matcher: { kind: 'any', value: '' },
      revision: 1,
    });
    expect(JSON.parse(String(requests[1].init?.body))).toEqual({
      decision: 'deny',
      revision: 1,
    });
  });

  it('reads and replaces the policy', async () => {
    const strict = { ...DEFAULT_APPROVAL_POLICY, write: 'ask' as const };
    const { approvals, requests } = transport((_url, init) =>
      Response.json({
        policy: init?.method === 'PUT' ? strict : DEFAULT_APPROVAL_POLICY,
      }),
    );

    expect(await approvals.policy('agent/a')).toEqual({
      write: 'allow',
      exec: 'ask',
      network: 'allow',
      delegate: 'allow',
    });
    expect(await approvals.setPolicy('agent/a', strict)).toEqual(strict);
    expect(
      requests.map(({ url, init }) => [init?.method ?? 'GET', url]),
    ).toEqual([
      ['GET', '/api/agents/agent%2Fa/approval-policy'],
      ['PUT', '/api/agents/agent%2Fa/approval-policy'],
    ]);
    expect(JSON.parse(String(requests[1].init?.body))).toEqual(strict);
    expect(POLICY_CLASSES).toEqual(['write', 'exec', 'network', 'delegate']);
    expect(MAX_APPROVAL_NOTE_CHARS).toBe(1_000);
  });

  it('lists, adds, and removes rules', async () => {
    const rule = {
      id: 'rule_1',
      agentId: 'agent/a',
      tool: 'bash',
      matcher: { kind: 'command_prefix', value: 'git status' },
      createdAtMs: 5,
      fromApprovalId: null,
    };
    const { approvals, requests } = transport((_url, init) =>
      init?.method === 'DELETE'
        ? Response.json({ deleted: true })
        : init?.method === 'POST'
          ? Response.json({ rule }, { status: 201 })
          : Response.json({
              rules: [rule],
              tools: [
                {
                  name: 'bash',
                  class: 'exec',
                  matcherKinds: ['command_prefix', 'any'],
                },
              ],
            }),
    );

    const listed = await approvals.rules('agent/a');
    expect(listed.rules).toEqual([rule]);
    expect(listed.tools[0].class).toBe('exec');
    expect(
      await approvals.addRule('agent/a', {
        tool: 'bash',
        matcher: { kind: 'command_prefix', value: 'git status' },
      }),
    ).toEqual(rule);
    await approvals.removeRule('agent/a', 'rule/1');
    expect(
      requests.map(({ url, init }) => [init?.method ?? 'GET', url]),
    ).toEqual([
      ['GET', '/api/agents/agent%2Fa/approval-rules'],
      ['POST', '/api/agents/agent%2Fa/approval-rules'],
      ['DELETE', '/api/agents/agent%2Fa/approval-rules/rule%2F1'],
    ]);
  });

  it('surfaces a decision that lost a race as a daemon error', async () => {
    const { approvals } = transport(() =>
      Response.json(
        { error: 'This approval was already resolved' },
        { status: 409 },
      ),
    );

    const failure = approvals.decide('apr_1', {
      decision: 'deny',
      revision: 1,
    });
    await expect(failure).rejects.toBeInstanceOf(DaemonHttpError);
    await expect(failure).rejects.toMatchObject({
      status: 409,
      message: 'This approval was already resolved',
    });
  });
});
