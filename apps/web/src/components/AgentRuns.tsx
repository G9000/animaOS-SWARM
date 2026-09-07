import { useEffect, useRef, useState } from 'react';
import { DaemonHttpError, type AgentJob } from '@animaOS-SWARM/sdk';
import { daemon } from '../lib/daemon-api';

type Draft = { title: string; prompt: string; requestKey: string };
const labels: Record<AgentJob['status'], string> = {
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
      return saved;
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

export function AgentRunsView({ agentId }: { agentId: string }) {
  return <AgentRuns key={agentId} agentId={agentId} />;
}

function AgentRuns({ agentId }: { agentId: string }) {
  const [draft, setDraft] = useState(() => readDraft(agentId));
  const draftRef = useRef(draft);
  const [jobs, setJobs] = useState<AgentJob[] | null>(null);
  const [loadError, setLoadError] = useState('');
  const [actionError, setActionError] = useState('');
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const [refresh, setRefresh] = useState(0);
  const [pollingPaused, setPollingPaused] = useState(false);
  const [acknowledged, setAcknowledged] = useState<Record<string, boolean>>({});
  const live = useRef(true);
  useEffect(() => {
    live.current = true;
    return () => {
      live.current = false;
    };
  }, []);

  useEffect(() => {
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
  }, [agentId, refresh]);

  function change(patch: Partial<Pick<Draft, 'title' | 'prompt'>>) {
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
    if (busyRef.current) return;
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
      }
    } finally {
      busyRef.current = false;
      if (live.current) setBusy(false);
    }
  }

  return (
    <div className="space-y-6">
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
        <button
          type="submit"
          className="rounded-xl bg-accent px-4 py-3 text-sm text-accent-fg disabled:opacity-50"
          disabled={busy || !draft.title.trim() || !draft.prompt.trim()}
        >
          Queue run
        </button>
        {busy && (
          <p role="status" className="text-sm">
            Saving run…
          </p>
        )}
      </form>
      {actionError && (
        <p role="alert" className="text-sm text-danger">
          {actionError}
        </p>
      )}
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
      {jobs === null && !loadError && <p role="status">Loading runs…</p>}
      {jobs?.length === 0 && !loadError && (
        <p className="text-sm text-ink-3">No runs yet.</p>
      )}
      <div className="space-y-4">
        {jobs?.map((job) => (
          <article
            key={job.id}
            className="min-w-0 space-y-3 rounded-2xl border border-line p-5"
          >
            <div className="flex flex-wrap justify-between gap-2">
              <h4 className="break-words font-semibold">{job.title}</h4>
              <span className="text-sm">{labels[job.status]}</span>
            </div>
            <p className="text-xs text-ink-3">
              Attempt {job.attempt} of 3 · Updated{' '}
              {new Date(job.updatedAtMs).toLocaleString()}
            </p>
            <details>
              <summary className="cursor-pointer text-sm">Assignment</summary>
              <p className="whitespace-pre-wrap break-words text-sm">
                {job.prompt}
              </p>
            </details>
            {job.result !== null && (
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
            {job.status === 'needs_review' && (
              <p className="text-sm text-ink-3">
                Execution was interrupted or its outcome is uncertain. Review
                any external effects before retrying.
              </p>
            )}
            {job.status === 'queued' && (
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
                Cancel queued run
              </button>
            )}
            {(job.status === 'failed' || job.status === 'needs_review') &&
              (job.attempt >= 3 ? (
                <p className="text-sm text-ink-3">Attempt limit reached.</p>
              ) : (
                <div className="space-y-3">
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
                      (job.status === 'needs_review' && !acknowledged[job.id])
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
    </div>
  );
}
