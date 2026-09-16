import { useEffect, useRef, useState } from 'react';
import {
  DaemonHttpError,
  type AgentJob,
  type GoalInput,
  type GoalView,
  type GoalStatus,
} from '@animaOS-SWARM/sdk';
import { daemon } from '../lib/daemon-api';
const fresh = (): GoalInput => ({
  title: '',
  objective: '',
  requestKey: crypto.randomUUID(),
  maxAttempts: 10,
});
const draftKey = 'anima:goal-draft';
function readDraft(): GoalInput {
  try {
    const value = JSON.parse(sessionStorage.getItem(draftKey) ?? 'null');
    if (
      value &&
      typeof value.title === 'string' &&
      typeof value.objective === 'string' &&
      typeof value.requestKey === 'string' &&
      Number.isInteger(value.maxAttempts) &&
      value.maxAttempts >= 1 &&
      value.maxAttempts <= 100
    )
      return value;
  } catch {
    /* Browser storage is optional. */
  }
  return fresh();
}
function saveDraft(value: GoalInput) {
  try {
    sessionStorage.setItem(draftKey, JSON.stringify(value));
  } catch {
    /* Retain the in-memory draft. */
  }
}
const errorText = (error: unknown) =>
  error instanceof Error ? error.message : String(error);
export function WorkspaceGoals({
  agents,
}: {
  agents: readonly { id: string; name: string }[];
}) {
  const [goals, setGoals] = useState<GoalView[] | null>(null);
  const [selected, setSelected] = useState('');
  const [outputs, setOutputs] = useState<AgentJob[] | null>(null);
  const [loadError, setLoadError] = useState('');
  const [outputError, setOutputError] = useState('');
  const [actionError, setActionError] = useState('');
  const [refresh, setRefresh] = useState(0);
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const live = useRef(true);
  const [draft, setDraft] = useState(readDraft);
  const draftRef = useRef(draft);
  useEffect(() => {
    live.current = true;
    return () => {
      live.current = false;
    };
  }, []);
  useEffect(() => {
    const controller = new AbortController();
    void daemon
      .goals({ signal: controller.signal })
      .then((value) => {
        if (!controller.signal.aborted) {
          setGoals(value);
          setLoadError('');
        }
      })
      .catch((error) => {
        if (!controller.signal.aborted) setLoadError(errorText(error));
      });
    return () => controller.abort();
  }, [refresh]);
  useEffect(() => {
    const controller = new AbortController();
    setOutputs(null);
    setOutputError('');
    if (selected)
      void daemon
        .goalJobs(selected, { signal: controller.signal })
        .then((value) => {
          if (!controller.signal.aborted) setOutputs(value);
        })
        .catch((error) => {
          if (!controller.signal.aborted) setOutputError(errorText(error));
        });
    return () => controller.abort();
  }, [selected, refresh]);
  const goal = goals?.find((value) => value.id === selected);
  function change(patch: Partial<GoalInput>) {
    const next = {
      ...draftRef.current,
      ...patch,
      requestKey: crypto.randomUUID(),
    };
    draftRef.current = next;
    saveDraft(next);
    setDraft(next);
  }
  async function mutate(
    operation: () => Promise<GoalView>,
    submitted?: GoalInput,
  ) {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setActionError('');
    try {
      const value = await operation();
      if (!live.current) return;
      setGoals((current) => [
        value,
        ...(current ?? []).filter((item) => item.id !== value.id),
      ]);
      setOutputs(null);
      setOutputError('');
      setSelected(value.id);
      if (submitted && draftRef.current.requestKey === submitted.requestKey) {
        const next = fresh();
        draftRef.current = next;
        saveDraft(next);
        setDraft(next);
      }
      setRefresh((value) => value + 1);
    } catch (error) {
      if (!live.current) return;
      setActionError(
        `${errorText(error)}. ${submitted ? 'Your draft is retained. Retry the same submission safely.' : 'Refresh goals before trying again.'}`,
      );
      if (error instanceof DaemonHttpError && error.status === 409)
        setRefresh((value) => value + 1);
    } finally {
      busyRef.current = false;
      if (live.current) setBusy(false);
    }
  }
  function status(next: GoalStatus) {
    if (goal)
      void mutate(() =>
        daemon.setGoalStatus(goal.id, {
          revision: goal.revision,
          status: next,
        }),
      );
  }
  const canComplete =
    !!outputs?.length &&
    outputs.every(
      (job) =>
        job.status === 'cancelled' ||
        (job.status === 'completed' &&
          job.attempts.find((attempt) => attempt.attempt === job.attempt)
            ?.review?.decision === 'accepted'),
    ) &&
    outputs.some(
      (job) =>
        job.status === 'completed' &&
        job.attempts.find((attempt) => attempt.attempt === job.attempt)?.review
          ?.decision === 'accepted',
    );
  return (
    <div className="space-y-6">
      <p className="text-sm text-ink-3">
        Give related assignments a shared objective and attempt budget. Create a
        goal here, then link assignments from Runs. Creating a goal does not
        start work.
      </p>
      <form
        className="space-y-4 rounded-2xl border border-line p-5"
        onSubmit={(event) => {
          event.preventDefault();
          if (
            !draft.title.trim() ||
            !draft.objective.trim() ||
            !Number.isInteger(draft.maxAttempts) ||
            draft.maxAttempts < 1 ||
            draft.maxAttempts > 100
          )
            return;
          saveDraft(draft);
          void mutate(() => daemon.createGoal(draft), draft);
        }}
      >
        <label className="block space-y-2 text-sm">
          Goal title
          <input
            className="field"
            required
            maxLength={160}
            value={draft.title}
            onChange={(event) => change({ title: event.target.value })}
          />
        </label>
        <label className="block space-y-2 text-sm">
          Objective
          <textarea
            className="field min-h-24"
            required
            value={draft.objective}
            onChange={(event) => change({ objective: event.target.value })}
          />
        </label>
        <label className="block space-y-2 text-sm">
          Total attempt budget
          <input
            className="field"
            type="number"
            min={1}
            max={100}
            step={1}
            required
            value={draft.maxAttempts}
            onChange={(event) =>
              change({ maxAttempts: Number(event.target.value) })
            }
          />
        </label>
        <p className="text-xs text-ink-3">
          Each started attempt counts, including failed or uncertain runs.
          Queued runs reserve an attempt. This does not limit tokens or
          spending.
        </p>
        <button
          type="submit"
          className="rounded-xl bg-accent px-4 py-3 text-sm text-accent-fg disabled:opacity-50"
          disabled={busy || !draft.title.trim() || !draft.objective.trim()}
        >
          Create goal
        </button>
      </form>
      {actionError && (
        <p role="alert" className="text-sm text-danger">
          {actionError}
        </p>
      )}
      <div className="flex items-center justify-between gap-3">
        <h3 className="font-semibold">Goals</h3>
        <button
          type="button"
          className="rounded-xl border border-line px-4 py-2 text-sm"
          disabled={busy}
          onClick={() => setRefresh((value) => value + 1)}
        >
          Refresh goals
        </button>
      </div>
      {loadError && (
        <p role="alert" className="text-sm text-danger">
          Could not load goals: {loadError}.{' '}
          {goals
            ? 'Saved counts may be outdated.'
            : 'Goal list is unavailable.'}
        </p>
      )}
      {goals === null && !loadError && <p role="status">Loading goals…</p>}
      {goals?.length === 0 && !loadError && <p>No goals yet.</p>}
      <div className="grid gap-3 sm:grid-cols-2">
        {goals?.map((item) => (
          <article
            key={item.id}
            className={`min-w-0 space-y-2 rounded-xl border p-4 ${selected === item.id ? 'border-accent' : 'border-line'}`}
          >
            <button
              type="button"
              className="break-words text-left font-semibold underline"
              aria-pressed={selected === item.id}
              onClick={() => {
                if (item.id !== selected) {
                  setOutputs(null);
                  setOutputError('');
                  setSelected(item.id);
                }
              }}
            >
              {item.title}
            </button>
            <p className="text-sm capitalize">{item.status}</p>
            <p className="text-xs text-ink-3">
              {item.remainingAttempts} of {item.maxAttempts} attempts available
              · {item.jobCount} runs
            </p>
          </article>
        ))}
      </div>
      {goal && (
        <section
          aria-label="Selected goal"
          className="min-w-0 space-y-4 rounded-2xl border border-line p-5"
        >
          <h3 className="break-words text-lg font-semibold">{goal.title}</h3>
          <p className="whitespace-pre-wrap break-words text-sm">
            {goal.objective}
          </p>
          <div className="flex flex-wrap gap-3 text-sm">
            <span>{goal.consumedAttempts} consumed</span>
            <span>{goal.reservedAttempts} reserved</span>
            <span>{goal.remainingAttempts} remaining</span>
            <span>{goal.acceptedOutputs} accepted outputs</span>
          </div>
          {goal.status !== 'completed' && (
            <div className="space-y-2">
              <div className="flex flex-wrap gap-4">
                <button
                  type="button"
                  className="text-sm underline disabled:opacity-50"
                  disabled={busy || !!loadError}
                  onClick={() =>
                    status(goal.status === 'active' ? 'paused' : 'active')
                  }
                >
                  {goal.status === 'active' ? 'Pause goal' : 'Resume goal'}
                </button>
                <button
                  type="button"
                  className="text-sm underline disabled:opacity-50"
                  disabled={
                    busy || !!loadError || !!outputError || !canComplete
                  }
                  onClick={() => status('completed')}
                >
                  Complete goal
                </button>
              </div>
              <p className="text-xs text-ink-3">
                Pausing holds queued work; running attempts can finish.
                Completion requires accepted results and no unfinished work.
              </p>
            </div>
          )}
          <h4 className="font-semibold">Linked runs and saved outputs</h4>
          {outputError && (
            <p role="alert" className="text-sm text-danger">
              Could not load goal outputs: {outputError}. Refresh goals to
              retry.
            </p>
          )}
          {outputs === null && !outputError && (
            <p role="status">Loading goal outputs…</p>
          )}
          {outputs?.length === 0 && !outputError && <p>No linked runs yet.</p>}
          {outputs?.map((job) => (
            <article
              key={job.id}
              className="min-w-0 space-y-2 rounded-xl border border-line p-4"
            >
              <h5 className="break-words font-semibold">{job.title}</h5>
              <p className="text-sm text-ink-3">
                {agents.find((agent) => agent.id === job.agentId)?.name ??
                  job.agentId}{' '}
                · {job.status.replace(/_/g, ' ')} · Attempt {job.attempt} of{' '}
                {job.maxAttempts}
              </p>
              {job.attempts.map((attempt) => (
                <details key={attempt.attempt} className="space-y-2">
                  <summary className="cursor-pointer text-sm">
                    Saved attempt {attempt.attempt} ·{' '}
                    {attempt.review?.decision.replace(/_/g, ' ') ??
                      attempt.status.replace(/_/g, ' ')}
                  </summary>
                  {attempt.result !== null && (
                    <pre className="max-h-80 overflow-auto whitespace-pre-wrap break-words font-sans text-sm">
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
                  {attempt.review?.note && (
                    <p className="whitespace-pre-wrap break-words text-sm">
                      Feedback: {attempt.review.note}
                    </p>
                  )}
                </details>
              ))}
            </article>
          ))}
        </section>
      )}
    </div>
  );
}
