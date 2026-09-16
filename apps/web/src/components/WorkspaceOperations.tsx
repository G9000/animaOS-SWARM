import { useEffect, useState } from 'react';
import type { AgentJob } from '@animaOS-SWARM/sdk';
import { daemon } from '../lib/daemon-api';
import { AgentRunsView } from './AgentRuns';

type Agent = { id: string; name: string };
type Selection =
  | { agentId: string; jobId: string }
  | { agentId: string; jobId?: undefined };
const filters = [
  'All',
  'Approval',
  'Active',
  'Review',
  'Completed',
  'Failed',
  'Cancelled',
] as const;
type Filter = (typeof filters)[number];
function review(job: AgentJob) {
  return job.attempts.find((attempt) => attempt.attempt === job.attempt)
    ?.review;
}
function matches(job: AgentJob, filter: Filter) {
  switch (filter) {
    case 'All':
      return true;
    case 'Approval':
      return job.status === 'awaiting_approval';
    case 'Active':
      return job.status === 'queued' || job.status === 'running';
    case 'Review':
      return (
        job.status === 'needs_review' ||
        (job.status === 'completed' && !review(job))
      );
    case 'Completed':
      return job.status === 'completed' && review(job)?.decision === 'accepted';
    case 'Failed':
      return (
        job.status === 'failed' || review(job)?.decision === 'changes_requested'
      );
    case 'Cancelled':
      return job.status === 'cancelled';
  }
}
function status(job: AgentJob) {
  if (job.status === 'completed') {
    return review(job)?.decision === 'accepted'
      ? 'Accepted'
      : review(job)?.decision === 'changes_requested'
        ? 'Changes requested'
        : 'Awaiting review';
  }
  return {
    awaiting_approval: 'Awaiting approval',
    queued: 'Queued',
    running: 'Running',
    failed: 'Failed',
    needs_review: 'Outcome uncertain',
    cancelled: 'Cancelled',
  }[job.status];
}

