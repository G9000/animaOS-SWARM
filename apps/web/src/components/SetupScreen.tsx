import { useState } from 'react';
import { MODEL_SUGGESTIONS, type DaemonProvider } from '../lib/daemon-api';
import { AlertIcon, ChevronIcon, SparkIcon } from './icons';
import { ErrorBanner, labelCls, primaryBtnCls } from './ui-bits';

export interface SetupState {
  providers: DaemonProvider[] | null;
  name: string;
  setName: (v: string) => void;
  provider: string;
  setProvider: (p: string) => void;
  model: string;
  setModel: (m: string) => void;
  customModel: string;
  setCustomModel: (v: string) => void;
  system: string;
  setSystem: (v: string) => void;
  creating: boolean;
  error: string | null;
  online: boolean | null;
  createAgent: () => void;
}

export function SetupScreen(s: SetupState) {
  const [showAdvanced, setShowAdvanced] = useState(false);
  const modelOptions = MODEL_SUGGESTIONS[s.provider] ?? [];

  return (
    <div className="relative flex flex-1 items-center justify-center overflow-y-auto px-6 py-10">
      <div className="animate-rise-in w-full max-w-lg">
        {/* Hero */}
        <div className="mb-8 flex flex-col items-center text-center">
          <div className="relative mb-5 flex h-16 w-16 items-center justify-center">
            <span className="animate-ripple absolute inset-0 rounded-2xl border border-sky-400/40" />
            <span
              className="animate-ripple absolute inset-0 rounded-2xl border border-sky-400/25"
              style={{ animationDelay: '1.3s' }}
            />
            <div className="animate-orb relative flex h-14 w-14 items-center justify-center rounded-2xl bg-sky-500 shadow-xl shadow-sky-500/25">
              <SparkIcon size={26} className="text-white" />
            </div>
          </div>
          <h1 className="font-display text-3xl font-bold tracking-tight text-ink">
            Meet your <span className="text-gradient">agent</span>
          </h1>
          <p className="mt-2 max-w-sm text-sm leading-relaxed text-ink-2">
            One agent on the daemon runtime. Configure it, chat with it,
            watch the loop work end to end.
          </p>
        </div>

        {/* Card */}
        <div className="glass-strong rounded-3xl p-7 shadow-2xl shadow-black/50">
          <div className="space-y-5">
            {/* Provider grid */}
            <div>
              <label className={labelCls}>Provider</label>
              {s.providers === null ? (
                <p className="rounded-xl border border-dashed border-line px-3.5 py-3 text-xs text-ink-3">
                  loading provider catalog…
                </p>
              ) : (
                <div className="grid grid-cols-2 gap-1.5 sm:grid-cols-3">
                  {s.providers.map((p) => {
                    const active = s.provider === p.id;
                    return (
                      <button
                        key={p.id}
                        type="button"
                        onClick={() => {
                          s.setProvider(p.id);
                          s.setModel((MODEL_SUGGESTIONS[p.id] ?? [])[0] ?? '__custom__');
                        }}
                        title={
                          p.requiresKey && !p.configured
                            ? `needs ${p.apiKeyEnvs[0] ?? 'API key'} in the daemon env`
                            : p.requiresKey
                              ? 'API key configured'
                              : 'no key needed'
                        }
                        className={`flex cursor-pointer items-center justify-between gap-1.5 rounded-lg border px-2.5 py-2 text-left transition-all duration-150 ${
                          active
                            ? 'border-sky-400/60 bg-sky-400/10 shadow-[0_0_12px_-2px_rgba(56,189,248,0.4)]'
                            : 'border-line bg-white/[0.02] hover:border-line-strong'
                        }`}
                      >
                        <span className={`truncate font-mono text-[11px] ${active ? 'text-sky-300' : 'text-ink-3'}`}>
                          {p.id}
                        </span>
                        <span
                          className={`h-1.5 w-1.5 shrink-0 rounded-full ${
                            p.configured ? 'bg-mint' : 'bg-zinc-600'
                          }`}
                          title={p.configured ? 'configured' : 'not configured'}
                        />
                      </button>
                    );
                  })}
                </div>
              )}
              <p className="mt-1.5 font-mono text-[10px] text-ink-3/70">
                green dot = key configured on the daemon · grey = missing env key
              </p>
            </div>

            {/* Name + Model */}
            <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
              <div>
                <label className={labelCls}>
                  Name <span className="normal-case text-ink-3/60">(optional)</span>
                </label>
                <input
                  value={s.name}
                  onChange={(e) => s.setName(e.target.value)}
                  placeholder={s.model === '__custom__' ? 'my-agent' : s.model}
                  className="field"
                />
              </div>
              <div>
                <label className={labelCls}>Model</label>
                <select
                  value={s.model}
                  onChange={(e) => s.setModel(e.target.value)}
                  className="field"
                >
                  {modelOptions.map((m) => (
                    <option key={m} value={m}>{m}</option>
                  ))}
                  <option value="__custom__">custom…</option>
                </select>
              </div>
            </div>
            {s.model === '__custom__' && (
              <input
                value={s.customModel}
                onChange={(e) => s.setCustomModel(e.target.value)}
                placeholder="model id, e.g. llama3.1"
                className="field animate-fade-in"
              />
            )}

            {/* Advanced */}
            <div className="rounded-xl border border-line">
              <button
                type="button"
                onClick={() => setShowAdvanced((v) => !v)}
                className="flex w-full cursor-pointer items-center justify-between px-4 py-2.5 text-xs font-medium text-ink-2 transition hover:text-ink"
              >
                Advanced options
                <ChevronIcon
                  size={14}
                  className={`transition-transform duration-200 ${showAdvanced ? 'rotate-180' : ''}`}
                />
              </button>
              {showAdvanced && (
                <div className="animate-fade-in border-t border-line px-4 py-4">
                  <label className={labelCls}>
                    System prompt <span className="normal-case text-ink-3/60">(optional)</span>
                  </label>
                  <textarea
                    value={s.system}
                    onChange={(e) => s.setSystem(e.target.value)}
                    rows={3}
                    placeholder="You are a helpful assistant."
                    className="field resize-y"
                  />
                </div>
              )}
            </div>

            {s.error && (
              <ErrorBanner message={s.error} icon={<AlertIcon size={14} />} />
            )}

            <button
              onClick={s.createAgent}
              disabled={s.creating}
              className={`${primaryBtnCls} w-full py-3 text-[15px]`}
            >
              {s.creating ? (
                <>
                  <span className="typing-dot inline-block h-1.5 w-1.5 rounded-full bg-white" />
                  <span className="typing-dot inline-block h-1.5 w-1.5 rounded-full bg-white" style={{ animationDelay: '0.15s' }} />
                  <span className="typing-dot inline-block h-1.5 w-1.5 rounded-full bg-white" style={{ animationDelay: '0.3s' }} />
                  <span className="ml-1">Creating on daemon…</span>
                </>
              ) : (
                <>
                  <SparkIcon size={16} />
                  Create agent
                </>
              )}
            </button>

            {s.online === false && (
              <p className="flex items-center justify-center gap-1.5 text-center text-xs text-red-400">
                <AlertIcon size={13} />
                daemon offline — start it with <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-[11px]">bun run daemon</code>
              </p>
            )}
          </div>
        </div>

        <p className="mt-5 text-center font-mono text-[11px] text-ink-3/70">
          backed by anima-daemon · keys live in the daemon env
        </p>
      </div>
    </div>
  );
}
