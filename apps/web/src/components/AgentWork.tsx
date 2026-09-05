import { useEffect, useState } from 'react';
import type { AgentTask, AgentTasks } from '@animaOS-SWARM/sdk';
import { daemon, type DaemonSchedule } from '../lib/daemon-api';
import { primaryBtnCls } from './ui-bits';

export function AgentTasksView({
  agentId,
  name,
  running = false,
  onBusyChange,
}: {
  agentId: string;
  name: string;
  running?: boolean;
  onBusyChange?: (busy: boolean) => void;
}) {
  const [snapshot, setSnapshot] = useState<AgentTasks | null>(null);
  const [tasks, setTasks] = useState<AgentTask[]>([]);
  const [content, setContent] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [reload, setReload] = useState(0);
  useEffect(() => {
    let cancelled = false;
    setSnapshot(null);
    setError(null);
    daemon
      .agentTasks(agentId)
      .then((value) => {
        if (!cancelled) {
          setSnapshot(value);
          setTasks(value.tasks);
        }
      })
      .catch((e) => {
        if (!cancelled) setError(String(e.message ?? e));
      });
    return () => {
      cancelled = true;
    };
  }, [agentId, reload]);
  const dirty =
    snapshot && JSON.stringify(tasks) !== JSON.stringify(snapshot.tasks);
  const save = async () => {
    if (!snapshot || busy || running) return;
    setBusy(true);
    onBusyChange?.(true);
    setError(null);
    try {
      const saved = await daemon.updateAgentTasks(agentId, {
        tasks,
        revision: snapshot.revision,
      });
      setSnapshot(saved);
      setTasks(saved.tasks);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
      onBusyChange?.(false);
    }
  };
  return (
    <section className="space-y-5" aria-label={`${name}'s tasks`}>
      <div className="flex items-center justify-between gap-3">
        <h4 className="font-semibold">Tasks for {name}</h4>
        <button
          type="button"
          disabled={busy}
          className="text-xs underline"
          onClick={() => setReload((value) => value + 1)}
        >
          Refresh tasks
        </button>
      </div>
      <p className="text-sm leading-relaxed text-ink-3">
        This is {name}’s own task list. Their todo tools read and update this
        same list. Add a proactive schedule separately to have them check it
        automatically.
      </p>
      {error && (
        <p role="alert" className="text-sm text-danger">
          {error}
        </p>
      )}
      {running && (
        <p role="status" className="text-sm text-ink-3">
          {name} is working. You can draft tasks here; wait for the run to
          finish before saving.
        </p>
      )}
      {!snapshot && !error && <p role="status">Loading tasks…</p>}
      {snapshot && (
        <>
          <form
            className="space-y-3"
            onSubmit={(event) => {
              event.preventDefault();
              if (!content.trim()) return;
              setTasks((current) => [
                ...current,
                {
                  content: content.trim(),
                  activeForm: content.trim(),
                  status: 'pending',
                },
              ]);
              setContent('');
            }}
          >
            <textarea
              className="field"
              aria-label="New agent task"
              placeholder="What should this agent work on? Include the details they need."
              value={content}
              disabled={busy}
              onChange={(event) => setContent(event.target.value)}
              rows={3}
            />
            <button
              className="rounded-lg border border-line px-4 py-2 text-sm"
              disabled={busy || !content.trim()}
            >
              Add task
            </button>
          </form>
          {!tasks.length && (
            <p className="rounded-xl border border-line p-5 text-sm text-ink-3">
              No tasks yet for {name}.
            </p>
          )}
          {tasks.map((task, index) => (
            <article
              className="space-y-3 rounded-xl border border-line p-4"
              key={index}
            >
              <textarea
                aria-label={`Task ${index + 1}`}
                className="field"
                rows={2}
                disabled={busy}
                value={task.content}
                onChange={(event) =>
                  setTasks((current) =>
                    current.map((item, i) =>
                      i === index
                        ? {
                            ...item,
                            content: event.target.value,
                            activeForm: event.target.value,
                          }
                        : item,
                    ),
                  )
                }
              />
              <div className="flex items-center justify-between gap-3">
                <select
                  aria-label={`Task ${index + 1} status`}
                  className="field w-auto text-sm"
                  value={task.status}
                  disabled={busy}
                  onChange={(event) =>
                    setTasks((current) =>
                      current.map((item, i) =>
                        i === index
                          ? {
                              ...item,
                              status: event.target.value as AgentTask['status'],
                            }
                          : item,
                      ),
                    )
                  }
                >
                  <option value="pending">Pending</option>
                  <option value="in_progress">In progress</option>
                  <option value="completed">Completed</option>
                </select>
                <button
                  type="button"
                  aria-label={`Remove task ${index + 1}`}
                  className="text-xs text-danger"
                  disabled={busy}
                  onClick={() =>
                    setTasks((current) => current.filter((_, i) => i !== index))
                  }
                >
                  Remove
                </button>
              </div>
            </article>
          ))}
          <button
            type="button"
            className={`${primaryBtnCls} w-full`}
            disabled={
              busy ||
              running ||
              !dirty ||
              tasks.some((task) => !task.content.trim())
            }
            onClick={() => void save()}
          >
            {busy ? 'Saving tasks…' : dirty ? 'Save tasks' : 'Tasks saved'}
          </button>
          {dirty && (
            <p className="text-xs text-ink-3">
              Unsaved changes. Refresh reloads the saved list.
            </p>
          )}
        </>
      )}
    </section>
  );
}

