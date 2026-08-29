import type { AgentDetail } from '../lib/types';
import { formatInterval, formatRelative } from '../lib/checkins';
import type { DaemonSchedule } from '../lib/daemon-api';
import { AlertIcon, PlusIcon, PulseIcon, TrashIcon } from './icons';
import { ErrorBanner, primaryBtnCls } from './ui-bits';

const OUTCOME_STYLE: Record<
  NonNullable<DaemonSchedule['lastOutcome']>['status'],
  { dot: string; label: string }
> = {
  silent: { dot: 'bg-zinc-500', label: 'stayed silent' },
  spoke: { dot: 'bg-mint', label: 'sent a message' },
  error: { dot: 'bg-danger', label: 'run failed' },
};

export function CheckinsView({
  agent,
  checkins,
  prompt,
  setPrompt,
  intervalMin,
  setIntervalMin,
  addCheckin,
  removeCheckin,
  error,
  target,
  setTarget,
  telegramAvailable,
  busy,
}: {
  agent: AgentDetail;
  checkins: DaemonSchedule[];
  prompt: string;
  setPrompt: (v: string) => void;
  intervalMin: number;
  setIntervalMin: (v: number) => void;
  addCheckin: () => void;
  removeCheckin: (id: string) => void;
  error: string | null;
  target: 'workspace' | 'telegram';
  setTarget: (value: 'workspace' | 'telegram') => void;
  telegramAvailable: boolean;
  busy: boolean;
}) {
  return (
    <section
      className="relative z-[1] flex min-w-0 flex-col"
      aria-labelledby="checkins-heading"
    >
      {/* View header */}
      <header className="flex items-center justify-between gap-3 border-b border-line bg-panel/60 px-6 py-3 backdrop-blur-xl">
        <div className="flex items-center gap-3">
          <div className="flex h-9 w-9 items-center justify-center rounded-xl bg-mint/10 text-mint">
            <PulseIcon size={17} />
          </div>
          <div>
            <h3
              id="checkins-heading"
              className="font-display text-[15px] font-semibold tracking-tight text-ink"
            >
              Proactive
            </h3>
            <p className="font-mono text-[11px] text-ink-3">
              {checkins.length} scheduled · {agent.name}
            </p>
          </div>
        </div>
      </header>

      <div className="flex-1 overflow-y-auto">
        <div className="animate-rise-in mx-auto w-full max-w-2xl px-6 py-8">
          <p className="text-sm leading-relaxed text-ink-2">
            Your agent wakes up on a timer and messages you first when it has
            something to say. Each proactive prompt runs on a schedule — a reply
            of exactly{' '}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-[11px] text-mint">
              CHECKIN_OK
            </code>{' '}
            keeps the run silent.
          </p>
          <p className="mt-2 font-mono text-[11px] text-ink-3">
            runs while anima-daemon is active · persisted per agent
          </p>

          {/* Add form */}
          <div className="glass-strong mt-6 rounded-2xl p-5">
            <label className="mb-2 block font-mono text-[11px] font-medium uppercase tracking-[0.14em] text-ink-3">
              New proactive prompt
            </label>
            <input
              value={prompt}
              onChange={(e) => setPrompt(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') addCheckin();
              }}
              placeholder="e.g. Check my goals and remind me what to focus on"
              className="field"
            />
            <div className="mt-3 flex items-center justify-between gap-3">
              <label className="flex items-center gap-2 font-mono text-[11px] text-ink-3">
                deliver to
                <select
                  value={target}
                  onChange={(event) =>
                    setTarget(event.target.value as 'workspace' | 'telegram')
                  }
                  className="field w-auto px-2 py-1"
                  disabled={busy}
                >
                  <option value="workspace">Workspace</option>
                  <option value="telegram" disabled={!telegramAvailable}>
                    Telegram
                  </option>
                </select>
              </label>
              {!telegramAvailable ? (
                <span className="font-mono text-[10px] text-ink-3">
                  approve a Telegram chat to enable delivery
                </span>
              ) : null}
              <label className="flex items-center gap-2 font-mono text-[11px] text-ink-3">
                every
                <input
                  type="number"
                  min={1}
                  value={intervalMin}
                  onChange={(e) =>
                    setIntervalMin(Math.max(1, Number(e.target.value) || 1))
                  }
                  className="field w-16 px-2 py-1 text-center font-mono text-[11px]"
                />
                min
              </label>
              <button
                onClick={addCheckin}
                disabled={
                  !prompt.trim() ||
                  busy ||
                  (target === 'telegram' && !telegramAvailable)
                }
                className={primaryBtnCls}
              >
                <PlusIcon size={14} />
                Add prompt
              </button>
            </div>
            {error && (
              <div className="mt-3">
                <ErrorBanner message={error} icon={<AlertIcon size={14} />} />
              </div>
            )}
          </div>

          {/* List */}
          <div className="mt-6 space-y-2">
            {checkins.length === 0 ? (
              <div className="flex flex-col items-center rounded-2xl border border-dashed border-line px-6 py-12 text-center">
                <PulseIcon size={22} className="mb-3 text-ink-3/50" />
                <p className="text-sm font-medium text-ink-2">
                  Nothing scheduled yet
                </p>
                <p className="mt-1 text-xs text-ink-3">
                  Add a proactive prompt above and your agent will reach out on
                  its own.
                </p>
              </div>
            ) : (
              checkins.map((c) => (
                <div
                  key={c.id}
                  className="group glass animate-fade-in rounded-xl px-4 py-3.5 transition hover:border-line-strong"
                >
                  <div className="flex items-center gap-3.5">
                    <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-mint/10 text-mint">
                      <PulseIcon size={14} />
                    </div>
                    <div className="min-w-0 flex-1">
                      <div
                        className="truncate text-sm text-ink"
                        title={c.prompt}
                      >
                        {c.prompt}
                      </div>
                      <div className="mt-0.5 flex items-center gap-2 font-mono text-[10px] text-ink-3">
                        {c.lastFiredAtMs ? (
                          <>
                            <span
                              className={`h-1.5 w-1.5 rounded-full ${OUTCOME_STYLE[c.lastOutcome?.status ?? 'silent'].dot}`}
                            />
                            last ran {formatRelative(c.lastFiredAtMs)} ·{' '}
                            {
                              OUTCOME_STYLE[c.lastOutcome?.status ?? 'silent']
                                .label
                            }
                          </>
                        ) : (
                          'has not run yet'
                        )}
                      </div>
                    </div>
                    <span className="shrink-0 rounded-md border border-line bg-white/[0.03] px-2 py-0.5 font-mono text-[10px] text-ink-2">
                      {c.trigger.type === 'interval'
                        ? `every ${formatInterval(c.trigger.intervalMs / 1000)}`
                        : `daily ${String(c.trigger.hour).padStart(2, '0')}:${String(c.trigger.minute).padStart(2, '0')}`}
                    </span>
                    <button
                      onClick={() => removeCheckin(c.id)}
                      className="shrink-0 cursor-pointer text-ink-3 opacity-0 transition focus-visible:opacity-100 group-hover:opacity-100 hover:text-danger"
                      aria-label="Remove check-in"
                    >
                      <TrashIcon size={14} />
                    </button>
                  </div>
                </div>
              ))
            )}
          </div>
        </div>
      </div>
    </section>
  );
}
