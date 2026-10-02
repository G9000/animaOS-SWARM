import {
  isRunLifecycleEvent,
  isTerminalRunStatus,
  type AgentEvent,
  type Approval,
  type LiveToolCard,
  type Run,
} from '@animaOS-SWARM/sdk';

/** The streamed text of one model call (spec §4.5 steps). */
export interface LiveStep {
  stepId: string;
  /** The newest part of the step's text. */
  text: string;
  /** UTF-16 units of the step's text before `text`. */
  textOffset: number;
}

/** A run as the companion's stream shows it (spec §6, §15.2). */
export interface LiveRun {
  run: Run;
  /** Model calls with streamed text, oldest first. */
  steps: LiveStep[];
  /** Tool calls in the order they started. */
  tools: LiveToolCard[];
  /** The `run.progress` phase until the run moves on, e.g. `compacting`. */
  phase: string | null;
  /** Owner messages steered into the run, in order. */
  steers: { messageId: string; text: string }[];
  /** The run's calls waiting for the owner, oldest first (spec §7.3). */
  approvals: Approval[];
}

export interface LiveState {
  /** The `seq` of the newest event applied from the current stream. */
  seq: number;
  runs: Readonly<Record<string, LiveRun>>;
  /** Every pending approval this stream knows, by id (spec §7.3). */
  approvals: Readonly<Record<string, Approval>>;
  /** Bumped by every snapshot and resync, so views refetch what they show. */
  epoch: number;
}

export const EMPTY_LIVE_STATE: LiveState = {
  seq: 0,
  runs: {},
  approvals: {},
  epoch: 0,
};

/** A step's streamed text kept in the page; its full text arrives with the
 *  committed message. */
export const MAX_LIVE_STEP_CHARS = 200_000;
/** Finished runs kept for views still waiting on their committed messages. */
export const MAX_FINISHED_LIVE_RUNS = 50;
/** Tool cards kept per run, the oldest dropped first past this, matching
 *  the daemon's live registry (`MAX_LIVE_TOOL_CARDS`, S3b-B). */
export const MAX_LIVE_TOOL_CARDS = 50;

export function emptyLiveRun(run: Run): LiveRun {
  return { run, steps: [], tools: [], phase: null, steers: [], approvals: [] };
}

function byRequest(left: Approval, right: Approval): number {
  return (
    left.createdAtMs - right.createdAtMs || left.id.localeCompare(right.id)
  );
}

/** Pending approvals, oldest first. */
export function pendingApprovals(
  approvals: LiveState['approvals'],
): Approval[] {
  return Object.values(approvals).sort(byRequest);
}

function runApprovals(
  approvals: LiveState['approvals'],
  runId: string,
): Approval[] {
  return pendingApprovals(approvals).filter(
    (approval) => approval.runId === runId,
  );
}

/** Adds a pending approval once, on its run if the stream has the run. A
 *  run that already finished waits on nothing, so a late request is ignored. */
function withApproval(state: LiveState, approval: Approval): LiveState {
  if (approval.status !== 'pending' || state.approvals[approval.id])
    return state;
  const live = state.runs[approval.runId];
  if (live && isTerminalRunStatus(live.run.status)) return state;
  const approvals = { ...state.approvals, [approval.id]: approval };
  return {
    ...state,
    approvals,
    runs: live
      ? {
          ...state.runs,
          [approval.runId]: {
            ...live,
            approvals: runApprovals(approvals, approval.runId),
          },
        }
      : state.runs,
  };
}

/** Removes approvals the stream holds, from their runs too; an id it never
 *  held (a request settled before it was announced) is ignored. */
function withoutApprovals(state: LiveState, ids: readonly string[]): LiveState {
  const gone = ids.filter((id) => state.approvals[id]);
  if (gone.length === 0) return state;
  const approvals = { ...state.approvals };
  const runs = { ...state.runs };
  for (const id of gone) {
    const { runId } = approvals[id];
    delete approvals[id];
    const live = runs[runId];
    if (live)
      runs[runId] = {
        ...live,
        approvals: live.approvals.filter((approval) => approval.id !== id),
      };
  }
  return { ...state, approvals, runs };
}

/** Running or waiting for an approval: the session's reply is in progress. */
export function isActiveRun(run: Pick<Run, 'status'>): boolean {
  return run.status === 'running' || run.status === 'awaiting_approval';
}

