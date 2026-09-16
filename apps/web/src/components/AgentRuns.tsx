import { useEffect, useRef, useState } from 'react';
import {
  DaemonHttpError,
  type AgentJob,
  type GoalView,
} from '@animaOS-SWARM/sdk';
import { daemon } from '../lib/daemon-api';

type Draft = {
  title: string;
  prompt: string;
  requestKey: string;
  maxAttempts: number;
  requiresApproval: boolean;
  goalId: string | null;
};
const labels: Record<AgentJob['status'], string> = {
  awaiting_approval: 'Awaiting approval',
  queued: 'Queued',
  running: 'Running',
  completed: 'Completed',
  failed: 'Failed',
  needs_review: 'Needs review',
  cancelled: 'Cancelled',
};
const freshDraft = (): Draft => ({
  title: '',
  prompt: '',
  requestKey: crypto.randomUUID(),
  maxAttempts: 3,
  requiresApproval: false,
  goalId: null,
});
const storageKey = (id: string) => `anima:run-draft:${id}`;
function readDraft(id: string): Draft {
  try {
    const saved = JSON.parse(sessionStorage.getItem(storageKey(id)) ?? 'null');
    if (
      saved &&
      typeof saved.title === 'string' &&
      typeof saved.prompt === 'string' &&
      typeof saved.requestKey === 'string'
    )
      return {
        title: saved.title,
        prompt: saved.prompt,
        requestKey: saved.requestKey,
        maxAttempts: [1, 2, 3].includes(saved.maxAttempts)
          ? saved.maxAttempts
          : 3,
        requiresApproval: saved.requiresApproval === true,
        goalId: typeof saved.goalId === 'string' ? saved.goalId : null,
      };
  } catch {
    /* A draft still works when browser storage is unavailable. */
  }
  return freshDraft();
}
function storeDraft(id: string, draft: Draft) {
  try {
    sessionStorage.setItem(storageKey(id), JSON.stringify(draft));
  } catch {
    /* Keep the in-memory draft. */
  }
}
const message = (error: unknown) =>
  error instanceof Error ? error.message : String(error);

type RunsProps = {
  agentId: string;
  jobId?: string;
  composeOnly?: boolean;
  online?: boolean;
  snapshot?: AgentJob;
  refreshKey?: number;
  onChanged?: (job?: AgentJob) => void;
};
export function AgentRunsView(props: RunsProps) {
  return <AgentRuns key={props.agentId} {...props} />;
}

