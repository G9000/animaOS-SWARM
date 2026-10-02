import { afterEach, describe, expect, it, vi } from 'vitest';
import { DaemonConnectionError, DaemonHttpError } from '@animaOS-SWARM/sdk';

import { daemon } from './daemon-api';
import {
  COMPANION_UNREACHABLE,
  approvalOutcome,
  decideApproval,
  describeMatcher,
  isBroadExecMatcher,
  pendingApprovalCount,
  revealHiddenCharacters,
} from './approvals';
import { approvalFixture } from '../test/live';
import { sessionFixture } from '../test/sessions';

afterEach(() => {
  vi.restoreAllMocks();
});

describe('describeMatcher', () => {
  it('says what a rule or allowance covers', () => {
    expect(
      describeMatcher('bash', { kind: 'command_prefix', value: 'git status' }),
    ).toBe('bash commands starting with “git status”');
    expect(
      describeMatcher('write_file', { kind: 'path_glob', value: 'notes/**' }),
    ).toBe('write_file on files matching “notes/**”');
    expect(
      describeMatcher('web_fetch', { kind: 'domain', value: 'docs.rs' }),
    ).toBe('web_fetch on docs.rs and its subdomains');
    expect(describeMatcher('memory_add', { kind: 'any', value: '' })).toBe(
      'every memory_add call',
    );
  });
});

describe('describeMatcher with hidden characters', () => {
  it('writes them out in the scope line', () => {
    expect(
      describeMatcher('bash', {
        kind: 'command_prefix',
        value: 'rm\u202E gnirts',
      }),
    ).toBe('bash commands starting with “rm\\u{202e} gnirts”');
    expect(
      describeMatcher('web\u200Bfetch', {
        kind: 'domain',
        value: 'a\u200Db.io',
      }),
    ).toBe('web\\u{200b}fetch on a\\u{200d}b.io and its subdomains');
  });
});

describe('isBroadExecMatcher', () => {
  it('reads the first word like the daemon does', () => {
    const exec = (value: string) =>
      isBroadExecMatcher('exec', { kind: 'command_prefix', value });
    expect(exec('/usr/bin/python3 -c')).toBe(true);
    expect(exec('bash.exe -c')).toBe(true);
    expect(exec('Python')).toBe(true);
    expect(exec(String.raw`C:\tools\Node.EXE x`)).toBe(true);
    expect(exec('/usr/bin/ls')).toBe(false);
    expect(exec('pythonic')).toBe(false);
  });

  it('flags an exec rule that lets the companion run almost anything', () => {
    expect(isBroadExecMatcher('exec', { kind: 'any', value: '' })).toBe(true);
    for (const value of ['curl https://x', 'python3 -c 1', 'python3.12', 'sh'])
      expect(
        isBroadExecMatcher('exec', { kind: 'command_prefix', value }),
      ).toBe(true);
    expect(
      isBroadExecMatcher('exec', {
        kind: 'command_prefix',
        value: ' git status',
      }),
    ).toBe(true);
    expect(
      isBroadExecMatcher('exec', { kind: 'command_prefix', value: 'ls -la' }),
    ).toBe(false);
    expect(
      isBroadExecMatcher('exec', {
        kind: 'command_prefix',
        value: 'pythonista',
      }),
    ).toBe(false);
    expect(isBroadExecMatcher('write', { kind: 'any', value: '' })).toBe(false);
    expect(
      isBroadExecMatcher('network', { kind: 'domain', value: 'curl.se' }),
    ).toBe(false);
  });
});

describe('revealHiddenCharacters', () => {
  it('turns invisible and direction-changing characters into visible escapes', () => {
    expect(revealHiddenCharacters('a\u200Bb\u202Ec\u2066d\uFEFFe\u200F')).toBe(
      'a\\u{200b}b\\u{202e}c\\u{2066}d\\u{feff}e\\u{200f}',
    );
    expect(revealHiddenCharacters('plain text \n\t')).toBe('plain text \n\t');
  });
});

describe('decideApproval', () => {
  it('sends the approval’s own revision and answers null once it went through', async () => {
    const approval = approvalFixture('apr_1', { revision: 1 });
    const decide = vi
      .spyOn(daemon, 'decideApproval')
      .mockResolvedValue({ ...approval, status: 'allowed', revision: 2 });

    await expect(
      decideApproval(approval, { decision: 'allow_once' }),
    ).resolves.toBeNull();
    expect(decide).toHaveBeenCalledWith('apr_1', {
      decision: 'allow_once',
      revision: 1,
    });
  });

  it('answers what to show when it did not go through', async () => {
    const approval = approvalFixture('apr_1');
    const decide = vi.spyOn(daemon, 'decideApproval');

    decide.mockRejectedValueOnce(
      new DaemonHttpError(409, { error: 'This approval was already resolved' }),
    );
    await expect(decideApproval(approval, { decision: 'deny' })).resolves.toBe(
      'This approval was already resolved',
    );
    decide.mockRejectedValueOnce(
      new DaemonHttpError(404, { error: 'not found' }),
    );
    await expect(decideApproval(approval, { decision: 'deny' })).resolves.toBe(
      'This approval is no longer waiting.',
    );
    decide.mockRejectedValueOnce(new DaemonConnectionError('', new Error('x')));
    await expect(decideApproval(approval, { decision: 'deny' })).resolves.toBe(
      COMPANION_UNREACHABLE,
    );
    expect(COMPANION_UNREACHABLE).toBe(
      'Could not reach your companion. Try again.',
    );
  });
});

describe('approvalOutcome', () => {
  it('says how each approval ended', () => {
    const resolution = {
      decision: null,
      note: null,
      matcher: null,
      ruleId: null,
      resolvedBy: 'owner' as const,
      resolvedAtMs: 5,
    };
    const ended = (
      status: 'allowed' | 'denied' | 'stopped' | 'expired',
      overrides: Partial<typeof resolution> = {},
    ) =>
      approvalOutcome(
        approvalFixture('apr_1', {
          status,
          resolution: { ...resolution, ...overrides },
        }),
      );

    expect(ended('allowed', { decision: 'allow_once' })).toBe('Allowed once');
    expect(ended('allowed', { decision: 'allow_session' })).toBe(
      'Allowed for the session',
    );
    expect(ended('allowed', { decision: 'allow_always' })).toBe(
      'Always allowed',
    );
    expect(ended('denied', { decision: 'deny' })).toBe('Denied');
    expect(ended('denied', { decision: 'deny', resolvedBy: 'timeout' })).toBe(
      'Timed out',
    );
    expect(ended('stopped', { resolvedBy: 'stop' })).toBe(
      'Stopped with its run',
    );
    expect(ended('expired', { resolvedBy: 'restart' })).toBe(
      'Expired at a restart',
    );
    expect(approvalOutcome(approvalFixture('apr_2'))).toBe('Waiting');
  });
});

describe('pendingApprovalCount', () => {
  it('counts the stream’s approvals while it is open, else the sessions’', () => {
    const approvals = {
      apr_1: approvalFixture('apr_1'),
      apr_2: approvalFixture('apr_2'),
    };
    const sessions = [
      sessionFixture('chat:1', { pendingApprovals: 1 }),
      sessionFixture('chat:2', { pendingApprovals: 3 }),
    ];
    expect(pendingApprovalCount(approvals, true, sessions)).toBe(2);
    expect(pendingApprovalCount(approvals, false, sessions)).toBe(4);
    expect(pendingApprovalCount({}, true, sessions)).toBe(0);
  });
});
