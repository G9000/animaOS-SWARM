import { isTerminalRunStatus, type Run } from '@animaOS-SWARM/sdk';

import { emptyLiveRun, stepRunId, type LiveRun } from './session-events';
import type { ChatMessage } from './types';

/** Tools whose calls start a helper or delegated run (spec §15.2). */
export const HELPER_TOOLS: ReadonlySet<string> = new Set([
  'spawn_helper',
  'delegate_to_agent',
]);

/** Where a helper card's "Open session" goes. */
export interface HelperTarget {
  agentId: string;
  sessionId: string;
}

/** The helper a `spawn_helper` or `delegate_to_agent` call runs. */
export interface ToolHelper {
  /** The helper's name, or the specialist's id. */
  label: string;
  /** Known from the call (a specialist) or its result (a helper). */
  agentId: string | null;
}

/** One tool call as a card shows it (spec §15.2). */
export interface ToolStep {
  toolCallId: string;
  name: string;
  /** The call's arguments on one short line. */
  argumentsPreview: string;
  status: 'running' | 'success' | 'error';
  durationMs: number | null;
  /** The result, or the error of a failed call; null while it runs. */
  result: string | null;
  truncated: boolean;
  /** The run that made the call, when known. */
  runId: string | null;
  helper: ToolHelper | null;
}

/** A message the daemon has not accepted yet, or a steer not yet applied. */
export interface PendingBubble {
  key: string;
  text: string;
  createdAtMs: number;
  status: 'sending' | 'retrying' | 'steering';
}

export type TranscriptItem =
  | { kind: 'message'; key: string; message: ChatMessage }
  | { kind: 'delegated'; key: string; message: ChatMessage; from: string }
  | { kind: 'revised'; key: string; message: ChatMessage }
  | { kind: 'tools'; key: string; steps: ToolStep[]; messageIds: string[] }
  | { kind: 'run'; key: string; live: LiveRun }
  | { kind: 'outcome'; key: string; run: Run }
  | { kind: 'pending'; key: string; pending: PendingBubble }
  | { kind: 'trimmed'; key: string };

type ToolsItem = Extract<TranscriptItem, { kind: 'tools' }>;

/** What the owner can do from the transcript. */
export interface TranscriptActions {
  onCancelQueued?: (run: Run) => void;
  onSendAgain?: (run: Run) => void;
  onCompact?: () => void;
  helperSession?: (step: ToolStep) => HelperTarget | null;
  onOpenSession?: (target: HelperTarget) => void;
}

export interface TranscriptInput {
  /** The loaded history, oldest first. */
  messages: readonly ChatMessage[];
  /** The session's runs from its stream and ledger, oldest first. */
  runs?: readonly LiveRun[];
  pending?: readonly PendingBubble[];
  /** The newest message outside the companion's view (spec §5.3). */
  trimmedThrough?: string | null;
  /** Set for helper sessions: who wrote their user turns. */
  delegatedBy?: string | null;
}

type Fields = Record<string, unknown>;

function fieldsOf(value: unknown): Fields {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as Fields)
    : {};
}

function stringField(fields: Fields, key: string): string | null {
  const value = fields[key];
  return typeof value === 'string' ? value : null;
}

function numberField(fields: Fields, key: string): number | null {
  const value = fields[key];
  return typeof value === 'number' && Number.isFinite(value) ? value : null;
}

function parsedFields(text: string | null): Fields {
  if (!text) return {};
  try {
    return fieldsOf(JSON.parse(text));
  } catch {
    return {};
  }
}

function metadataOf(message: ChatMessage): Fields {
  return fieldsOf(message.content.metadata);
}

/** The run a committed message belongs to. */
export function messageRunId(message: ChatMessage): string | null {
  const metadata = metadataOf(message);
  const stepId = stringField(metadata, 'stepId');
  return stringField(metadata, 'runId') ?? (stepId ? stepRunId(stepId) : null);
}

function shorten(line: string, limit = 120): string {
  const flat = line.replace(/\s+/g, ' ').trim();
  return flat.length > limit ? `${flat.slice(0, limit - 1)}…` : flat;
}

/** A call's arguments on one short line: `key: value, …`. */
export function argumentsSummary(args: unknown): string {
  const entries = Object.entries(fieldsOf(args));
  if (entries.length === 0) return '';
  return shorten(
    entries
      .map(
        ([key, value]) =>
          `${key}: ${typeof value === 'string' ? value : JSON.stringify(value)}`,
      )
      .join(', '),
  );
}

/** A live card's JSON arguments preview (2 KiB at most, maybe cut). */
export function previewSummary(preview: string): string {
  try {
    return argumentsSummary(JSON.parse(preview));
  } catch {
    return shorten(preview);
  }
}

const DELEGATED_TASK_PREFIX = 'Task delegated by workspace manager ';

