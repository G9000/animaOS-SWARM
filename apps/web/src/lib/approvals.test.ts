import { afterEach, describe, expect, it, vi } from 'vitest';
import { DaemonConnectionError, DaemonHttpError } from '@animaOS-SWARM/sdk';

import { daemon } from './daemon-api';
import {
  COMPANION_UNREACHABLE,
  decideApproval,
  describeMatcher,
  isBroadExecMatcher,
  revealHiddenCharacters,
} from './approvals';
import { approvalFixture } from '../test/live';

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

describe('isBroadExecMatcher', () => {
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
