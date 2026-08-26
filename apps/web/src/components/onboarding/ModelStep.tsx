import type { RefObject } from 'react';

import { MODEL_SUGGESTIONS, type DaemonProvider } from '../../lib/daemon-api';
import { labelCls } from '../ui-bits';

export interface ModelStepProps {
  providers: DaemonProvider[] | null;
  providersError: string | null;
  providersRetrying: boolean;
  provider: string;
  model: string;
  customModel: string;
  onProviderChange(provider: string): void;
  onModelChange(model: string): void;
  onCustomModelChange(model: string): void;
  onRetryProviders(): void;
  providerGroupRef: RefObject<HTMLDivElement | null>;
  modelSelectRef: RefObject<HTMLSelectElement | null>;
  customModelInputRef: RefObject<HTMLInputElement | null>;
}

function providerGuidance(provider: DaemonProvider): string {
  if (provider.configured) {
    return 'Configured';
  }

  if (!provider.requiresKey) {
    return 'Unavailable in the daemon';
  }

  const envName = provider.apiKeyEnvs[0] ?? 'the provider API key';
  return `Unavailable: set ${envName} in the daemon environment`;
}

export function ModelStep({
  providers,
  providersError,
  providersRetrying,
  provider,
  model,
  customModel,
  onProviderChange,
  onModelChange,
  onCustomModelChange,
  onRetryProviders,
  providerGroupRef,
  modelSelectRef,
  customModelInputRef,
}: ModelStepProps) {
  const modelSuggestions = MODEL_SUGGESTIONS[provider] ?? [];
  const providerCatalogBusy =
    providersRetrying || (providers === null && !providersError);

  return (
    <section aria-labelledby="onboarding-model-heading" className="space-y-5">
      <div>
        <h2
          id="onboarding-model-heading"
          className="font-display text-2xl font-semibold text-ink"
        >
          Intelligence
        </h2>
        <p className="mt-1 text-sm text-ink-2">
          Choose a provider configured on the daemon and a model to run.
        </p>
      </div>

      <div
        ref={providerGroupRef}
        role="group"
        aria-label="Provider catalog"
        aria-busy={providerCatalogBusy}
        tabIndex={-1}
        className="space-y-3 outline-none"
      >
        <p className={labelCls}>Provider</p>
        {providerCatalogBusy ? (
          <p className="rounded-xl border border-dashed border-line px-4 py-3 text-sm text-ink-3">
            Loading provider catalog…
          </p>
        ) : providersError ? (
          <div className="space-y-3 rounded-xl border border-red-400/30 bg-red-400/5 p-4">
            <p role="alert" className="text-sm text-red-300">
              {providersError}
            </p>
            <button
              type="button"
              className="rounded-lg border border-line px-3 py-2 text-sm text-ink"
              onClick={onRetryProviders}
            >
              Retry providers
            </button>
          </div>
        ) : (
          <div className="grid gap-2 sm:grid-cols-2">
            {(providers ?? []).map((candidate) => {
              const guidance = providerGuidance(candidate);
              const selected = candidate.id === provider;

              return (
                <button
                  key={candidate.id}
                  type="button"
                  disabled={!candidate.configured}
                  aria-pressed={selected}
                  onClick={() => onProviderChange(candidate.id)}
                  className={`rounded-xl border p-3 text-left disabled:cursor-not-allowed disabled:opacity-60 ${
                    selected
                      ? 'border-sky-400/60 bg-sky-400/10'
                      : 'border-line bg-white/[0.02]'
                  }`}
                >
                  <span className="block text-sm font-medium text-ink">
                    {candidate.label}
                  </span>
                  <span className="mt-1 block text-xs text-ink-3">
                    {guidance}
                  </span>
                </button>
              );
            })}
          </div>
        )}
      </div>

      <div>
        <label htmlFor="onboarding-model" className={labelCls}>
          Model
        </label>
        <select
          ref={modelSelectRef}
          id="onboarding-model"
          className="field"
          value={model}
          disabled={!provider}
          onChange={(event) => onModelChange(event.target.value)}
        >
          <option value="" disabled>
            Choose a model
          </option>
          {modelSuggestions.map((suggestion) => (
            <option key={suggestion} value={suggestion}>
              {suggestion}
            </option>
          ))}
          <option value="__custom__">Custom model</option>
        </select>
      </div>

      {model === '__custom__' ? (
        <div>
          <label htmlFor="onboarding-custom-model" className={labelCls}>
            Custom model
          </label>
          <input
            ref={customModelInputRef}
            id="onboarding-custom-model"
            className="field"
            value={customModel}
            onChange={(event) => onCustomModelChange(event.target.value)}
            placeholder="Provider model identifier"
          />
        </div>
      ) : null}
    </section>
  );
}
