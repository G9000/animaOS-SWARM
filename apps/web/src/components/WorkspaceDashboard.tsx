import { useEffect, useState } from 'react';
import type { AgentTask } from '@animaOS-SWARM/sdk';
import { daemon, type DaemonSchedule } from '../lib/daemon-api';
import type { AgentDetail } from '../lib/types';
import { AgentAvatar } from './AgentAvatar';
import { selectMainAgent } from '../lib/agent-access';

type Snapshot = {
  tasks: (AgentTask & { agentId: string })[];
  schedules: DaemonSchedule[];
  errors: string[];
  updatedAt: number;
};
const panel = 'rounded-2xl border border-line bg-surface p-5 sm:p-6';
const action =
  'rounded-xl border border-line px-3 py-2 text-sm font-medium hover:bg-accent/5';

export function WorkspaceDashboard({
  agents,
  companyName,
  mission,
  online,
  onChat,
  onOpenWork,
  onOpenTeam,
  managerId,
  onStartAssignment,
  onOpenFiles,
  onOpenCapabilities,
}: {
  agents: readonly AgentDetail[];
  companyName?: string | null;
  mission?: string;
  online: boolean;
  onChat(id: string): void;
  onOpenWork(section: 'Tasks' | 'Schedules'): void;
  onOpenTeam(): void;
  managerId?: string;
  onStartAssignment?(text: string): void;
  onOpenFiles?(): void;
  onOpenCapabilities?(): void;
}) {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [refresh, setRefresh] = useState(0);
  const [busy, setBusy] = useState(true);
  const agentIds = JSON.stringify(agents.map((agent) => agent.id).sort());
  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout>;
    setSnapshot(null);
    const ids = JSON.parse(agentIds) as string[];
    async function load() {
      if (!online) {
        setBusy(false);
        return;
      }
      setBusy(true);
      const results = await Promise.all(
        ids.map(async (id) => ({
          id,
          results: await Promise.allSettled([
            daemon.agentTasks(id),
            daemon.listSchedules(id),
          ] as const),
        })),
      );
      if (cancelled) return;
      const next: Snapshot = {
        tasks: [],
        schedules: [],
        errors: [],
        updatedAt: Date.now(),
      };
      for (const {
        id,
        results: [tasks, schedules],
      } of results) {
        if (tasks.status === 'fulfilled')
          next.tasks.push(
            ...tasks.value.tasks.map((task) => ({ ...task, agentId: id })),
          );
        else next.errors.push(`${id}:tasks`);
        if (schedules.status === 'fulfilled')
          next.schedules.push(...schedules.value.schedules);
        else next.errors.push(`${id}:schedules`);
      }
      setSnapshot(next);
      setBusy(false);
      timer = setTimeout(() => void load(), 30_000);
    }
    void load();
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [agentIds, online, refresh]);

  const owner = (id: string) =>
    agents.find((agent) => agent.id === id)?.name ?? 'Agent';
  const tasks =
    snapshot?.tasks.filter((task) => task.status !== 'completed') ?? [];
  const upcoming =
    snapshot?.schedules
      .filter((schedule) => schedule.enabled)
      .sort((a, b) => a.nextDueAtMs - b.nextDueAtMs) ?? [];
  const failed = agents.filter((agent) => agent.status === 'Failed');
  const scheduleErrors =
    snapshot?.schedules.filter(
      (schedule) => schedule.lastOutcome?.status === 'error',
    ) ?? [];
  const recent = agents
    .flatMap((agent) =>
      agent.messages
        .filter(
          (message) =>
            message.role === 'Assistant' && message.content.text.trim(),
        )
        .map((message) => ({ ...message, agentId: agent.id })),
    )
    .sort((a, b) => b.created_at_ms - a.created_at_ms)
    .slice(0, 5);
  const tasksIncomplete =
    !snapshot || snapshot.errors.some((error) => error.endsWith(':tasks'));
  const schedulesIncomplete =
    !snapshot || snapshot.errors.some((error) => error.endsWith(':schedules'));
  const manager = managerId
    ? agents.find((agent) => agent.id === managerId)
    : selectMainAgent(agents);
  const marker =
    'Prepared first assignment (do not start until the owner asks):\n';
  const assignment =
    manager?.system
      ?.split(marker)[1]
      ?.split('\n\nTeam responsibilities:')[0]
      ?.trim() ?? '';
  const canStart =
    online &&
    !!onStartAssignment &&
    !!assignment &&
    !!manager &&
    manager.status === 'Idle' &&
    !manager.messages.some((message) => message.role === 'User') &&
    !agents.some((agent) => agent.status === 'Running');
  const completed =
    snapshot?.tasks.filter((task) => task.status === 'completed') ?? [];
  const running = agents.filter((agent) => agent.status === 'Running');
  const goal = mission?.trim().split(/\n\s*\n/)[0].replace(/^Goal\s*\n/i, '').trim();
  const nextStep = !online
    ? 'Reconnect to see what your agency needs next.'
    : failed.length || scheduleErrors.length
      ? 'Review the reported failures with your manager.'
      : canStart
        ? 'Your first assignment is prepared. Start it when you are ready.'
        : running.length
          ? 'Your team is working. Follow their progress or ask your manager for an update.'
          : 'Ask your manager to turn your goal into the next useful piece of work.';

  return (
    <section
      aria-labelledby="dashboard-heading"
      className="h-full overflow-y-auto bg-[radial-gradient(ellipse_at_top_left,rgb(var(--color-accent-rgb)/0.05),transparent_65%)] p-5 pb-28 sm:p-8 md:pb-8"
    >
      <div className="mx-auto max-w-6xl space-y-6">
        <div className="flex flex-wrap items-start justify-between gap-4">
          <div className="max-w-2xl">
            <p className="text-xs font-medium uppercase tracking-widest text-accent">
              {companyName || 'Your workspace'} · A place to move work forward
            </p>
            <h2
              id="dashboard-heading"
              className="mt-2 font-display text-3xl font-semibold tracking-tight"
            >
              Overview
            </h2>
            <p className="mt-3 text-xs font-semibold uppercase tracking-widest text-ink-3">Your goal</p>
            <p className="mt-2 break-words font-display text-lg leading-relaxed text-ink-2 sm:text-xl">
              {goal ||
                'A clear view of your team, your work, and what comes next.'}
            </p>
            {mission && mission.trim() !== goal && (
              <details className="mt-3 text-sm text-ink-3">
                <summary className="cursor-pointer">Read workspace brief</summary>
                <p className="mt-3 whitespace-pre-wrap break-words leading-relaxed">{mission}</p>
              </details>
            )}
          </div>
          <div className="space-y-2 text-right">
            <button
              className={action}
              disabled={busy || !online}
              onClick={() => setRefresh((value) => value + 1)}
            >
              Refresh overview
            </button>
            <p className="text-xs text-ink-3" role="status">
              {!online
                ? 'Offline · reconnect to update'
                : busy
                  ? 'Updating…'
                  : snapshot
                    ? `Updated ${new Date(snapshot.updatedAt).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })} · refreshes every 30s`
                    : 'Waiting for data'}
            </p>
          </div>
        </div>

        <dl aria-label="Workplace at a glance" className="grid grid-cols-2 overflow-hidden rounded-2xl border border-line bg-surface sm:grid-cols-4">
          {[
            ['Team members', String(agents.length), 'People responsible for your work'],
            ['Working now', online ? String(running.length) : 'Unavailable', 'Live agent runs'],
            ['Open tasks', !online || tasksIncomplete ? 'Unavailable' : String(tasks.length), 'Owned tasks to move forward'],
            ['Enabled routines', !online || schedulesIncomplete ? 'Unavailable' : String(upcoming.length), 'Run while the daemon is active'],
          ].map(([label, value, description]) => (
            <div key={label} className="min-w-0 border-b border-line p-4 odd:border-r sm:border-b-0 sm:border-r sm:last:border-r-0 sm:p-5">
              <dt className="text-xs font-medium text-ink-3">{label}</dt>
              <dd className={`mt-2 font-display font-semibold ${value === 'Unavailable' ? 'text-base text-ink-3' : 'text-2xl'}`}>{value}</dd>
              <p className="mt-1 text-xs leading-relaxed text-ink-3">{description}</p>
            </div>
          ))}
        </dl>

        {snapshot?.errors.length ? (
          <p
            role="alert"
            className="rounded-xl border border-amber/30 bg-amber/5 p-4 text-sm"
          >
            Some task or schedule data could not be loaded. Available results
            are shown; refresh to retry.
          </p>
        ) : null}
        <section
          className="relative overflow-hidden rounded-2xl border border-accent/20 bg-accent/5 p-5 sm:p-8"
          aria-labelledby="overview-next-step"
        >
          <p className="text-xs font-semibold uppercase tracking-widest text-accent">
            What comes next
          </p>
          <h3
            id="overview-next-step"
            className="mt-2 max-w-3xl font-display text-xl font-semibold leading-relaxed"
          >
            {nextStep}
          </h3>
          {canStart && (
            <p className="mt-3 max-w-3xl whitespace-pre-line text-sm leading-relaxed text-ink-2">
              {assignment}
            </p>
          )}
          <div className="mt-5 flex flex-wrap gap-3">
            {canStart && (
              <button
                className="rounded-xl bg-accent px-4 py-2.5 text-sm font-semibold text-accent-fg hover:opacity-90"
                onClick={() => onStartAssignment?.(assignment)}
              >
                Start first assignment
              </button>
            )}
            {manager && (
              <button className={action} onClick={() => onChat(manager.id)}>
                Ask your manager
              </button>
            )}
            {onOpenFiles && (
              <button className={action} onClick={onOpenFiles}>
                Open files
              </button>
            )}
          </div>
          {canStart && (
            <p className="mt-3 text-xs text-ink-3">
              Prepared during setup. Nothing starts until you click.
            </p>
          )}
        </section>

        <div className="grid gap-3 sm:grid-cols-3" aria-label="Explore your workplace">
          <button className={`${panel} text-left transition hover:border-accent/40`} onClick={() => onOpenWork('Tasks')}>
            <span className="text-xs font-medium text-accent">01 / Organize</span>
            <span className="mt-2 block font-semibold">Give your next outcome an owner →</span>
            <span className="mt-2 block text-sm leading-relaxed text-ink-3">Manage each agent’s tasks and choose which routines should run.</span>
          </button>
          {onOpenFiles && <button className={`${panel} text-left transition hover:border-accent/40`} onClick={onOpenFiles}>
            <span className="text-xs font-medium text-accent">02 / Review</span>
            <span className="mt-2 block font-semibold">Find the work in your folder →</span>
            <span className="mt-2 block text-sm leading-relaxed text-ink-3">Read saved briefs, drafts, and code alongside your team’s conversations.</span>
          </button>}
          {onOpenCapabilities && <button className={`${panel} text-left transition hover:border-accent/40`} onClick={onOpenCapabilities}>
            <span className="text-xs font-medium text-accent">03 / Equip</span>
            <span className="mt-2 block font-semibold">Explore what your team can use →</span>
            <span className="mt-2 block text-sm leading-relaxed text-ink-3">Inspect registered tools, access requirements, and what persists.</span>
          </button>}
        </div>

        <div className="grid gap-5 lg:grid-cols-[minmax(0,1.5fr)_minmax(0,1fr)]">
          <div className="min-w-0 space-y-5">
            <section className={panel} aria-labelledby="dashboard-work">
              <div className="flex items-center justify-between gap-3">
                <h3 id="dashboard-work" className="font-semibold">
                  Work in progress
                </h3>
                <button className={action} onClick={() => onOpenWork('Tasks')}>
                  View tasks
                </button>
              </div>
              {tasks.length ? (
                <ul className="mt-4 divide-y divide-line">
                  {[...tasks]
                    .sort(
                      (a, b) =>
                        Number(b.status === 'in_progress') -
                        Number(a.status === 'in_progress'),
                    )
                    .slice(0, 6)
                    .map((task, index) => (
                      <li key={`${task.agentId}:${index}`} className="py-3">
                        <p className="break-words text-sm">{task.content}</p>
                        <p className="mt-1 text-xs text-ink-3">
                          {owner(task.agentId)} ·{' '}
                          {task.status === 'in_progress'
                            ? 'In progress'
                            : 'To do'}
                        </p>
                      </li>
                    ))}
                </ul>
              ) : (
                <p className="mt-5 text-sm text-ink-3">
                  {tasksIncomplete
                    ? 'Tasks will appear when their data is available.'
                    : 'No open tasks. Ask your manager to plan the next piece of work.'}
                </p>
              )}
            </section>
            <section className={panel} aria-labelledby="dashboard-recent">
              <h3 id="dashboard-recent" className="font-semibold">
                Recent outputs
              </h3>
              {recent.length ? (
                <ul className="mt-3 divide-y divide-line">
                  {recent.map((message) => (
                    <li
                      key={`${message.agentId}:${message.id}`}
                      className="py-3"
                    >
                      <button
                        className="w-full text-left"
                        onClick={() => onChat(message.agentId)}
                      >
                        <span className="text-sm font-medium text-accent">
                          {owner(message.agentId)}
                        </span>
                        <p className="mt-1 line-clamp-2 break-words text-sm leading-relaxed text-ink-2">
                          {message.content.text}
                        </p>
                        <span className="mt-1 block text-xs text-ink-3">
                          Assistant reply ·{' '}
                          {new Date(message.created_at_ms).toLocaleString()} ·
                          Open source chat
                        </span>
                      </button>
                    </li>
                  ))}
                </ul>
              ) : (
                <p className="mt-5 text-sm text-ink-3">
                  Your team’s latest replies will appear here.
                </p>
              )}
              {!!completed.length && (
                <div className="mt-4 border-t border-line pt-4">
                  <p className="text-xs text-ink-3">
                    Completed tasks from current task lists; completion dates
                    are not recorded.
                  </p>
                  <ul className="mt-2 divide-y divide-line">
                    {completed.slice(0, 5).map((task, index) => (
                      <li
                        key={`${task.agentId}:completed:${index}`}
                        className="py-3"
                      >
                        <button
                          className="w-full text-left"
                          onClick={() => onChat(task.agentId)}
                        >
                          <p className="break-words text-sm font-medium">
                            {task.content}
                          </p>
                          <p className="mt-1 text-xs text-ink-3">
                            Completed task · completion time unavailable
                          </p>
                          <p className="mt-1 text-xs text-accent">
                            {owner(task.agentId)} · Open source chat
                          </p>
                        </button>
                      </li>
                    ))}
                  </ul>
                </div>
              )}
            </section>
          </div>
          <div className="min-w-0 space-y-5">
            <section className={panel} aria-labelledby="dashboard-attention">
              <h3 id="dashboard-attention" className="font-semibold">
                Needs your attention
              </h3>
              {!online && (
                <p className="mt-3 text-sm text-ink-3">
                  The daemon is offline. Reconnect before starting new work.
                </p>
              )}
              {failed.map((agent) => (
                <button
                  key={agent.id}
                  className="mt-3 block text-left text-sm text-danger"
                  onClick={() => onChat(agent.id)}
                >
                  {agent.name} · Latest run failed →
                </button>
              ))}
              {scheduleErrors.map((schedule) => (
                <button
                  key={schedule.id}
                  className="mt-3 block max-w-full break-words text-left text-sm text-danger"
                  onClick={() => onOpenWork('Schedules')}
                >
                  {owner(schedule.agentId)} · Schedule needs review:{' '}
                  {schedule.prompt}
                </button>
              ))}
              {online && !failed.length && !scheduleErrors.length && (
                <p className="mt-4 text-sm text-ink-3">
                  {schedulesIncomplete
                    ? 'Schedule checks are incomplete.'
                    : 'No failed agent runs or schedule errors reported.'}
                </p>
              )}
            </section>
            <section className={panel} aria-labelledby="dashboard-next">
              <div className="flex items-center justify-between gap-3">
                <h3 id="dashboard-next" className="font-semibold">
                  Coming up
                </h3>
                <button
                  className={action}
                  onClick={() => onOpenWork('Schedules')}
                >
                  View schedules
                </button>
              </div>
              {upcoming.length ? (
                <ul className="mt-3 divide-y divide-line">
                  {upcoming.slice(0, 4).map((schedule) => (
                    <li key={schedule.id} className="py-3">
                      <p className="break-words text-sm">{schedule.prompt}</p>
                      <p className="mt-1 text-xs text-ink-3">
                        {owner(schedule.agentId)} ·{' '}
                        {new Date(schedule.nextDueAtMs).toLocaleString()}
                      </p>
                    </li>
                  ))}
                </ul>
              ) : (
                <p className="mt-4 text-sm text-ink-3">
                  {schedulesIncomplete
                    ? 'Schedules are not available yet.'
                    : 'No upcoming schedules.'}
                </p>
              )}
            </section>
          </div>
        </div>
        <section className={panel} aria-labelledby="dashboard-team">
          <div className="flex items-center justify-between">
            <h3 id="dashboard-team" className="font-semibold">
              Your team
            </h3>
            <button className={action} onClick={onOpenTeam}>
              View team
            </button>
          </div>
          <div className="mt-4 grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
            {agents.map((agent) => (
              <button
                key={agent.id}
                onClick={() => onChat(agent.id)}
                className="flex min-w-0 items-center gap-3 rounded-xl border border-line p-3 text-left hover:border-accent/40"
              >
                <AgentAvatar id={agent.id} name={agent.name} size={36} />
                <span className="min-w-0">
                  <span className="block truncate text-sm font-medium">
                    {agent.name}
                  </span>
                  <span className="mt-1 block line-clamp-3 text-xs leading-relaxed text-ink-2">
                    {agent.bio?.trim() || (agent.id === manager?.id
                      ? 'Coordinates priorities and keeps the team moving.'
                      : 'Open the agent’s profile to review its responsibilities.')}
                  </span>
                  {online && agent.status === 'Running' && (
                    <span className="mt-2 block text-xs text-accent">
                      {tasks.find((task) => task.agentId === agent.id && task.status === 'in_progress')?.activeForm || 'Working now'}
                    </span>
                  )}
                  <span className="block text-xs text-ink-3">
                    {online ? agent.status : 'Status unavailable'} ·{' '}
                    {agent.provider === 'chatgpt'
                      ? 'ChatGPT subscription'
                      : agent.provider}
                  </span>
                </span>
              </button>
            ))}
          </div>
        </section>
      </div>
    </section>
  );
}
