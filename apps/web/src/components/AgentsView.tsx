import { deriveAccessProfile } from '../lib/agent-access';
import type { AgentDetail } from '../lib/types';
import { ShieldIcon, SparkIcon } from './icons';

function titleCase(value: string) {
  return `${value.charAt(0).toUpperCase()}${value.slice(1)}`;
}

export function AgentsView({
  agents,
  mainAgent,
}: {
  agents: readonly AgentDetail[];
  mainAgent: AgentDetail;
}) {
  return (
    <section
      className="h-full overflow-y-auto px-4 py-7 sm:px-6"
      aria-labelledby="agents-view-heading"
    >
      <div className="mx-auto w-full max-w-4xl">
        <div className="flex items-end justify-between gap-4">
          <div>
            <p className="font-mono text-[10px] uppercase tracking-[0.16em] text-ink-3">
              Daemon collection
            </p>
            <h2
              id="agents-view-heading"
              className="mt-1 font-display text-2xl font-semibold tracking-tight text-ink"
            >
              Agents
            </h2>
          </div>
          <span className="font-mono text-[11px] text-ink-3">
            {agents.length} total
          </span>
        </div>

        <div className="mt-6 grid gap-3 md:grid-cols-2">
          {agents.map((agent) => {
            const isMain = agent.id === mainAgent.id;
            const access = titleCase(deriveAccessProfile(agent.toolNames));
            return (
              <article
                key={agent.id}
                aria-label={`${agent.name} agent`}
                className="glass rounded-2xl p-4 transition hover:border-line-strong"
              >
                <div className="flex items-start justify-between gap-3">
                  <div className="flex min-w-0 items-center gap-3">
                    <div
                      className={`flex h-10 w-10 shrink-0 items-center justify-center rounded-xl ${
                        isMain
                          ? 'bg-accent/15 text-accent'
                          : 'bg-white/[0.04] text-ink-2'
                      }`}
                    >
                      <SparkIcon size={16} />
                    </div>
                    <div className="min-w-0">
                      <h3 className="truncate font-display text-sm font-semibold text-ink">
                        {agent.name}
                      </h3>
                      <p
                        className="truncate font-mono text-[10px] text-ink-3"
                        title={`${agent.provider}/${agent.model}`}
                      >
                        {agent.provider}/{agent.model}
                      </p>
                    </div>
                  </div>
                  <span className="rounded-full border border-line bg-white/[0.03] px-2 py-0.5 font-mono text-[9px] uppercase tracking-wider text-ink-2">
                    {isMain ? 'Main' : 'Read only'}
                  </span>
                </div>
                <div className="mt-4 flex flex-wrap gap-2 font-mono text-[10px] text-ink-3">
                  <span className="inline-flex items-center gap-1.5 rounded-lg border border-line px-2 py-1">
                    <SparkIcon size={11} /> Agent {agent.status}
                  </span>
                  <span className="inline-flex items-center gap-1.5 rounded-lg border border-line px-2 py-1">
                    <ShieldIcon size={11} /> Access {access}
                  </span>
                </div>
              </article>
            );
          })}
        </div>
      </div>
    </section>
  );
}