/** The run a step id (`<runId>:<n>`) belongs to. */
export function stepRunId(stepId: string): string {
  const index = stepId.lastIndexOf(':');
  return index < 0 ? stepId : stepId.slice(0, index);
}

function isLowSurrogate(code: number): boolean {
  return code >= 0xdc00 && code <= 0xdfff;
}

/** At most `MAX_LIVE_STEP_CHARS`, never starting inside a surrogate pair. */
function capped(step: LiveStep): LiveStep {
  let cut = step.text.length - MAX_LIVE_STEP_CHARS;
  if (cut <= 0) return step;
  if (isLowSurrogate(step.text.charCodeAt(cut))) cut += 1;
  return {
    ...step,
    text: step.text.slice(cut),
    textOffset: step.textOffset + cut,
  };
}

/**
 * Adds a delta to its step. Offsets count UTF-16 units from the step's start,
 * so text the step already has (a delta that overlaps what a snapshot
 * carried) is dropped, and a delta past the step's end (events this stream
 * missed) is ignored until the resync or the committed message.
 */
export function appendDelta(
  step: LiveStep,
  offset: number,
  text: string,
): LiveStep {
  const end = step.textOffset + step.text.length;
  if (offset > end) return step;
  const fresh = text.slice(end - offset);
  if (!fresh) return step;
  return capped({ ...step, text: step.text + fresh });
}

function finishedAt(live: LiveRun): number {
  return live.run.finishedAtMs ?? live.run.createdAtMs;
}

function withoutOldFinished(
  runs: Record<string, LiveRun>,
): Record<string, LiveRun> {
  const finished = Object.values(runs).filter((live) =>
    isTerminalRunStatus(live.run.status),
  );
  if (finished.length <= MAX_FINISHED_LIVE_RUNS) return runs;
  finished.sort((left, right) => finishedAt(left) - finishedAt(right));
  const next = { ...runs };
  for (const live of finished.slice(
    0,
    finished.length - MAX_FINISHED_LIVE_RUNS,
  ))
    delete next[live.run.id];
  return next;
}

function withRun(state: LiveState, run: Run): LiveState {
  const current = state.runs[run.id];
  // A finished run never goes back to an earlier status.
  if (
    current &&
    isTerminalRunStatus(current.run.status) &&
    !isTerminalRunStatus(run.status)
  )
    return state;
  const next: LiveRun = current
    ? {
        ...current,
        run,
        phase: isTerminalRunStatus(run.status) ? null : current.phase,
      }
    : // An approval that arrived before its run joins it now.
      {
        ...emptyLiveRun(run),
        approvals: runApprovals(state.approvals, run.id),
      };
  const updated: LiveState = {
    ...state,
    runs: withoutOldFinished({ ...state.runs, [run.id]: next }),
  };
  if (!isTerminalRunStatus(run.status)) return updated;
  // A finished run waits on nothing: drop what the stream still holds.
  return withoutApprovals(
    updated,
    Object.values(updated.approvals)
      .filter((approval) => approval.runId === run.id)
      .map((approval) => approval.id),
  );
}

function updateRun(
  state: LiveState,
  runId: string,
  update: (live: LiveRun) => LiveRun,
): LiveState {
  const live = state.runs[runId];
  if (!live) return state;
  const next = update(live);
  return next === live
    ? state
    : { ...state, runs: { ...state.runs, [runId]: next } };
}

/** Adds a tool card or updates the one with its step and call id (a
 *  provider may reuse a call id in every step, as the daemon's live
 *  registry allows); a late start never undoes a finish. */
function upsertTool(
  tools: LiveToolCard[],
  card: LiveToolCard,
  finished: boolean,
): LiveToolCard[] {
  const index = tools.findIndex(
    (tool) =>
      tool.stepId === card.stepId && tool.toolCallId === card.toolCallId,
  );
  if (index < 0) {
    const next = [...tools, card];
    return next.length > MAX_LIVE_TOOL_CARDS
      ? next.slice(next.length - MAX_LIVE_TOOL_CARDS)
      : next;
  }
  const current = tools[index];
  if (!finished && current.status !== 'running') return tools;
  const next = [...tools];
  next[index] = finished
    ? {
        ...current,
        status: card.status,
        durationMs: card.durationMs,
        resultPreview: card.resultPreview,
        truncated: card.truncated,
      }
    : { ...current, ...card };
  return next;
}

