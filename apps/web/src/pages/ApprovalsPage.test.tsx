import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  DaemonHttpError,
  DEFAULT_APPROVAL_POLICY,
  type Approval,
  type ApprovalRule,
} from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';
import { EMPTY_LIVE_STATE, type LiveState } from '../lib/session-events';
import { approvalFixture } from '../test/live';
import { sessionFixture } from '../test/sessions';
import { ApprovalsPage } from './ApprovalsPage';

const gitRule: ApprovalRule = {
  id: 'rule_1',
  agentId: 'agent-main',
  tool: 'bash',
  matcher: { kind: 'command_prefix', value: 'git status' },
  createdAtMs: 1,
  fromApprovalId: null,
};

const tools = [
  {
    name: 'bash',
    class: 'exec' as const,
    matcherKinds: ['command_prefix' as const, 'any' as const],
  },
  {
    name: 'memory_add',
    class: 'write' as const,
    matcherKinds: ['any' as const],
  },
];

function streamWith(...approvals: Approval[]): LiveState {
  return {
    ...EMPTY_LIVE_STATE,
    approvals: Object.fromEntries(
      approvals.map((approval) => [approval.id, approval]),
    ),
  };
}

function renderPage(props: Partial<Parameters<typeof ApprovalsPage>[0]> = {}) {
  const onOpenSession = vi.fn();
  render(
    <ApprovalsPage
      agentId="agent-main"
      live={EMPTY_LIVE_STATE}
      streamOpen
      sessions={[sessionFixture('chat:1', { title: 'Weekend plans' })]}
      onOpenSession={onOpenSession}
      {...props}
    />,
  );
  return { onOpenSession };
}