export function WorkspaceOperations({
  agents,
  online,
  onChat,
}: {
  agents: readonly Agent[];
  online: boolean;
  onChat: (agentId: string) => void;
}) {
  const [byAgent, setByAgent] = useState<Record<string, AgentJob[]>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [loading, setLoading] = useState(online);
  const [paused, setPaused] = useState(false);
  const [refresh, setRefresh] = useState(0);
  const [filter, setFilter] = useState<Filter>('All');
  const [query, setQuery] = useState('');
  const [owner, setOwner] = useState('');
  const [selection, setSelection] = useState<Selection | null>(null);
  const agentKey = JSON.stringify(agents.map((agent) => agent.id));
  useEffect(() => {
    const ids: string[] = JSON.parse(agentKey);
    setOwner((current) => (ids.includes(current) ? current : ''));
  }, [agentKey]);
  useEffect(() => {
    if (!online) {
      setLoading(false);
      return;
    }
    const ids: string[] = JSON.parse(agentKey);
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let remaining = 30;
    setLoading(true);
    setPaused(false);
    async function load() {
      const results = await Promise.allSettled(
        ids.map((id) => daemon.agentJobs(id, { signal: controller.signal })),
      );
      if (controller.signal.aborted) return;
      const updates: Record<string, AgentJob[]> = {};
      const failures: Record<string, string> = {};
      results.forEach((result, index) => {
        if (result.status === 'fulfilled') updates[ids[index]] = result.value;
        else
          failures[ids[index]] =
            result.reason instanceof Error
              ? result.reason.message
              : 'Could not load assignments';
      });
      setByAgent((current) => ({ ...current, ...updates }));
      setErrors(failures);
      setLoading(false);
      if (--remaining > 0) timer = setTimeout(() => void load(), 10000);
      else setPaused(true);
    }
    void load();
    return () => {
      controller.abort();
      clearTimeout(timer);
    };
  }, [agentKey, online, refresh]);

  const jobs = agents
    .flatMap((agent) => byAgent[agent.id] ?? [])
    .sort((a, b) => b.updatedAtMs - a.updatedAtMs || a.id.localeCompare(b.id));
  const scoped = jobs.filter((job) => !owner || job.agentId === owner);
  const visible = scoped.filter(
    (job) =>
      matches(job, filter) &&
      `${job.title} ${job.id} ${job.prompt} ${agents.find((a) => a.id === job.agentId)?.name ?? ''}`
        .toLowerCase()
        .includes(query.toLowerCase()),
  );
  const selectedAgent = agents.find((agent) => agent.id === selection?.agentId);
  const selectedJob = jobs.find(
    (job) => job.id === selection?.jobId && job.agentId === selection?.agentId,
  );
  const incomplete = Object.keys(errors).length > 0;
  const update = (job?: AgentJob) => {
    if (job) {
      setByAgent((current) => ({
        ...current,
        [job.agentId]: [
          job,
          ...(current[job.agentId] ?? []).filter((item) => item.id !== job.id),
        ],
      }));
      setSelection({ agentId: job.agentId, jobId: job.id });
    }
    setRefresh((value) => value + 1);
  };

  return (
    <section className="operator-console" aria-label="Operations">
      <div className="operator-heading">
        <div>
          <p className="operator-eyebrow">OPERATOR CONSOLE</p>
          <h1>Operations</h1>
          <p>Dispatch assignments. Follow execution. Review results.</p>
        </div>
        <div className="operator-actions">
          <button
            className="operator-button"
            disabled={!online || loading}
            onClick={() => setRefresh((value) => value + 1)}
          >
            Refresh queue
          </button>
          <button
            className="operator-button is-primary"
            disabled={!online || !agents.length}
            onClick={() =>
              setSelection({
                agentId:
                  agents.find((agent) => agent.id === owner)?.id ??
                  agents[0].id,
              })
            }
          >
            New assignment
          </button>
        </div>
      </div>
      {!online && (
        <p className="operator-notice" role="status">
          Offline. Reconnect to dispatch or update assignments. Saved rows may
          be outdated.
        </p>
      )}
      {incomplete && (
        <div className="operator-notice" role="alert">
          Queue incomplete.{' '}
          {agents
            .filter((a) => errors[a.id])
            .map((a) => `${a.name}: ${errors[a.id]}`)
            .join(' · ')}
          . Retained assignments may be outdated.
        </div>
      )}
      {paused && (
        <p className="operator-notice">
          Automatic refresh paused after five minutes. Refresh queue to resume.
        </p>
      )}
      <div className="operator-filters" aria-label="Assignment status filters">
        {filters.map((item) => (
          <button
            key={item}
            aria-pressed={filter === item}
            onClick={() => setFilter(item)}
          >
            {item}
            <span>
              {loading && !jobs.length
                ? '—'
                : scoped.filter((job) => matches(job, item)).length}
              {incomplete ? '+' : ''}
            </span>
          </button>
        ))}
      </div>
      <div className={`operator-split ${selection ? 'has-selection' : ''}`}>
        <section className="operator-queue" aria-label="Assignment queue">
          <div className="operator-search">
            <input
              aria-label="Search assignments"
              placeholder="Search assignments…"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
            />
            <select
              aria-label="Filter by agent"
              value={owner}
              onChange={(event) => setOwner(event.target.value)}
            >
              <option value="">All agents</option>
              {agents.map((agent) => (
                <option key={agent.id} value={agent.id}>
                  {agent.name}
                </option>
              ))}
            </select>
          </div>
          <div className="operator-queue-label">
            <span>ASSIGNMENT / AGENT</span>
            <span>{visible.length} shown</span>
          </div>
          <div className="operator-rows">
            {loading && !jobs.length ? (
              <p className="operator-empty" role="status">
                Loading assignments…
              </p>
            ) : !visible.length ? (
              <div className="operator-empty">
                <h2>
                  {jobs.length
                    ? 'No matching assignments'
                    : incomplete || !online
                      ? 'Queue unavailable'
                      : 'No assignments yet'}
                </h2>
                <p>
                  {jobs.length
                    ? 'Try another status or search.'
                    : 'Create an assignment with an objective and an agent to begin.'}
                </p>
              </div>
            ) : (
              visible.map((job) => (
                <button
                  className="operator-row"
                  key={`${job.agentId}:${job.id}`}
                  aria-pressed={
                    selection?.jobId === job.id &&
                    selection.agentId === job.agentId
                  }
                  onClick={() =>
                    setSelection({ agentId: job.agentId, jobId: job.id })
                  }
                >
                  <span className="operator-row-top">
                    <code>{job.id.slice(0, 12)}</code>
                    <span
                      className="operator-status"
                      data-status={job.status}
                      data-review={review(job)?.decision ?? 'pending'}
                    >
                      {status(job)}
                    </span>
                  </span>
                  <strong>{job.title}</strong>
                  <span className="operator-row-meta">
                    {agents.find((agent) => agent.id === job.agentId)?.name} ·
                    Attempt {job.attempt}/{job.maxAttempts}
                    {errors[job.agentId] ? ' · Stale' : ''}
                  </span>
                </button>
              ))
            )}
          </div>
          <footer className="operator-queue-footer">
            {agents.length} agents ·{' '}
            {online
              ? paused
                ? 'Refresh paused'
                : 'Refreshes every 10s'
              : 'Offline'}
          </footer>
        </section>
        <section className="operator-detail" aria-label="Assignment detail">
          {selection ? (
            <>
              <div className="operator-detail-toolbar">
                <button
                  className="operator-button"
                  onClick={() => setSelection(null)}
                >
                  Back to queue
                </button>
                <span>{selectedAgent?.name ?? 'Agent unavailable'}</span>
                {selectedAgent && (
                  <button
                    className="operator-button"
                    onClick={() => onChat(selectedAgent.id)}
                  >
                    Message agent
                  </button>
                )}
              </div>
              {!selectedAgent ? (
                <p className="operator-empty">
                  This agent is no longer in the workspace. Select another
                  assignment.
                </p>
              ) : (
                <>
                  {!selection.jobId ? (
                    <div className="operator-dispatch-heading">
                      <h2>New assignment</h2>
                      <p>Define the objective and dispatch it to an agent.</p>
                      <label>
                        Assign to
                        <select
                          value={selection.agentId}
                          onChange={(event) =>
                            setSelection({ agentId: event.target.value })
                          }
                        >
                          {agents.map((agent) => (
                            <option key={agent.id} value={agent.id}>
                              {agent.name}
                            </option>
                          ))}
                        </select>
                      </label>
                    </div>
                  ) : (
                    <div className="operator-record-meta">
                      <span>ASSIGNMENT</span>
                      <code>{selection.jobId}</code>
                      {selectedJob && (
                        <span>
                          Created{' '}
                          {new Date(selectedJob.createdAtMs).toLocaleString()}
                        </span>
                      )}
                    </div>
                  )}
                  <AgentRunsView
                    key={`${selection.agentId}:${selection.jobId ?? 'new'}`}
                    agentId={selection.agentId}
                    jobId={selection.jobId}
                    snapshot={selectedJob}
                    refreshKey={refresh}
                    composeOnly={!selection.jobId}
                    online={online}
                    onChanged={update}
                  />
                </>
              )}
            </>
          ) : (
            <div className="operator-detail-empty">
              <span className="operator-empty-symbol" aria-hidden>
                ≡
              </span>
              <h2>Select an assignment</h2>
              <p>
                Its objective, execution history, saved output, and next actions
                appear here.
              </p>
              <div className="operator-lifecycle">
                <span>01 · Dispatch</span>
                <span>02 · Execute</span>
                <span>03 · Review</span>
              </div>
            </div>
          )}
        </section>
      </div>
    </section>
  );
}
