import { useState } from 'react';
import { deriveAccessProfile } from '../lib/agent-access';
import { formatTokens } from './ui-bits';
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
  const [query, setQuery] = useState('');
  const visibleAgents = agents.filter((agent) =>
    `${agent.name} ${agent.provider} ${agent.model} ${agent.status}`
      .toLowerCase()
      .includes(query.trim().toLowerCase()),
  );
  return (
    <section
      className="studio-page h-full overflow-y-auto"
      aria-labelledby="agents-view-heading"
    >
      <div className="mx-auto w-full max-w-4xl">
        <div className="flex items-end justify-between gap-4">
          <div>
            <p className="font-mono text-[10px] uppercase tracking-[0.16em] text-ink-3">
              GOOD COMPANY
            </p>
            <h2
              id="agents-view-heading"
              className="studio-page-title mt-3 text-ink"
            >
              Agents
            </h2>
          </div>
          <span className="font-mono text-[11px] text-ink-3">
            {agents.length} total
          </span>
        </div>
        <p className="studio-page-intro">
          Different strengths. Shared purpose. Meet the intelligence behind your
          workspace.
        </p>

        <div className="studio-agent-search">
          <input
            type="search"
            className="field"
            aria-label="Search agents"
            placeholder="Search by name, model, provider, or status…"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
          <span role="status">
            {visibleAgents.length} of {agents.length}
          </span>
        </div>
        {!visibleAgents.length && (
          <div className="studio-empty-result">
            <span aria-hidden>⌕</span>
            <p>No agents match your search.</p>
            <button
              type="button"
              className="studio-tool-button"
              onClick={() => setQuery('')}
            >
              Clear search
            </button>
          </div>
        )}
        <div className="mt-6 grid gap-3 md:grid-cols-2">
          {visibleAgents.map((agent) => {
            const isMain = agent.id === mainAgent.id;
            const access = titleCase(deriveAccessProfile(agent.toolNames));
            return (
              <article
                key={agent.id}
                aria-label={`${agent.name} agent`}
                className="studio-agent-card glass transition hover:border-line-strong"
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
                <div className="studio-agent-usage">
                  <span>
                    <strong>{agent.messages.length}</strong> messages
                  </span>
                  <span>
                    <strong>
                      {formatTokens(agent.token_usage.total_tokens)}
                    </strong>{' '}
                    tokens
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
