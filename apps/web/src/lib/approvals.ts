import {
  DaemonHttpError,
  type Approval,
  type ApprovalDecision,
  type ApprovalDecisionInput,
  type ApprovalMatcher,
  type ApprovalMatcherKind,
  type RiskClass,
  type Session,
} from '@animaOS-SWARM/sdk';

import { daemon } from './daemon-api';
import type { LiveState } from './session-events';

/** The daemon's bound on a matcher value (`MAX_MATCHER_VALUE_CHARS`). */
export const MAX_MATCHER_VALUE_CHARS = 512;

/** What the owner sees when a request never reached the daemon. */
export const COMPANION_UNREACHABLE =
  'Could not reach your companion. Try again.';

/** Sends the owner's decision with the approval's own revision; answers
 *  null when it went through, otherwise the message to show. */
export type ApprovalDecide = (
  approval: Approval,
  input: Omit<ApprovalDecisionInput, 'revision'>,
) => Promise<string | null>;

export const CLASS_LABELS: Record<RiskClass, string> = {
  read: 'Reads',
  write: 'Changes files and records',
  exec: 'Runs commands',
  network: 'Uses the internet',
  delegate: 'Asks other agents',
};

export const DECISION_LABELS: Record<ApprovalDecision, string> = {
  allow_once: 'Allow once',
  allow_session: 'Allow for this session',
  allow_always: 'Always allow',
  deny: 'Deny',
};

export const MATCHER_KIND_LABELS: Record<ApprovalMatcherKind, string> = {
  command_prefix: 'Command starts with',
  path_glob: 'File path matches',
  domain: 'Website domain is',
  any: 'Any call of this tool',
};

/** What a rule or allowance covers, in words. */
export function describeMatcher(
  tool: string,
  matcher: ApprovalMatcher,
): string {
  // The model chose these values: show hidden characters as escapes.
  const value = revealHiddenCharacters(matcher.value);
  tool = revealHiddenCharacters(tool);
  switch (matcher.kind) {
    case 'command_prefix':
      return `${tool} commands starting with “${value}”`;
    case 'path_glob':
      return `${tool} on files matching “${value}”`;
    case 'domain':
      return `${tool} on ${value} and its subdomains`;
    case 'any':
      return `every ${tool} call`;
  }
}

/** Commands that can run or fetch almost anything: a rule starting with
 *  one of them is no real limit. `python` also covers `python3.12`. */
const BROAD_COMMANDS: ReadonlySet<string> = new Set([
  'curl',
  'wget',
  'node',
  'bun',
  'deno',
  'ruby',
  'perl',
  'php',
  'sh',
  'bash',
  'zsh',
  'env',
  'sudo',
  'xargs',
  'npx',
  'bunx',
  'npm',
  'pnpm',
  'yarn',
  'make',
  'git',
  'cargo',
  'go',
  'docker',
  'ssh',
]);

/** True when a rule or allowance on this matcher would let the companion
 *  run almost anything, including changing its own approval settings. */
export function isBroadExecMatcher(
  riskClass: RiskClass,
  matcher: ApprovalMatcher,
): boolean {
  if (riskClass !== 'exec') return false;
  if (matcher.kind === 'any') return true;
  if (matcher.kind !== 'command_prefix') return false;
  const [word = ''] = matcher.value.trim().split(/\s+/);
  // Like the daemon's `is_wrapper`: `/usr/bin/python3`, `bash.exe` and
  // `Python` count as the command they name.
  const base = word.split(/[/\\]/).pop() ?? word;
  const first = base.toLowerCase().replace(/\.exe$/, '');
  return BROAD_COMMANDS.has(first) || /^python[\d.]*$/.test(first);
}

const HIDDEN_CHARACTERS = /[\u200B-\u200F\u202A-\u202E\u2066-\u2069\uFEFF]/g;

/** Writes zero-width and direction-changing characters out as `\u{…}`, so
 *  text cannot look different from what it is. */
export function revealHiddenCharacters(text: string): string {
  return text.replace(
    HIDDEN_CHARACTERS,
    (character) =>
      `\\u{${(character.codePointAt(0) ?? 0).toString(16).padStart(4, '0')}}`,
  );
}

export const decideApproval: ApprovalDecide = async (approval, input) => {
  try {
    await daemon.decideApproval(approval.id, {
      ...input,
      revision: approval.revision,
    });
    return null;
  } catch (error) {
    if (error instanceof DaemonHttpError)
      return error.status === 404
        ? 'This approval is no longer waiting.'
        : error.message;
    return COMPANION_UNREACHABLE;
  }
};

/** How an approval ended, in a few words. */
export function approvalOutcome(approval: Approval): string {
  const resolution = approval.resolution;
  switch (approval.status) {
    case 'pending':
      return 'Waiting';
    case 'stopped':
      return 'Stopped with its run';
    case 'expired':
      return 'Expired at a restart';
    case 'denied':
      return resolution?.resolvedBy === 'timeout' ? 'Timed out' : 'Denied';
    case 'allowed':
      return resolution?.decision === 'allow_session'
        ? 'Allowed for the session'
        : resolution?.decision === 'allow_always'
          ? 'Always allowed'
          : 'Allowed once';
  }
}

/** When it was settled, or asked while it is still pending. */
export function resolvedAt(approval: Approval): number {
  return approval.resolution?.resolvedAtMs ?? approval.createdAtMs;
}

export function formatWhen(ms: number): string {
  return new Date(ms).toLocaleString([], {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}

/** Approvals waiting for the owner: the stream's while it is open (every
 *  pending one of the companion and its helpers), else the listed
 *  sessions' counts. */
export function pendingApprovalCount(
  approvals: LiveState['approvals'],
  streamOpen: boolean,
  sessions: readonly Session[],
): number {
  if (streamOpen) return Object.keys(approvals).length;
  return sessions.reduce(
    (total, session) => total + session.pendingApprovals,
    0,
  );
}