beforeEach(() => {
  vi.spyOn(daemon, 'listApprovals').mockResolvedValue({
    approvals: [],
    nextCursor: null,
  });
  vi.spyOn(daemon, 'approvalPolicy').mockResolvedValue(DEFAULT_APPROVAL_POLICY);
  vi.spyOn(daemon, 'approvalRules').mockResolvedValue({
    rules: [gitRule],
    tools,
  });
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('ApprovalsPage', () => {
  it('shows the stream’s pending approvals and decides them', async () => {
    const user = userEvent.setup();
    const approval = approvalFixture('apr_1');
    const decide = vi
      .spyOn(daemon, 'decideApproval')
      .mockResolvedValue({ ...approval, status: 'allowed', revision: 2 });
    const { onOpenSession } = renderPage({ live: streamWith(approval) });
    // The page's reads settle first, so no update lands outside act().
    expect(
      await screen.findByText(
        'Always allow bash commands starting with “git status”',
      ),
    ).toBeVisible();

    const waiting = screen.getByRole('region', { name: 'Waiting for you' });
    const card = within(waiting).getByRole('region', {
      name: 'Approval needed: bash',
    });
    await user.click(
      within(card).getByRole('button', { name: 'Open Weekend plans' }),
    );
    expect(onOpenSession).toHaveBeenCalledWith(approval);
    await user.click(within(card).getByRole('button', { name: 'Allow once' }));
    expect(decide).toHaveBeenCalledWith('apr_1', {
      decision: 'allow_once',
      revision: 1,
    });
    expect(
      await within(card).findByText('Allowed once. Continuing…'),
    ).toBeVisible();
    expect(
      vi
        .mocked(daemon.listApprovals)
        .mock.calls.some(([options]) => options.status === 'pending'),
    ).toBe(false);
  });

  it('reads pending approvals when the stream is closed, and again after a decision', async () => {
    const user = userEvent.setup();
    const approval = approvalFixture('apr_1');
    vi.mocked(daemon.listApprovals).mockImplementation(async (options) => ({
      approvals: options.status === 'pending' ? [approval] : [],
      nextCursor: null,
    }));
    vi.spyOn(daemon, 'decideApproval').mockResolvedValue({
      ...approval,
      status: 'denied',
      revision: 2,
    });
    renderPage({ streamOpen: false });

    const card = await screen.findByRole('region', {
      name: 'Approval needed: bash',
    });
    const pendingReads = () =>
      vi
        .mocked(daemon.listApprovals)
        .mock.calls.filter(([options]) => options.status === 'pending').length;
    expect(pendingReads()).toBe(1);
    await user.click(within(card).getByRole('button', { name: 'Deny' }));
    await waitFor(() => expect(pendingReads()).toBe(2));
  });

  it('refreshes after a decision that failed, when the stream is closed', async () => {
    const user = userEvent.setup();
    const approval = approvalFixture('apr_1');
    vi.mocked(daemon.listApprovals).mockImplementation(async (options) => ({
      approvals: options.status === 'pending' ? [approval] : [],
      nextCursor: null,
    }));
    vi.spyOn(daemon, 'decideApproval').mockRejectedValue(
      new DaemonHttpError(409, { error: 'already decided' }),
    );
    renderPage({ streamOpen: false });

    const card = await screen.findByRole('region', {
      name: 'Approval needed: bash',
    });
    const pendingReads = () =>
      vi
        .mocked(daemon.listApprovals)
        .mock.calls.filter(([options]) => options.status === 'pending').length;
    await user.click(within(card).getByRole('button', { name: 'Deny' }));
    expect(await within(card).findByRole('alert')).toHaveTextContent(
      'already decided',
    );
    await waitFor(() => expect(pendingReads()).toBe(2));
  });

  it('offers no rule for another agent’s approval', async () => {
    const helper = approvalFixture('apr_2', { agentId: 'helper-7' });
    const mine = approvalFixture('apr_1');
    renderPage({ live: streamWith(mine, helper) });
    await screen.findByText(
      'Always allow bash commands starting with “git status”',
    );

    const cards = screen.getAllByRole('region', {
      name: 'Approval needed: bash',
    });
    expect(cards).toHaveLength(2);
    const withNote = cards.filter((card) =>
      within(card).queryByText(
        'Rules for other agents are not managed here yet.',
      ),
    );
    expect(withNote).toHaveLength(1);
    const [other] = withNote;
    const own = cards.find((card) => card !== other);
    if (!own) throw new Error('expected the companion’s own card');
    expect(
      within(other).queryByRole('button', { name: 'Always allow' }),
    ).not.toBeInTheDocument();
    expect(
      within(own).getByRole('button', { name: 'Always allow' }),
    ).toBeVisible();
  });

  it('lists decided approvals newest first and loads older ones', async () => {
    const user = userEvent.setup();
    const resolution = (
      overrides: Partial<Approval['resolution'] & object>,
    ) => ({
      decision: null,
      note: null,
      matcher: null,
      ruleId: null,
      resolvedBy: 'owner' as const,
      resolvedAtMs: 10,
      ...overrides,
    });
    const always = approvalFixture('apr_new', {
      status: 'allowed',
      resolution: resolution({ decision: 'allow_always' }),
    });
    const timedOut = approvalFixture('apr_old', {
      tool: 'web_fetch',
      status: 'denied',
      resolution: resolution({
        decision: 'deny',
        resolvedBy: 'timeout',
        note: '<b>Approval timed out</b>',
      }),
    });
    vi.mocked(daemon.listApprovals).mockImplementation(async (options) =>
      options.cursor
        ? { approvals: [timedOut], nextCursor: null }
        : { approvals: [always], nextCursor: '10:apr_new' },
    );
    renderPage();

    const decided = screen.getByRole('region', {
      name: 'Decided in the last 30 days',
    });
    expect(await within(decided).findByText('Always allowed')).toBeVisible();
    await user.click(
      within(decided).getByRole('button', { name: 'Show older' }),
    );
    expect(await within(decided).findByText('Timed out')).toBeVisible();
    expect(
      within(decided).getByText('“<b>Approval timed out</b>”'),
    ).toBeVisible();
    expect(
      within(decided).queryByRole('button', { name: 'Show older' }),
    ).not.toBeInTheDocument();
    expect(daemon.listApprovals).toHaveBeenCalledWith({
      status: 'decided',
      agentId: 'agent-main',
      cursor: '10:apr_new',
      signal: expect.any(AbortSignal),
    });
  });

  it('sets each class’s policy and manages its rules', async () => {
    const user = userEvent.setup();
    const setPolicy = vi
      .spyOn(daemon, 'setApprovalPolicy')
      .mockImplementation(async (_agentId, policy) => policy);
    const remove = vi.spyOn(daemon, 'removeApprovalRule').mockResolvedValue();
    const added: ApprovalRule = {
      ...gitRule,
      id: 'rule_2',
      matcher: { kind: 'command_prefix', value: 'npm test' },
    };
    const add = vi.spyOn(daemon, 'addApprovalRule').mockResolvedValue(added);
    renderPage();

    const exec = await screen.findByRole('region', { name: 'Runs commands' });
    const policy = within(exec).getByRole('combobox', {
      name: 'Policy for Runs commands',
    });
    await waitFor(() => expect(policy).toBeEnabled());
    await user.selectOptions(policy, 'Deny');
    expect(setPolicy).toHaveBeenCalledWith('agent-main', {
      ...DEFAULT_APPROVAL_POLICY,
      exec: 'deny',
    });

    expect(
      within(exec).getByText(
        'Always allow bash commands starting with “git status”',
      ),
    ).toBeVisible();
    const form = within(exec).getByRole('form', {
      name: 'Add a rule for Runs commands',
    });
    await user.type(within(form).getByLabelText('Match value'), 'npm test');
    await user.click(within(form).getByRole('button', { name: 'Add rule' }));
    expect(add).toHaveBeenCalledWith('agent-main', {
      tool: 'bash',
      matcher: { kind: 'command_prefix', value: 'npm test' },
    });
    expect(
      await within(exec).findByText(
        'Always allow bash commands starting with “npm test”',
      ),
    ).toBeVisible();

    await user.click(
      within(exec).getByRole('button', {
        name: 'Remove rule: bash commands starting with “git status”',
      }),
    );
    expect(remove).toHaveBeenCalledWith('agent-main', 'rule_1');
    await waitFor(() =>
      expect(
        within(exec).queryByText(
          'Always allow bash commands starting with “git status”',
        ),
      ).not.toBeInTheDocument(),
    );
    const write = screen.getByRole('region', {
      name: 'Changes files and records',
    });
    expect(within(write).getByText('No rules yet.')).toBeVisible();
  });

  it('shows why a change did not go through', async () => {
    const user = userEvent.setup();
    vi.spyOn(daemon, 'setApprovalPolicy').mockRejectedValue(
      new DaemonHttpError(503, { error: 'control plane save failed' }),
    );
    renderPage();

    const policy = await screen.findByRole('combobox', {
      name: 'Policy for Uses the internet',
    });
    await waitFor(() => expect(policy).toBeEnabled());
    await user.selectOptions(policy, 'Ask me first');

    expect(await screen.findByRole('alert')).toHaveTextContent(
      'control plane save failed',
    );
  });

  it('warns when the rule being added is as broad as running anything', async () => {
    const user = userEvent.setup();
    renderPage();

    const form = await screen.findByRole('form', {
      name: 'Add a rule for Runs commands',
    });
    const warning = /A rule this broad lets your companion run almost anything/;
    await user.type(within(form).getByLabelText('Match value'), 'npm test');
    expect(within(form).getByText(warning)).toBeVisible();
    await user.clear(within(form).getByLabelText('Match value'));
    await user.type(within(form).getByLabelText('Match value'), 'ls -la');
    expect(within(form).queryByText(warning)).not.toBeInTheDocument();
    await user.selectOptions(
      within(form).getByLabelText('Match by'),
      'Any call of this tool',
    );
    expect(within(form).getByText(warning)).toBeVisible();
  });
});