/** A delegated task without its routing preamble (the daemon's
 *  `delegated_task_text`). */
export function delegatedTaskText(text: string): string {
  if (!text.startsWith(DELEGATED_TASK_PREFIX)) return text;
  const split = text.indexOf('\n\n');
  return split < 0 ? text : text.slice(split + 2);
}

/** Totals such as "Used 3 tools · 4s". */
export function formatElapsed(ms: number): string {
  return ms < 1_000 ? '<1s' : `${Math.round(ms / 1_000)}s`;
}

function toolHelper(
  name: string,
  args: Fields,
  result: string | null,
): ToolHelper | null {
  if (!HELPER_TOOLS.has(name)) return null;
  const started = stringField(parsedFields(result), 'agentId');
  if (name === 'delegate_to_agent') {
    const agentId = stringField(args, 'agent_id');
    return { label: agentId ?? 'Specialist', agentId: agentId ?? started };
  }
  return { label: stringField(args, 'name') ?? 'Helper', agentId: started };
}

interface StoredCall {
  id: string;
  name: string;
  args: Fields;
}

function storedCalls(metadata: Fields): StoredCall[] {
  const calls = metadata.toolCalls;
  if (!Array.isArray(calls)) return [];
  return calls.flatMap((value): StoredCall[] => {
    const call = fieldsOf(value);
    const id = stringField(call, 'id');
    const name = stringField(call, 'name');
    return id && name ? [{ id, name, args: fieldsOf(call.args) }] : [];
  });
}

/** `"success"` or `"error"` from the daemon's own `toolStatus` marker
 *  (spec §3.3); `taskResult` is never exposed to the web (Ruling 3). */
function resultStatus(metadata: Fields): 'success' | 'error' {
  return stringField(metadata, 'toolStatus') === 'error' ? 'error' : 'success';
}

function resultDuration(metadata: Fields): number | null {
  return numberField(metadata, 'toolDurationMs');
}

/** The result text a card shows: on success, the message's own text; on
 *  failure, the error the daemon wrote into the message's JSON text
 *  (`content_from_tool_result` in anima-core's runtime — `taskResult`
 *  itself stays hidden from the web, Ruling 3). */
function resultText(message: ChatMessage, status: 'success' | 'error'): string {
  if (status !== 'error') return message.content.text;
  return (
    stringField(parsedFields(message.content.text), 'error') ??
    message.content.text
  );
}

/** The tool cards of a run the stream is showing. */
export function liveToolSteps(live: LiveRun): ToolStep[] {
  return live.tools.map((card) => ({
    toolCallId: card.toolCallId,
    name: card.name,
    argumentsPreview: previewSummary(card.argumentsPreview),
    status: card.status,
    durationMs: card.durationMs,
    result: card.resultPreview,
    truncated: card.truncated,
    runId: live.run.id,
    helper: toolHelper(
      card.name,
      parsedFields(card.argumentsPreview),
      card.resultPreview,
    ),
  }));
}

/** The session's runs: the stream's view of each, the ledger's record for
 *  the rest, and the ledger's finish when the stream missed it. */
export function mergeSessionRuns(
  live: readonly LiveRun[],
  ledger: readonly Run[],
): LiveRun[] {
  const byId = new Map(live.map((item) => [item.run.id, item]));
  for (const run of ledger) {
    const current = byId.get(run.id);
    if (!current) byId.set(run.id, emptyLiveRun(run));
    else if (
      isTerminalRunStatus(run.status) &&
      !isTerminalRunStatus(current.run.status)
    )
      byId.set(run.id, { ...current, run });
  }
  return [...byId.values()].sort(
    (left, right) => left.run.createdAtMs - right.run.createdAtMs,
  );
}

/** A finished run whose outcome the owner should see (spec §15.2). */
function hasOutcome(run: Run): boolean {
  return (
    run.status === 'failed' ||
    run.status === 'interrupted' ||
    (run.status === 'cancelled' && run.startedAtMs !== null)
  );
}

function messageItem(
  message: ChatMessage,
  delegatedBy: string | null,
): TranscriptItem {
  const metadata = metadataOf(message);
  if (message.role === 'Assistant' && metadata.revised === true)
    return { kind: 'revised', key: message.id, message };
  if (message.role === 'User' && delegatedBy)
    return {
      kind: 'delegated',
      key: message.id,
      from: delegatedBy,
      message: {
        ...message,
        content: {
          ...message.content,
          text: delegatedTaskText(message.content.text),
        },
      },
    };
  return { kind: 'message', key: message.id, message };
}

/** The session view's transcript (spec §15.2): history with tool steps
 *  grouped, then the runs and sends not yet in history. */
