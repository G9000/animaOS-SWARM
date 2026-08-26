import { useEffect, useMemo, useState } from 'react';
import type { AgentDetail } from '../lib/types';
import { MODEL_SUGGESTIONS, type DaemonProvider } from '../lib/daemon-api';
import { AlertIcon, TrashIcon, XIcon } from './icons';
import { ErrorBanner, SectionTitle, formatTokens, labelCls, primaryBtnCls } from './ui-bits';

function InfoRow({ label, value, mono = true }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="flex items-start justify-between gap-4 rounded-lg border border-line bg-white/[0.02] px-3.5 py-2.5">
      <span className="shrink-0 font-mono text-[11px] uppercase tracking-wider text-ink-3">{label}</span>
      <span className={`min-w-0 break-all text-right text-xs text-ink ${mono ? 'font-mono' : ''}`}>
        {value}
      </span>
    </div>
  );
}

export interface AgentConfigPatch {
  name?: string;
  model?: string;
  provider?: string;
  system?: string;
}

/**
 * Editable agent settings. Saves through the daemon's
 * PATCH /api/agents/:id endpoint — the conversation is preserved and the
 * new system prompt takes effect on the next run.
 */
export function SettingsPanel({
  agent,
  providers,
  saving,
  error,
  saveSettings,
  resetAgent,
  close,
}: {
  agent: AgentDetail;
  providers: DaemonProvider[] | null;
  saving: boolean;
  error: string | null;
  saveSettings: (patch: AgentConfigPatch) => Promise<boolean>;
  resetAgent: () => void;
  close: () => void;
}) {
  const seedModel = useMemo(() => {
    const options = MODEL_SUGGESTIONS[agent.provider] ?? [];
    return options.includes(agent.model) ? agent.model : '__custom__';
  }, [agent.provider, agent.model]);

  const [name, setName] = useState(agent.name);
  const [provider, setProvider] = useState(agent.provider);
  const [model, setModel] = useState(seedModel);
  const [customModel, setCustomModel] = useState(
    seedModel === '__custom__' ? agent.model : ''
  );
  const [system, setSystem] = useState(agent.system ?? '');
  const [savedFlash, setSavedFlash] = useState(false);

  // Re-seed the form whenever a different agent is shown.
  useEffect(() => {
    const options = MODEL_SUGGESTIONS[agent.provider] ?? [];
    const seeded = options.includes(agent.model) ? agent.model : '__custom__';
    setName(agent.name);
    setProvider(agent.provider);
    setModel(seeded);
    setCustomModel(seeded === '__custom__' ? agent.model : '');
    setSystem(agent.system ?? '');
  }, [agent.id, agent.name, agent.provider, agent.model, agent.system]);

  const resolvedModel = model === '__custom__' ? customModel.trim() : model;
  const dirty =
    name.trim() !== agent.name ||
    provider !== agent.provider ||
    resolvedModel !== agent.model ||
    system !== (agent.system ?? '');

  const save = async () => {
    const patch: AgentConfigPatch = {};
    if (name.trim() && name.trim() !== agent.name) patch.name = name.trim();
    if (provider !== agent.provider) patch.provider = provider;
    if (resolvedModel && resolvedModel !== agent.model) patch.model = resolvedModel;
    // Empty string clears the prompt back to the daemon default.
    if (system !== (agent.system ?? '')) patch.system = system;
    if (Object.keys(patch).length === 0) return;
    if (await saveSettings(patch)) {
      setSavedFlash(true);
      setTimeout(() => setSavedFlash(false), 2000);
    }
  };

  return (
    <>
      <div
        className="animate-fade-in absolute inset-0 z-10 bg-black/50 backdrop-blur-[2px]"
        onClick={close}
      />
      <aside className="animate-slide-in-right absolute inset-y-0 right-0 z-20 flex w-full max-w-md flex-col border-l border-line bg-panel/95 shadow-2xl shadow-black/60 backdrop-blur-2xl">
        {/* Panel header */}
        <div className="flex items-center justify-between border-b border-line px-6 py-4">
          <div>
            <h3 className="font-display text-base font-semibold tracking-tight text-ink">
              Agent settings
            </h3>
            <p className="mt-0.5 font-mono text-[11px] text-ink-3">
              edits keep the conversation · apply on next run
            </p>
          </div>
          <button
            onClick={close}
            className="flex h-8 w-8 cursor-pointer items-center justify-center rounded-lg border border-line text-ink-3 transition hover:border-line-strong hover:text-ink"
            aria-label="Close settings"
          >
            <XIcon size={14} />
          </button>
        </div>

        <div className="flex flex-1 flex-col gap-7 overflow-y-auto px-6 py-6">
          {/* Runtime info (read-only) */}
          <section className="space-y-2.5">
            <SectionTitle>Runtime</SectionTitle>
            <InfoRow label="Status" value={agent.status.toLowerCase()} />
            <InfoRow
              label="Created"
              value={new Date(agent.created_at_ms).toLocaleString([], {
                month: 'short',
                day: 'numeric',
                hour: '2-digit',
                minute: '2-digit',
              })}
            />
            <InfoRow label="Tokens" value={formatTokens(agent.token_usage.total_tokens)} />
          </section>

          {/* Editable identity */}
          <section className="space-y-2.5">
            <SectionTitle>Identity</SectionTitle>
            <div>
              <label className={labelCls}>Name</label>
              <input
                value={name}
                onChange={(e) => setName(e.target.value)}
                className="field"
              />
            </div>
            <div>
              <label className={labelCls}>Provider</label>
              <select
                value={provider}
                onChange={(e) => {
                  const next = e.target.value;
                  setProvider(next);
                  const options = MODEL_SUGGESTIONS[next] ?? [];
                  setModel(options[0] ?? '__custom__');
                }}
                className="field"
              >
                {/* Keep the current provider selectable even if the daemon
                    catalog is still loading or lacks it. */}
                {(providers ?? [])
                  .map((p) => p.id)
                  .concat(
                    providers?.some((p) => p.id === agent.provider) || !agent.provider
                      ? []
                      : [agent.provider]
                  )
                  .filter((id, i, all) => all.indexOf(id) === i)
                  .map((id) => (
                    <option key={id} value={id}>{id}</option>
                  ))}
              </select>
              {providers && (
                <p className="mt-1.5 font-mono text-[10px] text-ink-3/70">
                  {providers.find((p) => p.id === provider)?.configured
                    ? 'key configured on the daemon'
                    : 'no key configured for this provider on the daemon'}
                </p>
              )}
            </div>
            <div>
              <label className={labelCls}>Model</label>
              <select
                value={model}
                onChange={(e) => setModel(e.target.value)}
                className="field"
              >
                {(MODEL_SUGGESTIONS[provider] ?? []).map((m) => (
                  <option key={m} value={m}>{m}</option>
                ))}
                <option value="__custom__">custom…</option>
              </select>
            </div>
            {model === '__custom__' && (
              <input
                value={customModel}
                onChange={(e) => setCustomModel(e.target.value)}
                placeholder="model id, e.g. llama3.1"
                className="field animate-fade-in"
              />
            )}
          </section>

          {/* System prompt */}
          <section className="space-y-2.5">
            <SectionTitle>System prompt</SectionTitle>
            <textarea
              value={system}
              onChange={(e) => setSystem(e.target.value)}
              rows={6}
              placeholder="Leave empty for the daemon default."
              className="field resize-y leading-relaxed"
            />
            <p className="font-mono text-[10px] text-ink-3/70">
              cleared = daemon builds its default prompt from the agent profile
            </p>
          </section>

          {error && <ErrorBanner message={error} icon={<AlertIcon size={14} />} />}

          <button
            onClick={save}
            disabled={saving || !dirty || !resolvedModel || !name.trim()}
            className={`${primaryBtnCls} w-full py-2.5`}
          >
            {saving ? 'Saving…' : savedFlash ? 'Saved ✓' : dirty ? 'Save changes' : 'No changes'}
          </button>

          {/* Danger zone */}
          <section className="rounded-xl border border-red-400/20 bg-red-400/[0.04] p-4">
            <div className="flex items-center justify-between gap-3">
              <div>
                <div className="text-xs font-semibold text-red-300">Reset agent</div>
                <div className="mt-0.5 text-[11px] leading-relaxed text-red-300/60">
                  deletes the agent and its entire conversation
                </div>
              </div>
              <button
                onClick={resetAgent}
                className="flex shrink-0 cursor-pointer items-center gap-1.5 rounded-lg border border-red-400/30 bg-red-400/10 px-3 py-1.5 text-xs font-medium text-red-300 transition hover:bg-red-400/20"
              >
                <TrashIcon size={12} />
                Reset
              </button>
            </div>
          </section>
        </div>
      </aside>
    </>
  );
}
