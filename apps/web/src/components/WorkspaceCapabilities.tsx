import { useEffect, useState } from 'react';
import type { DaemonCapabilities } from '@animaOS-SWARM/sdk';
import { daemon } from '../lib/daemon-api';

const groups = [
  [
    'workspace',
    'Files & workspace',
    'Read, create and maintain the work in your folder.',
  ],
  [
    'terminal',
    'Coding & processes',
    'Run commands and manage local development work.',
  ],
  [
    'memory',
    'Memory & context',
    'Keep useful context available for future work.',
  ],
  [
    'team',
    'Team coordination',
    'Let agents communicate and divide work within their authority.',
  ],
  [
    'research',
    'Research',
    'Gather source material with the registered research tools.',
  ],
  [
    'productivity',
    'Connected work',
    'Work with connected services when configured and authorized.',
  ],
  ['utility', 'Utilities', 'Supporting tools for everyday agent work.'],
] as const;
const panel = 'rounded-2xl border border-line bg-surface p-5 sm:p-6';

export function WorkspaceCapabilities({ online }: { online: boolean }) {
  const [inventory, setInventory] = useState<DaemonCapabilities | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(online);
  const [refresh, setRefresh] = useState(0);
  const [query, setQuery] = useState('');

  useEffect(() => {
    let cancelled = false;
    setInventory(null);
    setError(null);
    setLoading(online);
    if (online) {
      void daemon.capabilities().then(
        (result) => {
          if (!cancelled) {
            setInventory(result);
            setLoading(false);
          }
        },
        (reason: unknown) => {
          if (!cancelled) {
            setError(
              reason instanceof Error
                ? reason.message
                : 'Capabilities could not be loaded.',
            );
            setLoading(false);
          }
        },
      );
    }
    return () => {
      cancelled = true;
    };
  }, [online, refresh]);

  const search = query.trim().toLowerCase();
  const tools =
    inventory?.tools.filter((tool) =>
      `${tool.name} ${tool.description} ${tool.category}`
        .toLowerCase()
        .includes(search),
    ) ?? [];
  const toolGroups = [
    ...groups.map(([category, label, description]) => ({
      category,
      label,
      description,
      members: tools.filter((tool) => tool.category === category),
    })),
    {
      category: 'other',
      label: 'Other tools',
      description: 'Registered tools from additional daemon categories.',
      members: tools.filter(
        (tool) => !groups.some(([category]) => tool.category === category),
      ),
    },
  ];

  return (
    <section
      aria-labelledby="capabilities-heading"
      className="h-full overflow-y-auto p-5 pb-28 sm:p-8 md:pb-8"
    >
      <div className="mx-auto max-w-6xl space-y-7">
        <header className="flex flex-wrap items-start justify-between gap-4">
          <div className="max-w-2xl">
            <p className="text-xs font-semibold uppercase tracking-widest text-accent">
              Your workplace, equipped
            </p>
            <h2
              id="capabilities-heading"
              className="mt-2 font-display text-3xl font-semibold tracking-tight"
            >
              Capabilities
            </h2>
            <p className="mt-3 text-sm leading-relaxed text-ink-2">
              Explore the tools your daemon knows how to run, what persists, and
              what is still to come.
            </p>
          </div>
          <button
            type="button"
            disabled={!online || loading}
            onClick={() => setRefresh((value) => value + 1)}
            className="rounded-xl border border-line px-4 py-2.5 text-sm font-medium hover:bg-accent/5 disabled:opacity-50"
          >
            Refresh capabilities
          </button>
        </header>

        {!online ? (
          <div className={panel}>
            <h3 className="font-semibold">Your daemon is offline</h3>
            <p className="mt-2 text-sm text-ink-2">
              Connect to the daemon to inspect its current tools and
              persistence. Availability cannot be confirmed while offline.
            </p>
          </div>
        ) : loading ? (
          <p role="status" className={`${panel} text-sm text-ink-2`}>
            Loading capabilities…
          </p>
        ) : error ? (
          <div
            role="alert"
            className="rounded-2xl border border-amber/30 bg-amber/5 p-5"
          >
            <h3 className="font-semibold">Capabilities could not be loaded</h3>
            <p className="mt-2 break-words text-sm text-ink-2">{error}</p>
            <p className="mt-2 text-sm text-ink-3">
              Check the daemon connection and refresh to try again.
            </p>
          </div>
        ) : inventory ? (
          <>
            <section
              className="rounded-2xl border border-accent/20 bg-accent/5 p-5 sm:p-6"
              aria-labelledby="capabilities-access"
            >
              <h3 id="capabilities-access" className="font-semibold">
                Tools follow your authority
              </h3>
              <p className="mt-2 max-w-3xl text-sm leading-relaxed text-ink-2">
                These tools are registered in the daemon. Registration does not
                grant an agent access or confirm a service is connected. Each
                agent still needs the appropriate permissions and any listed
                requirements.
              </p>
            </section>

            <section
              aria-labelledby="registered-tools-heading"
              className="space-y-4"
            >
              <div className="flex flex-wrap items-center justify-between gap-3">
                <div>
                  <h3
                    id="registered-tools-heading"
                    className="text-lg font-semibold"
                  >
                    Registered tools{' '}
                    <span className="ml-2 text-sm font-normal text-ink-3">
                      {inventory.tools.length}
                    </span>
                  </h3>
                  <p className="mt-1 text-sm text-ink-3">
                    Inspect the actual tools behind your agents’ work.
                  </p>
                </div>
                <input
                  type="search"
                  aria-label="Search capabilities"
                  placeholder="Find a tool…"
                  value={query}
                  onChange={(event) => setQuery(event.target.value)}
                  className="field w-full sm:w-64"
                />
              </div>
              {!tools.length && (
                <p className={`${panel} text-sm text-ink-3`}>
                  {search
                    ? 'No tools match your search.'
                    : 'No native tools are registered in this daemon.'}
                </p>
              )}
              <div className="grid items-start gap-4 lg:grid-cols-2">
                {toolGroups.map(({ category, label, description, members }) => {
                  return members.length ? (
                    <section
                      key={category}
                      className={panel}
                      aria-label={label}
                    >
                      <h4 className="font-semibold">{label}</h4>
                      <p className="mt-1 text-sm leading-relaxed text-ink-3">
                        {description}
                      </p>
                      <ul className="mt-4 divide-y divide-line">
                        {members.map((tool) => (
                          <li
                            key={tool.name}
                            className="py-3 first:pt-0 last:pb-0"
                          >
                            <p className="break-words font-mono text-xs font-medium text-accent">
                              {tool.name}
                            </p>
                            <p className="mt-1 text-sm leading-relaxed text-ink-2">
                              {tool.description}
                            </p>
                            {tool.requirements.length > 0 && (
                              <div
                                className="mt-2 flex flex-wrap gap-1.5"
                                aria-label={`Requirements for ${tool.name}`}
                              >
                                {tool.requirements.map((requirement) => (
                                  <span
                                    key={requirement}
                                    className="max-w-full break-words rounded-md border border-line px-2 py-1 text-xs text-ink-3"
                                  >
                                    {requirement}
                                  </span>
                                ))}
                              </div>
                            )}
                          </li>
                        ))}
                      </ul>
                    </section>
                  ) : null;
                })}
              </div>
            </section>

            <section
              className={panel}
              aria-labelledby="capabilities-persistence"
            >
              <h3
                id="capabilities-persistence"
                className="text-lg font-semibold"
              >
                What carries forward
              </h3>
              <p className="mt-2 text-sm text-ink-3">
                Saved records and running work have different lifetimes.
              </p>
              <dl className="mt-5 grid gap-5 sm:grid-cols-3">
                <div>
                  <dt className="text-sm font-medium">
                    Workspace configuration
                  </dt>
                  <dd className="mt-2 break-words text-sm text-ink-2">
                    {inventory.persistence.controlPlane}
                  </dd>
                </div>
                <div>
                  <dt className="text-sm font-medium">Agent memory</dt>
                  <dd className="mt-2 break-words text-sm text-ink-2">
                    {inventory.persistence.memory}
                  </dd>
                </div>
                <div>
                  <dt className="text-sm font-medium">Interrupted work</dt>
                  <dd className="mt-2 text-sm leading-relaxed text-ink-2">
                    {inventory.persistence.executionJournal
                      ? 'Execution journal configured. Recovery depends on the operation and its authority.'
                      : 'Automatic run continuation is not available. Review interrupted work before starting it again.'}
                  </dd>
                </div>
              </dl>
              <p className="mt-4 text-sm leading-relaxed text-ink-3">
                Agent task lists are saved separately in workspace files.
              </p>
              {inventory.limitations.length > 0 && (
                <ul className="mt-5 list-disc space-y-2 border-t border-line pl-5 pt-4 text-sm leading-relaxed text-ink-3">
                  {inventory.limitations.map((limitation) => (
                    <li key={limitation}>{limitation}</li>
                  ))}
                </ul>
              )}
            </section>

            {inventory.extensions.length > 0 && (
              <section aria-labelledby="planned-capabilities-heading">
                <h3
                  id="planned-capabilities-heading"
                  className="text-lg font-semibold"
                >
                  Room to grow
                </h3>
                <p className="mt-2 text-sm text-ink-3">
                  Future extensions. These modules cannot be enabled or used
                  yet.
                </p>
                <div className="mt-4 grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
                  {inventory.extensions.map((extension) => (
                    <article key={extension.id} className={panel}>
                      <p className="text-xs text-ink-3">
                        Planned · unavailable
                      </p>
                      <h4 className="mt-2 font-semibold">{extension.label}</h4>
                      <p className="mt-2 text-sm leading-relaxed text-ink-2">
                        {extension.description}
                      </p>
                    </article>
                  ))}
                </div>
              </section>
            )}
          </>
        ) : null}
      </div>
    </section>
  );
}