export function buildTranscript(input: TranscriptInput): TranscriptItem[] {
  const items: TranscriptItem[] = [];
  const lastOfRun = new Map<string, number>();
  const stoppedRuns = new Set<string>();
  // Ruling 3: an orphan tool card (its call is not in the same block)
  // takes its name from the matching assistant message's `toolCalls`,
  // never a `toolName` metadata key the daemon does not write.
  const callNames = new Map<string, string>();
  const trimmed = input.trimmedThrough ?? null;
  const delegatedBy = input.delegatedBy ?? null;
  let block: ToolsItem | null = null;

  if (trimmed && !input.messages.some((message) => message.id === trimmed))
    items.push({ kind: 'trimmed', key: 'trimmed' });

  for (const message of input.messages) {
    const runId = messageRunId(message);
    const metadata = metadataOf(message);
    if (message.role === 'Tool') {
      const callId = stringField(metadata, 'toolCallId');
      const status = resultStatus(metadata);
      const result = resultText(message, status);
      const step = block
        ? block.steps.find(
            (item) => item.toolCallId === callId && item.result === null,
          )
        : undefined;
      if (block && step) {
        step.status = status;
        step.durationMs = resultDuration(metadata);
        step.result = result;
        if (step.helper && !step.helper.agentId)
          step.helper = {
            ...step.helper,
            agentId: stringField(parsedFields(message.content.text), 'agentId'),
          };
        block.messageIds.push(message.id);
      } else {
        // A result whose call is on an older page still gets its card.
        const orphan: ToolStep = {
          toolCallId: callId ?? message.id,
          name: (callId && callNames.get(callId)) ?? 'tool',
          argumentsPreview: '',
          status,
          durationMs: resultDuration(metadata),
          result,
          truncated: false,
          runId,
          helper: null,
        };
        if (block) {
          block.steps.push(orphan);
          block.messageIds.push(message.id);
        } else {
          block = {
            kind: 'tools',
            key: `tools:${message.id}`,
            steps: [orphan],
            messageIds: [message.id],
          };
          items.push(block);
        }
      }
    } else {
      const calls = message.role === 'Assistant' ? storedCalls(metadata) : [];
      for (const call of calls) callNames.set(call.id, call.name);
      if (calls.length === 0 || message.content.text.trim()) {
        block = null;
        items.push(messageItem(message, delegatedBy));
      }
      if (calls.length > 0) {
        const steps = calls.map(
          (call): ToolStep => ({
            toolCallId: call.id,
            name: call.name,
            argumentsPreview: argumentsSummary(call.args),
            status: 'running',
            durationMs: null,
            result: null,
            truncated: false,
            runId,
            helper: toolHelper(call.name, call.args, null),
          }),
        );
        if (block) {
          block.steps.push(...steps);
          block.messageIds.push(message.id);
        } else {
          block = {
            kind: 'tools',
            key: `tools:${message.id}`,
            steps,
            messageIds: [message.id],
          };
          items.push(block);
        }
      }
    }
    if (runId) {
      lastOfRun.set(runId, items.length - 1);
      if (metadata.stopped === true) stoppedRuns.add(runId);
    }
    if (message.id === trimmed) {
      block = null;
      items.push({ kind: 'trimmed', key: 'trimmed' });
    }
  }

  // History is committed whole: a stored call with no result never got one.
  for (const item of items)
    if (item.kind === 'tools')
      for (const step of item.steps)
        if (step.result === null) step.status = 'error';

  const oldestLoaded =
    input.messages.length > 0 ? input.messages[0].created_at_ms : null;
  const inserts: { after: number; item: TranscriptItem }[] = [];
  const tail: TranscriptItem[] = [];
  for (const live of input.runs ?? []) {
    const { run } = live;
    const outcome: TranscriptItem = {
      kind: 'outcome',
      key: `outcome:${run.id}`,
      run,
    };
    const after = lastOfRun.get(run.id);
    if (after !== undefined) {
      if (
        hasOutcome(run) &&
        !(run.status === 'cancelled' && stoppedRuns.has(run.id))
      )
        inserts.push({ after, item: outcome });
      continue;
    }
    // A queued message the owner cancelled leaves nothing behind.
    if (run.status === 'cancelled' && run.startedAtMs === null) continue;
    const streamed = live.steps.length > 0 || live.tools.length > 0;
    const recent = oldestLoaded === null || run.createdAtMs >= oldestLoaded;
    if (
      !isTerminalRunStatus(run.status) ||
      streamed ||
      (hasOutcome(run) && recent)
    ) {
      tail.push({ kind: 'run', key: `run:${run.id}`, live });
      if (hasOutcome(run)) tail.push(outcome);
    }
  }
  inserts.sort((left, right) => right.after - left.after);
  for (const insert of inserts) items.splice(insert.after + 1, 0, insert.item);
  items.push(...tail);
  for (const pending of input.pending ?? [])
    items.push({ kind: 'pending', key: `pending:${pending.key}`, pending });
  return items;
}
