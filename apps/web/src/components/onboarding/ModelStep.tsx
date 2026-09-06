import { useEffect, useRef, type RefObject } from 'react';

import { MODEL_SUGGESTIONS, type DaemonProvider } from '../../lib/daemon-api';
import { labelCls } from '../ui-bits';
import { ChatGptConnection } from '../ChatGptConnection';

export interface ModelStepProps {
  providers: DaemonProvider[] | null;
  catalogState: ProviderCatalogState;
  providerError: string | null;
  provider: string;
  model: string;
  customModel: string;
  onProviderChange(provider: string): void;
  onModelChange(model: string): void;
  onCustomModelChange(model: string): void;
  onRetryProviders(): void;
  modelSelectRef: RefObject<HTMLSelectElement | null>;
  customModelInputRef: RefObject<HTMLInputElement | null>;
  customModelValidationErrorId?: string;
}

export type ProviderCatalogState =
  | 'loading'
  | 'retrying'
  | 'error'
  | 'empty'
  | 'ready';

function providerGuidance(provider: DaemonProvider): string {
  if (provider.id === 'chatgpt')
    return provider.configured
      ? 'ChatGPT connected'
      : 'Connect your ChatGPT subscription below';
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
  catalogState,
  providerError,
  provider,
  model,
  customModel,
  onProviderChange,
  onModelChange,
  onCustomModelChange,
  onRetryProviders,
  modelSelectRef,
  customModelInputRef,
  customModelValidationErrorId,
}: ModelStepProps) {
  const modelSuggestions = MODEL_SUGGESTIONS[provider] ?? [];
  const selectedProviderRef = useRef<HTMLButtonElement>(null);
  const retryButtonRef = useRef<HTMLButtonElement>(null);
  const providerCatalogBusy =
    catalogState === 'loading' || catalogState === 'retrying';
  const chatGpt = providers?.find((candidate) => candidate.id === 'chatgpt');

  useEffect(() => {
    if (catalogState === 'error' || catalogState === 'empty') {
      retryButtonRef.current?.focus();
      return;
    }

    if (catalogState === 'ready') {
      selectedProviderRef.current?.focus();
    }
  }, [catalogState, provider]);

  return (
    <section aria-labelledby="onboarding-model-heading" className="space-y-5">
      <div>
        <h2
          id="onboarding-model-heading"
          className="font-display text-2xl font-semibold tracking-tight text-ink"
        >
          Model
        </h2>
        <p className="mt-1 text-sm text-ink-2">
          Choose the AI model your manager and specialists will use.
        </p>
      </div>

      {chatGpt && (
        <div className="space-y-3 rounded-2xl border border-accent/30 bg-accent/[0.04] p-4">
          <div>
            <h3 className="text-base font-semibold">Start with your ChatGPT subscription</h3>
            <p className="mt-1 text-sm text-ink-2">
              Sign in and use your plan for your manager and specialists. No other
              AI provider connection or API key is needed.
            </p>
          </div>
          <ChatGptConnection onConnectionChange={onRetryProviders} />
          <button
            ref={provider === 'chatgpt' ? selectedProviderRef : undefined}
            type="button"
            disabled={!chatGpt.configured || catalogState !== 'ready'}
            aria-pressed={provider === 'chatgpt'}
            onClick={() => onProviderChange('chatgpt')}
            className="rounded-xl border border-accent/40 bg-accent/10 px-4 py-2.5 text-sm font-medium disabled:opacity-50"
          >
            {provider === 'chatgpt' ? 'ChatGPT subscription selected' : 'Use ChatGPT subscription'}
          </button>
          <p className="text-xs text-ink-3">
            {provider === 'chatgpt'
              ? 'Choose a model below, then continue. You can add other providers later.'
              : 'Connect above, then select your subscription to continue.'}
          </p>
        </div>
      )}

      <details open={!chatGpt || provider !== 'chatgpt'} className="space-y-3">
        <summary className="cursor-pointer text-sm font-medium text-ink-2">
          {chatGpt ? 'Other AI providers (optional)' : 'AI providers'}
        </summary>
      <div
        role="group"
        aria-label="Provider catalog"
        aria-busy={providerCatalogBusy}
        className="space-y-3"
      >
        <p className={labelCls}>Provider</p>
        {catalogState === 'loading' ? (
          <p
            role="status"
            className="rounded-xl border border-dashed border-line px-4 py-3 text-sm text-ink-3"
          >
            Loading provider catalog…
          </p>
        ) : catalogState === 'retrying' ? (
          <p
            role="status"
            className="rounded-xl border border-dashed border-line px-4 py-3 text-sm text-ink-3"
          >
            Retrying provider catalog…
          </p>
        ) : catalogState === 'error' ? (
          <div className="space-y-3 rounded-xl border border-danger/30 bg-danger/5 p-4">
            <p
              id="provider-catalog-error"
              role="alert"
              className="text-sm text-danger"
            >
              {providerError}
            </p>
            <button
              ref={retryButtonRef}
              type="button"
              aria-describedby="provider-catalog-error"
              className="rounded-xl border border-line bg-white/[0.02] px-3 py-2 text-sm text-ink transition hover:border-line-strong"
              onClick={onRetryProviders}
            >
              Retry providers
            </button>
          </div>
        ) : (
          <>
            <div className="grid gap-2 sm:grid-cols-2">
              {(providers ?? []).filter((candidate) => candidate.id !== 'chatgpt').map((candidate) => {
                const guidance = providerGuidance(candidate);
                const selected = candidate.id === provider;

                return (
                  <button
                    ref={
                      selected && candidate.configured
                        ? selectedProviderRef
                        : undefined
                    }
                    key={candidate.id}
                    type="button"
                    disabled={!candidate.configured}
                    aria-pressed={selected}
                    onClick={() => onProviderChange(candidate.id)}
                    className={`rounded-xl border p-3.5 text-left transition disabled:cursor-not-allowed disabled:opacity-55 ${
                      selected
                        ? 'border-accent/60 bg-accent/[0.08] shadow-[0_14px_36px_-28px_rgb(var(--color-accent-rgb)/0.8)]'
                        : 'border-line bg-white/[0.02] hover:border-line-strong hover:bg-white/[0.035]'
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
            {catalogState === 'empty' ? (
              <div className="space-y-3 rounded-xl border border-amber/30 bg-amber/5 p-4">
                <p id="provider-catalog-empty" className="text-sm text-ink-2">
                  {chatGpt
                    ? 'Connect your ChatGPT subscription above, or configure another provider and retry.'
                    : 'No providers are configured. Add a provider credential to the daemon environment, then retry.'}
                </p>
                <button
                  ref={retryButtonRef}
                  type="button"
                  aria-describedby="provider-catalog-empty"
                  className="rounded-xl border border-line bg-white/[0.02] px-3 py-2 text-sm text-ink transition hover:border-line-strong"
                  onClick={onRetryProviders}
                >
                  Retry providers
                </button>
              </div>
            ) : null}
          </>
        )}
      </div>
      </details>

      {catalogState === 'ready' ? (
        <>
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
                required
                aria-invalid={Boolean(customModelValidationErrorId)}
                aria-describedby={customModelValidationErrorId}
              />
            </div>
          ) : null}
        </>
      ) : null}
    </section>
  );
}