function AgentRuns({
  agentId,
  jobId,
  composeOnly = false,
  online = true,
  snapshot,
  refreshKey = 0,
  onChanged,
}: RunsProps) {
  const [draft, setDraft] = useState(() => readDraft(agentId));
  const draftRef = useRef(draft);
  const [jobs, setJobs] = useState<AgentJob[] | null>(
    snapshot ? [snapshot] : null,
  );
  useEffect(() => {
    if (!snapshot) return;
    setJobs((current) => {
      const existing = current?.find((job) => job.id === snapshot.id);
      if (existing && existing.revision > snapshot.revision) return current;
      return [
        snapshot,
        ...(current ?? []).filter((job) => job.id !== snapshot.id),
      ];
    });
  }, [snapshot]);
  const [loadError, setLoadError] = useState('');
  const [actionError, setActionError] = useState('');
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const [refresh, setRefresh] = useState(0);
  const [pollingPaused, setPollingPaused] = useState(false);
  const [acknowledged, setAcknowledged] = useState<Record<string, boolean>>({});
  const [feedback, setFeedback] = useState<Record<string, string>>({});
  const [goals, setGoals] = useState<GoalView[]>([]);
  const [goalError, setGoalError] = useState('');
  useEffect(() => {
    if (!online) return;
    const controller = new AbortController();
    void daemon
      .goals({ signal: controller.signal })
      .then((value) => {
        if (!controller.signal.aborted) {
          setGoals(value);
          setGoalError('');
        }
      })
      .catch((error) => {
        if (!controller.signal.aborted) setGoalError(message(error));
      });
    return () => controller.abort();
  }, [refresh, online, refreshKey]);
  const live = useRef(true);
  useEffect(() => {
    live.current = true;
    return () => {
      live.current = false;
    };
  }, []);

  useEffect(() => {
    if (!online) return;
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let remaining = 60;
    setPollingPaused(false);
    async function load() {
      try {
        const result = await daemon.agentJobs(agentId, {
          signal: controller.signal,
        });
        if (controller.signal.aborted) return;
        setJobs(result);
        setLoadError('');
        if (
          result.some(
            (job) => job.status === 'queued' || job.status === 'running',
          )
        ) {
          if (--remaining > 0) timer = setTimeout(() => void load(), 5000);
          else setPollingPaused(true);
        }
      } catch (error) {
        if (!controller.signal.aborted) setLoadError(message(error));
      }
    }
    void load();
    return () => {
      controller.abort();
      clearTimeout(timer);
    };
  }, [agentId, refresh, online, refreshKey]);

  function change(patch: Partial<Omit<Draft, 'requestKey'>>) {
    const next = {
      ...draftRef.current,
      ...patch,
      requestKey: crypto.randomUUID(),
    };
    draftRef.current = next;
    storeDraft(agentId, next);
    setDraft(next);
  }

  async function mutate(operation: () => Promise<AgentJob>, submitted?: Draft) {
    if (busyRef.current || !online) return;
    busyRef.current = true;
    setBusy(true);
    setActionError('');
    try {
      const result = await operation();
      if (!live.current) return;
      setJobs((current) => [
        result,
        ...(current ?? []).filter((job) => job.id !== result.id),
      ]);
      setAcknowledged((current) => ({ ...current, [result.id]: false }));
      if (submitted && draftRef.current.requestKey === submitted.requestKey) {
        const next = freshDraft();
        draftRef.current = next;
        storeDraft(agentId, next);
        setDraft(next);
      }
      setRefresh((value) => value + 1);
      onChanged?.(result);
    } catch (error) {
      if (!live.current) return;
      setActionError(
        error instanceof DaemonHttpError && error.status === 409
          ? `${error.message}. Refreshing the run history; review its latest state before trying again.`
          : submitted
            ? `${message(error)}. Your assignment draft is retained. You can retry the same submission safely.`
            : `${message(error)}. Refresh the run history before trying again.`,
      );
      if (error instanceof DaemonHttpError && error.status === 409) {
        setAcknowledged({});
        setRefresh((value) => value + 1);
        onChanged?.();
      }
    } finally {
      busyRef.current = false;
      if (live.current) setBusy(false);
    }
  }

  return (
    <fieldset disabled={!online} className="operator-run space-y-6 min-w-0">
      {!online && (
        <p role="status">Offline. Reconnect to update this assignment.</p>
      )}
      {!jobId && (
        <>
          <p className="text-sm text-ink-3">
            Queue an assignment to start work through the daemon, even after you
            close this page. The daemon must remain active with workspace
            persistence configured. Saving a task does not start a run.
          </p>
          <form
            className="space-y-4 rounded-2xl border border-line p-5"
            onSubmit={(event) => {
              event.preventDefault();
              if (!draft.title.trim() || !draft.prompt.trim()) return;
              storeDraft(agentId, draft);
              void mutate(() => daemon.createAgentJob(agentId, draft), draft);
            }}
          >
            <label className="block space-y-2 text-sm">
              Run title
              <input
                className="field"
                required
                maxLength={160}
                value={draft.title}
                onChange={(event) => change({ title: event.target.value })}
              />
            </label>
            <label className="block space-y-2 text-sm">
              Assignment
              <textarea
                className="field min-h-28"
                required
                value={draft.prompt}
                onChange={(event) => change({ prompt: event.target.value })}
              />
            </label>
            <label className="block space-y-2 text-sm">
              Maximum attempts
              <select
                className="field"
                value={draft.maxAttempts}
                onChange={(event) =>
                  change({ maxAttempts: Number(event.target.value) })
                }
              >
                {[1, 2, 3].map((count) => (
                  <option key={count} value={count}>
                    {count}
                  </option>
                ))}
              </select>
            </label>
            <label className="block space-y-2 text-sm">
              Goal
              <select
                className="field"
                value={draft.goalId ?? ''}
                onChange={(event) =>
                  change({ goalId: event.target.value || null })
                }
              >
                <option value="">No goal</option>
                {draft.goalId &&
                  !goals.some((goal) => goal.id === draft.goalId) && (
                    <option value={draft.goalId}>
                      Saved goal · unavailable
                    </option>
                  )}
                {goals.map((goal) => (
                  <option
                    key={goal.id}
                    value={goal.id}
                    disabled={goal.status === 'completed'}
                  >
                    {goal.title} · {goal.status} · {goal.remainingAttempts}{' '}
                    attempts available
                  </option>
                ))}
              </select>
            </label>
            {goalError && (
              <p role="alert" className="text-sm text-danger">
                Could not load goals: {goalError}. Your selected goal is
                retained.
                <button
                  type="button"
                  className="ml-2 underline"
                  disabled={busy}
                  onClick={() => setRefresh((value) => value + 1)}
                >
                  Retry loading goals
                </button>
              </p>
            )}
            {goals.find((goal) => goal.id === draft.goalId)?.status ===
              'paused' && (
              <p className="text-sm text-ink-3">
                This goal is paused. You may save a proposal; work can start
                after the goal resumes.
              </p>
            )}
            {goals.find((goal) => goal.id === draft.goalId)
              ?.remainingAttempts === 0 && (
              <p className="text-sm text-ink-3">
                This goal has no attempts available. A proposal will wait until
                a reservation is freed; consumed attempts are not refunded.
              </p>
            )}
            <label className="flex items-start gap-2 text-sm">
              <input
                type="checkbox"
                checked={draft.requiresApproval}
                onChange={(event) =>
                  change({ requiresApproval: event.target.checked })
                }
              />
              Require approval before each attempt
            </label>
            <p className="text-xs text-ink-3">
              Retries are always started explicitly. Approval applies to
              starting the assignment; existing tool permissions still apply.
            </p>
            <button
              type="submit"
              className="rounded-xl bg-accent px-4 py-3 text-sm text-accent-fg disabled:opacity-50"
              disabled={busy || !draft.title.trim() || !draft.prompt.trim()}
            >
              {draft.requiresApproval ? 'Save proposal' : 'Queue run'}
            </button>
            {busy && (
              <p role="status" className="text-sm">
                Saving run…
              </p>
            )}
          </form>
        </>
      )}
      {actionError && (
        <p role="alert" className="text-sm text-danger">
          {actionError}
        </p>
      )}
      {!composeOnly && (
        <>
          <div className="flex items-center justify-between gap-3">
            <h3 className="font-semibold">Run history</h3>
            <button
              type="button"
              className="rounded-xl border border-line px-4 py-2 text-sm"
              disabled={busy}
              onClick={() => setRefresh((value) => value + 1)}
            >
              Refresh runs
            </button>
          </div>
          {loadError && (
            <p role="alert" className="text-sm text-danger">
              Could not load runs: {loadError}.{' '}
              {jobs
                ? 'The history below may be outdated.'
                : 'Run history is unavailable.'}{' '}
              Use Refresh runs to retry.
            </p>
          )}
          {pollingPaused && (
            <p className="text-sm text-ink-3">
              Automatic refresh paused after five minutes. Refresh runs to check
              again.
            </p>
          )}
          {jobs === null && !loadError && (
            <p role="status">
              {online
                ? 'Loading runs…'
                : 'Assignment detail is unavailable offline.'}
            </p>
          )}
          {jobs?.length === 0 && !loadError && (
            <p className="text-sm text-ink-3">No runs yet.</p>
          )}
          <div className="space-y-4">
            {jobId &&
              jobs &&
              !jobs.some((job) => job.id === jobId) &&
              !loadError && (
                <p role="status">
                  This assignment is no longer available. Refresh the queue to
                  check its latest state.
                </p>
              )}
            {jobs
              ?.filter((job) => !jobId || job.id === jobId)
              .map((job) => (
                <article
                  key={job.id}
                  className="min-w-0 space-y-3 rounded-2xl border border-line p-5"
                >
                  <div className="flex flex-wrap justify-between gap-2">
                    <h4 className="break-words font-semibold">{job.title}</h4>
                    <span className="operator-status" data-status={job.status}>
                      {labels[job.status]}
                    </span>
                  </div>
                  <p className="text-xs text-ink-3">
                    Attempt {job.attempt} of {job.maxAttempts} · Updated{' '}
                    {new Date(job.updatedAtMs).toLocaleString()}
                  </p>
                  <details open={!!jobId}>
                    <summary className="cursor-pointer text-sm">
                      Assignment
                    </summary>
                    <p className="whitespace-pre-wrap break-words text-sm">
                      {job.prompt}
                    </p>
                  </details>
                  {job.attempts.length === 0 && job.result !== null && (
                    <div>
                      <h5 className="text-sm font-semibold">Result</h5>
                      <pre className="max-h-96 overflow-auto whitespace-pre-wrap break-words font-sans text-sm">
                        {job.result}
                      </pre>
                    </div>
                  )}
                  {job.error && (
                    <p className="whitespace-pre-wrap break-words text-sm text-danger">
                      {job.error}
                    </p>
                  )}
                  {job.attempts.map((attempt) => (
                    <details
                      key={attempt.attempt}
                      className="space-y-2 rounded-xl border border-line p-3"
                      open={attempt.attempt === job.attempt}
                    >
                      <summary className="cursor-pointer text-sm font-semibold">
                        Saved attempt {attempt.attempt} ·{' '}
                        {labels[attempt.status]}
                      </summary>
                      <p className="text-xs text-ink-3">
                        Finished{' '}
                        {new Date(attempt.finishedAtMs).toLocaleString()}
                      </p>
                      {attempt.result !== null && (
                        <pre className="max-h-96 overflow-auto whitespace-pre-wrap break-words font-sans text-sm">
                          {attempt.result}
                        </pre>
                      )}
                      {attempt.resultTruncated && (
                        <p className="text-xs text-ink-3">
                          Output was truncated to the saved text limit.
                        </p>
                      )}
                      {attempt.error && (
                        <p className="whitespace-pre-wrap break-words text-sm text-danger">
                          {attempt.error}
                        </p>
                      )}
                      {attempt.review && (
                        <div className="space-y-1 text-sm">
                          <p className="font-semibold">
                            {attempt.review.decision === 'accepted'
                              ? 'Result accepted'
                              : 'Changes requested'}
                          </p>
                          <p className="whitespace-pre-wrap break-words">
                            {attempt.review.note}
                          </p>
                        </div>
                      )}
                    </details>
                  ))}
                  {job.status === 'completed' &&
                    !job.attempts.find(
                      (attempt) => attempt.attempt === job.attempt,
                    )?.review && (
                      <div className="space-y-3">
                        <p className="text-sm text-ink-3">
                          Execution completed. Review the saved output to accept
                          it or request changes. This does not authorize
                          external actions.
                        </p>
                        <label className="block space-y-2 text-sm">
                          Review feedback
                          <textarea
                            className="field min-h-20"
                            value={feedback[`${job.id}:${job.attempt}`] ?? ''}
                            onChange={(event) =>
                              setFeedback((current) => ({
                                ...current,
                                [`${job.id}:${job.attempt}`]:
                                  event.target.value,
                              }))
                            }
                          />
                        </label>
                        {new TextEncoder().encode(
                          feedback[`${job.id}:${job.attempt}`] ?? '',
                        ).length > 4000 && (
                          <p className="text-sm text-danger">
                            Feedback must be at most 4,000 bytes.
                          </p>
                        )}
                        <div className="flex flex-wrap gap-4">
                          {(['accepted', 'changes_requested'] as const).map(
                            (decision) => (
                              <button
                                key={decision}
                                type="button"
                                className="text-sm underline disabled:opacity-50"
                                disabled={
                                  busy ||
                                  !!loadError ||
                                  new TextEncoder().encode(
                                    feedback[`${job.id}:${job.attempt}`] ?? '',
                                  ).length > 4000 ||
                                  (decision === 'changes_requested' &&
                                    !(
                                      feedback[`${job.id}:${job.attempt}`] ?? ''
                                    ).trim())
                                }
                                onClick={() =>
                                  void mutate(() =>
                                    daemon.reviewAgentJob(agentId, job.id, {
                                      revision: job.revision,
                                      decision,
                                      note:
                                        feedback[`${job.id}:${job.attempt}`] ??
                                        '',
                                    }),
                                  )
                                }
                              >
                                {decision === 'accepted'
                                  ? 'Accept result'
                                  : 'Request changes'}
                              </button>
                            ),
                          )}
                        </div>
                      </div>
                    )}
                  {job.status === 'awaiting_approval' && (
                    <div className="space-y-2">
                      <p className="text-sm text-ink-3">
                        The daemon will wait for approval before starting this
                        attempt.
                      </p>
                      <button
                        type="button"
                        className="rounded-xl bg-accent px-4 py-2 text-sm text-accent-fg disabled:opacity-50"
                        disabled={busy || !!loadError}
                        onClick={() =>
                          void mutate(() =>
                            daemon.approveAgentJob(agentId, job.id, {
                              revision: job.revision,
                            }),
                          )
                        }
                      >
                        Approve and start
                      </button>
                    </div>
                  )}
                  {job.status === 'needs_review' && (
                    <p className="text-sm text-ink-3">
                      Execution was interrupted or its outcome is uncertain.
                      Review any external effects before retrying.
                    </p>
                  )}
                  {(job.status === 'queued' ||
                    job.status === 'awaiting_approval') && (
                    <button
                      type="button"
                      className="text-sm underline disabled:opacity-50"
                      disabled={busy || !!loadError}
                      onClick={() =>
                        void mutate(() =>
                          daemon.cancelAgentJob(agentId, job.id, {
                            revision: job.revision,
                          }),
                        )
                      }
                    >
                      {job.status === 'awaiting_approval'
                        ? 'Cancel proposal'
                        : 'Cancel queued run'}
                    </button>
                  )}
                  {(job.status === 'failed' ||
                    job.status === 'needs_review' ||
                    (job.status === 'completed' &&
                      job.attempts.find(
                        (attempt) => attempt.attempt === job.attempt,
                      )?.review?.decision === 'changes_requested')) &&
                    (job.attempt >= job.maxAttempts ? (
                      <p className="text-sm text-ink-3">
                        Attempt limit reached.
                      </p>
                    ) : (
                      <div className="space-y-3">
                        {job.requiresApproval && (
                          <p className="text-sm text-ink-3">
                            Retrying saves a new proposal that requires approval
                            before it starts.
                          </p>
                        )}
                        {job.status === 'needs_review' && (
                          <label className="flex items-start gap-2 text-sm">
                            <input
                              type="checkbox"
                              checked={!!acknowledged[job.id]}
                              onChange={(event) =>
                                setAcknowledged((current) => ({
                                  ...current,
                                  [job.id]: event.target.checked,
                                }))
                              }
                            />
                            I understand retrying may repeat external actions or
                            duplicate side effects.
                          </label>
                        )}
                        <button
                          type="button"
                          className="text-sm underline disabled:opacity-50"
                          disabled={
                            busy ||
                            !!loadError ||
                            (job.status === 'needs_review' &&
                              !acknowledged[job.id])
                          }
                          onClick={() =>
                            void mutate(() =>
                              daemon.retryAgentJob(agentId, job.id, {
                                revision: job.revision,
                                acknowledgeUncertain: !!acknowledged[job.id],
                              }),
                            )
                          }
                        >
                          Retry run
                        </button>
                      </div>
                    ))}
                </article>
              ))}
          </div>
        </>
      )}
    </fieldset>
  );
}
