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
  /** The model call that made it; with `toolCallId`, the card's identity
   *  (a provider may reuse a call id in every step). Null when unknown. */
  stepId: string | null;
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
  /** Runs already sent again from this page: their Retry or Send again is
   *  used up. */
  resentRunIds?: ReadonlySet<string>;
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
 *  (spec §3.3); `taskResult` is never exposed to the web (Ruling 3). A
 *  pre-M3 tool message carries no `toolStatus` marker at all, so its
 *  status falls back to the `{status,data,error}` JSON shape of its own
 *  text (the same shape a failed result's text uses). */
function resultStatus(
  message: ChatMessage,
  metadata: Fields,
): 'success' | 'error' {
  const marker = stringField(metadata, 'toolStatus');
  if (marker) return marker === 'error' ? 'error' : 'success';
  return stringField(parsedFields(message.content.text), 'status') === 'error'
    ? 'error'
    : 'success';
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
    stepId: card.stepId,
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

/** Where a non-terminal status sits in the run lifecycle (spec §4.1). */
const NON_TERMINAL_ORDER: Record<string, number> = {
  queued: 0,
  running: 1,
  awaiting_approval: 2,
};

/** Whether `next` is further along the (non-terminal) lifecycle than
 *  `current` — never true once either side is terminal (a finish is
 *  decided by the terminal-status branch above, not this one). */
function isMoreAdvanced(next: Run['status'], current: Run['status']): boolean {
  if (isTerminalRunStatus(next) || isTerminalRunStatus(current)) return false;
  return NON_TERMINAL_ORDER[next] > NON_TERMINAL_ORDER[current];
}

/** The session's runs: the stream's view of each, the ledger's record for
 *  the rest, and the ledger's finish when the stream missed it. When both
 *  are still mid-flight, the more advanced status wins, so a stale
 *  `queued` snapshot never outlives the ledger's `running`. */
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
    else if (isMoreAdvanced(run.status, current.run.status))
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

/** The loaded history as transcript items, with what placing the runs
 *  among them needs. */
export interface TranscriptHistory {
  readonly items: readonly TranscriptItem[];
  /** When each item's first message was created; null for the divider. */
  readonly startedAtMs: readonly (number | null)[];
  /** The index of each run's last item. */
  readonly lastOfRun: ReadonlyMap<string, number>;
  /** Runs whose committed partial reply is labelled "Stopped". */
  readonly stoppedRuns: ReadonlySet<string>;
  /** When the oldest loaded message was created; null without history. */
  readonly oldestLoadedMs: number | null;
}

export type HistoryInput = Pick<
  TranscriptInput,
  'messages' | 'trimmedThrough' | 'delegatedBy'
>;

/** The session's history with tool steps grouped (spec §15.2). Built once
 *  per history change, so its items keep their identity while the
 *  session's runs stream and the view skips rendering them again. */
export function buildHistory(input: HistoryInput): TranscriptHistory {
  const items: TranscriptItem[] = [];
  const startedAtMs: (number | null)[] = [];
  const push = (item: TranscriptItem, at: number | null) => {
    items.push(item);
    startedAtMs.push(at);
  };
  const lastOfRun = new Map<string, number>();
  const stoppedRuns = new Set<string>();
  // Ruling 3: an orphan tool card (its call is not in the same block)
  // takes its name from the matching assistant message's `toolCalls`,
  // never a `toolName` metadata key the daemon does not write. This spans
  // the whole loaded page, not just the current block, so a delayed or
  // recovered result still gets the name its call used.
  const callNames = new Map<string, string>();
  const trimmed = input.trimmedThrough ?? null;
  const delegatedBy = input.delegatedBy ?? null;
  let block: ToolsItem | null = null;
  // The run `block` belongs to; a message from a different run (or a
  // known run following an unattributed one, or vice versa) starts a new
  // block instead of joining it, even with nothing textual in between.
  let blockRunId: string | null = null;

  if (trimmed && !input.messages.some((message) => message.id === trimmed))
    push({ kind: 'trimmed', key: 'trimmed' }, null);

  // The open block belonging to `runId`, or null if there is none (`block`
  // is read live: a same-iteration reset below must be seen by a later
  // check in that same iteration, not a value captured before it).
  const openBlockFor = (runId: string | null): ToolsItem | null =>
    block && blockRunId === runId ? block : null;

  for (const message of input.messages) {
    const runId = messageRunId(message);
    const metadata = metadataOf(message);
    if (message.role === 'Tool') {
      const callId = stringField(metadata, 'toolCallId');
      const stepId = stringField(metadata, 'stepId');
      const status = resultStatus(message, metadata);
      const result = resultText(message, status);
      const current = openBlockFor(runId);
      // A result answers the call of its own step: a provider may reuse a
      // call id in every step of a run.
      const step = current
        ? current.steps.find(
            (item) =>
              item.toolCallId === callId &&
              item.result === null &&
              (stepId === null ||
                item.stepId === null ||
                item.stepId === stepId),
          )
        : undefined;
      if (current && step) {
        step.status = status;
        step.durationMs = resultDuration(metadata);
        step.result = result;
        if (step.helper && !step.helper.agentId)
          step.helper = {
            ...step.helper,
            agentId: stringField(parsedFields(message.content.text), 'agentId'),
          };
        current.messageIds.push(message.id);
      } else {
        // A result whose call is on an older page, or not in this block,
        // still gets its own card.
        const orphan: ToolStep = {
          stepId,
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
        if (current) {
          current.steps.push(orphan);
          current.messageIds.push(message.id);
        } else {
          block = {
            kind: 'tools',
            key: `tools:${message.id}`,
            steps: [orphan],
            messageIds: [message.id],
          };
          blockRunId = runId;
          push(block, message.created_at_ms);
        }
      }
    } else {
      const calls = message.role === 'Assistant' ? storedCalls(metadata) : [];
      for (const call of calls) callNames.set(call.id, call.name);
      if (calls.length === 0 || message.content.text.trim()) {
        block = null;
        blockRunId = null;
        push(messageItem(message, delegatedBy), message.created_at_ms);
      }
      if (calls.length > 0) {
        const stepId = stringField(metadata, 'stepId');
        const steps = calls.map(
          (call): ToolStep => ({
            stepId,
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
        // Re-checked after the reset just above: a message with both text
        // and calls always starts its tool block fresh, never merging
        // into the block its own leading bubble just closed.
        const current = openBlockFor(runId);
        if (current) {
          current.steps.push(...steps);
          current.messageIds.push(message.id);
        } else {
          block = {
            kind: 'tools',
            key: `tools:${message.id}`,
            steps,
            messageIds: [message.id],
          };
          blockRunId = runId;
          push(block, message.created_at_ms);
        }
      }
    }
    if (runId) {
      lastOfRun.set(runId, items.length - 1);
      if (metadata.stopped === true) stoppedRuns.add(runId);
    }
    if (message.id === trimmed) {
      block = null;
      blockRunId = null;
      push({ kind: 'trimmed', key: 'trimmed' }, null);
    }
  }

  // History is committed whole: a stored call with no result never got one.
  for (const item of items)
    if (item.kind === 'tools')
      for (const step of item.steps)
        if (step.result === null) step.status = 'error';

  return {
    items,
    startedAtMs,
    lastOfRun,
    stoppedRuns,
    oldestLoadedMs:
      input.messages.length > 0 ? input.messages[0].created_at_ms : null,
  };
}

export interface LiveInput {
  runs?: readonly LiveRun[];
  pending?: readonly PendingBubble[];
  /** Older history may exist beyond the loaded page (assumed unless said
   *  otherwise): a run older than the page's oldest message belongs among
   *  messages not loaded, so it waits for them. */
  olderHistory?: boolean;
}

/** An item a run adds, placed after the history item at `after`. */
interface RunInsert {
  after: number;
  /** The run's place in `runs`, which keeps ties in order. */
  order: number;
  items: TranscriptItem[];
}

/**
 * The transcript (spec §15.2): the history's own items, unchanged, with
 * the session's runs and sends placed among and after them.
 *
 * - A run with committed messages gets its outcome after its last item.
 * - A finished run with nothing committed (it failed, stopped, or was
 *   interrupted before any message) sits where it happened: before the
 *   first history item whose first message was created after it.
 * - Runs still going, and finished runs this page saw stream (their
 *   messages are on their way), stay at the end, followed by the sends
 *   the daemon has not accepted yet.
 */
export function placeRuns(
  history: TranscriptHistory,
  input: LiveInput,
): TranscriptItem[] {
  const { lastOfRun, stoppedRuns, startedAtMs } = history;
  const oldestLoaded = history.oldestLoadedMs;
  const olderHistory = input.olderHistory ?? true;
  const items = [...history.items];
  /** The item a run created at `at` goes after. */
  const anchorFor = (at: number): number => {
    const next = startedAtMs.findIndex(
      (started) => started !== null && started > at,
    );
    return (next < 0 ? startedAtMs.length : next) - 1;
  };
  const inserts: RunInsert[] = [];
  const tail: TranscriptItem[] = [];
  for (const [order, live] of (input.runs ?? []).entries()) {
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
        inserts.push({ after, order, items: [outcome] });
      continue;
    }
    // A queued message the owner cancelled leaves nothing behind.
    if (run.status === 'cancelled' && run.startedAtMs === null) continue;
    const shown: TranscriptItem[] = [
      { kind: 'run', key: `run:${run.id}`, live },
      ...(hasOutcome(run) ? [outcome] : []),
    ];
    const streamed = live.steps.length > 0 || live.tools.length > 0;
    if (!isTerminalRunStatus(run.status) || streamed) {
      tail.push(...shown);
      continue;
    }
    const loaded =
      oldestLoaded === null || run.createdAtMs >= oldestLoaded || !olderHistory;
    if (hasOutcome(run) && loaded)
      inserts.push({ after: anchorFor(run.createdAtMs), order, items: shown });
  }
  // Descending by anchor so an earlier splice never shifts a later one's
  // target index; descending by original order within a tie so splicing
  // (which pushes the most-recently-inserted item back) still lands the
  // ties in their original relative order (spec §15.2).
  inserts.sort(
    (left, right) => right.after - left.after || right.order - left.order,
  );
  for (const insert of inserts)
    items.splice(insert.after + 1, 0, ...insert.items);
  items.push(...tail);
  for (const pending of input.pending ?? [])
    items.push({ kind: 'pending', key: `pending:${pending.key}`, pending });
  return items;
}

/** The session view's transcript (spec §15.2): history with tool steps
 *  grouped, then the runs and sends not yet in history. */
export function buildTranscript(input: TranscriptInput): TranscriptItem[] {
  return placeRuns(buildHistory(input), input);
}