export function applyEvent(state: LiveState, event: AgentEvent): LiveState {
  if (event.type === 'stream.snapshot') {
    const approvals: Record<string, Approval> = {};
    for (const approval of event.approvals)
      if (approval.status === 'pending') approvals[approval.id] = approval;
    const runs: Record<string, LiveRun> = {};
    for (const item of event.runs) {
      runs[item.run.id] = {
        ...emptyLiveRun(item.run),
        steps: item.stepId
          ? [
              capped({
                stepId: item.stepId,
                text: item.text,
                textOffset: item.textOffset,
              }),
            ]
          : [],
        tools: item.tools,
        approvals: runApprovals(approvals, item.run.id),
      };
    }
    return { seq: event.seq, runs, approvals, epoch: state.epoch + 1 };
  }
  // A seq at or below the last one applied is a repeat from this stream.
  if (event.seq <= state.seq) return state;
  const next: LiveState = { ...state, seq: event.seq };
  if (event.type === 'stream.resync')
    return { ...next, epoch: state.epoch + 1 };
  if (isRunLifecycleEvent(event)) return withRun(next, event.run);
  if (event.type === 'approval.requested')
    return withApproval(next, event.approval);
  if (event.type === 'approval.resolved')
    return withoutApprovals(next, [event.approval.id]);
  const runId = event.runId;
  if (!runId) return next;
  switch (event.type) {
    case 'run.progress':
      return updateRun(next, runId, (live) => ({
        ...live,
        phase: event.phase,
      }));
    case 'run.steered':
      return updateRun(next, runId, (live) =>
        live.steers.some((steer) => steer.messageId === event.messageId)
          ? live
          : {
              ...live,
              steers: [
                ...live.steers,
                { messageId: event.messageId, text: event.text },
              ],
            },
      );
    case 'step.delta':
      return updateRun(next, runId, (live) => {
        const index = live.steps.findIndex(
          (step) => step.stepId === event.stepId,
        );
        if (index < 0) {
          // A step that began before this stream joined shows from there.
          const step = capped({
            stepId: event.stepId,
            text: event.text,
            textOffset: event.offset,
          });
          return { ...live, phase: null, steps: [...live.steps, step] };
        }
        const step = appendDelta(live.steps[index], event.offset, event.text);
        if (step === live.steps[index] && live.phase === null) return live;
        const steps = [...live.steps];
        steps[index] = step;
        return { ...live, phase: null, steps };
      });
    case 'tool.started':
      return updateRun(next, runId, (live) => ({
        ...live,
        phase: null,
        tools: upsertTool(
          live.tools,
          {
            stepId: event.stepId,
            toolCallId: event.toolCallId,
            name: event.name,
            argumentsPreview: event.argumentsPreview,
            argumentsTruncated: event.argumentsTruncated,
            status: 'running',
            durationMs: null,
            resultPreview: null,
            truncated: false,
          },
          false,
        ),
      }));
    case 'tool.finished':
      return updateRun(next, runId, (live) => ({
        ...live,
        tools: upsertTool(
          live.tools,
          {
            stepId: event.stepId,
            toolCallId: event.toolCallId,
            name: event.name,
            argumentsPreview: '',
            argumentsTruncated: false,
            status: event.status,
            durationMs: event.durationMs,
            resultPreview: event.resultPreview,
            truncated: event.truncated,
          },
          true,
        ),
      }));
    default:
      return next;
  }
}

/** Drops a finished run's steps and tool cards once its messages are
 *  committed (spec §15.5, S3b-B): the transcript renders it from history
 *  from then on (`placeRuns`), so the stream's own copy just holds memory.
 *  A no-op for a run still going, already trimmed, or unknown. */
export function trimCommittedRun(state: LiveState, runId: string): LiveState {
  const live = state.runs[runId];
  if (!live || !isTerminalRunStatus(live.run.status)) return state;
  if (live.steps.length === 0 && live.tools.length === 0) return state;
  return {
    ...state,
    runs: { ...state.runs, [runId]: { ...live, steps: [], tools: [] } },
  };
}

/** The runs of one session, oldest first. */
export function sessionLiveRuns(
  state: LiveState,
  agentId: string,
  sessionId: string,
): LiveRun[] {
  return Object.values(state.runs)
    .filter(
      (live) =>
        live.run.agentId === agentId && live.run.sessionId === sessionId,
    )
    .sort((left, right) => left.run.createdAtMs - right.run.createdAtMs);
}