export function AgentProactiveView({
  agentId,
  name,
  onBusyChange,
}: {
  agentId: string;
  name: string;
  onBusyChange?: (busy: boolean) => void;
}) {
  const [schedules, setSchedules] = useState<DaemonSchedule[] | null>(null);
  const [prompt, setPrompt] = useState('');
  const [minutes, setMinutes] = useState(60);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [reload, setReload] = useState(0);
  useEffect(() => {
    let cancelled = false;
    setError(null);
    daemon
      .listSchedules(agentId)
      .then((result) => {
        if (!cancelled) setSchedules(result.schedules);
      })
      .catch((e) => {
        if (!cancelled) setError(String(e.message ?? e));
      });
    return () => {
      cancelled = true;
    };
  }, [agentId, reload]);
  const mutate = async (action: () => Promise<unknown>, created = false) => {
    if (busy) return;
    setBusy(true);
    onBusyChange?.(true);
    setError(null);
    try {
      await action();
      if (created) setPrompt('');
      setReload((value) => value + 1);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
      onBusyChange?.(false);
    }
  };
  return (
    <section className="space-y-5" aria-label={`${name}'s proactive settings`}>
      <div className="flex items-center justify-between">
        <h4 className="font-semibold">Proactive work for {name}</h4>
        <button
          type="button"
          disabled={busy}
          className="text-xs underline"
          onClick={() => setReload((value) => value + 1)}
        >
          Refresh schedules
        </button>
      </div>
      <p className="text-sm leading-relaxed text-ink-3">
        Choose when {name} wakes up and what they should do. Schedules belong
        only to this agent and run while the daemon is active, even with the UI
        closed. Their existing tool permissions still apply.
      </p>
      {error && (
        <p role="alert" className="text-sm text-danger">
          {error}
        </p>
      )}
      {schedules === null && !error && <p role="status">Loading schedules…</p>}
      {schedules?.length === 0 && (
        <p className="rounded-xl border border-line p-4 text-sm text-ink-3">
          Proactive work is off. No schedules configured for {name}.
        </p>
      )}
      {schedules?.map((schedule) => (
        <article
          key={schedule.id}
          className="space-y-3 rounded-xl border border-line p-4"
        >
          <p className="whitespace-pre-wrap break-words text-sm">
            {schedule.prompt}
          </p>
          <p className="text-xs text-ink-3">
            {schedule.enabled
              ? `Next run: ${new Date(schedule.nextDueAtMs).toLocaleString()}`
              : 'Paused'}{' '}
            ·{' '}
            {schedule.trigger.type === 'interval'
              ? `Every ${schedule.trigger.intervalMs / 60000} minutes`
              : `Daily ${schedule.trigger.hour}:${String(schedule.trigger.minute).padStart(2, '0')} ${schedule.trigger.timeZone}`}
          </p>
          {schedule.lastOutcome && (
            <p className="text-xs text-ink-3">
              Last result:{' '}
              {schedule.lastOutcome.status === 'silent'
                ? 'No update needed'
                : schedule.lastOutcome.status === 'spoke'
                  ? 'Posted an update'
                  : 'Run failed'}
              {schedule.lastOutcome.errorCode
                ? ` (${schedule.lastOutcome.errorCode})`
                : ''}
            </p>
          )}
          <div className="flex gap-4">
            <button
              type="button"
              className="text-sm underline"
              disabled={busy}
              onClick={() =>
                void mutate(() =>
                  daemon.updateSchedule(agentId, schedule.id, {
                    enabled: !schedule.enabled,
                  }),
                )
              }
            >
              {schedule.enabled ? 'Pause' : 'Resume'}
            </button>
            <button
              type="button"
              className="text-sm text-danger"
              disabled={busy}
              onClick={() =>
                void mutate(() => daemon.deleteSchedule(agentId, schedule.id))
              }
            >
              Remove schedule
            </button>
          </div>
        </article>
      ))}
      <form
        className="space-y-4 border-t border-line pt-5"
        onSubmit={(event) => {
          event.preventDefault();
          if (!prompt.trim() || !Number.isInteger(minutes) || minutes < 1)
            return;
          void mutate(
            () =>
              daemon.createSchedule(agentId, {
                prompt: prompt.trim(),
                trigger: { type: 'interval', intervalMs: minutes * 60000 },
                target: { type: 'workspace' },
                enabled: true,
              }),
            true,
          );
        }}
      >
        <label className="block text-sm">
          What should they do?
          <textarea
            aria-label="Proactive instructions"
            rows={5}
            className="field mt-2"
            value={prompt}
            disabled={busy}
            onChange={(event) => setPrompt(event.target.value)}
            placeholder="Describe the goal, what to check, and when to report back."
          />
        </label>
        <button
          type="button"
          className="text-xs underline"
          disabled={busy}
          onClick={() =>
            setPrompt(
              'Read your own task list. Work on the next clear, pending task within your existing permissions and agreed scope. Update task progress with todo_write. Ask before consequential external actions. Report meaningful progress or blockers; otherwise return CHECKIN_OK.',
            )
          }
        >
          Use my task list
        </button>
        <label className="flex items-center gap-3 text-sm">
          Every{' '}
          <input
            aria-label="Proactive interval minutes"
            type="number"
            min={1}
            step={1}
            className="field w-24"
            value={minutes}
            disabled={busy}
            onChange={(event) => setMinutes(Number(event.target.value))}
          />{' '}
          minutes
        </label>
        <p className="text-xs text-ink-3">
          Updates appear in this agent’s workspace conversation. Scheduled runs
          use your configured model.
        </p>
        <button
          className={`${primaryBtnCls} w-full`}
          disabled={
            busy || !prompt.trim() || !Number.isInteger(minutes) || minutes < 1
          }
        >
          {busy ? 'Saving…' : 'Enable schedule'}
        </button>
      </form>
    </section>
  );
}
