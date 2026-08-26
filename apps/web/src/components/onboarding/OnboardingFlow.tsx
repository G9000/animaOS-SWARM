import { useEffect, useRef, useState } from 'react';

import {
  toolNamesForProfile,
  type AccessProfile,
} from '../../lib/agent-access';
import {
  daemon,
  MODEL_SUGGESTIONS,
  type DaemonProvider,
  type DaemonSnapshot,
} from '../../lib/daemon-api';
import { AccessStep } from './AccessStep';
import { IdentityStep } from './IdentityStep';
import { ModelStep, type ProviderCatalogState } from './ModelStep';
import { ONBOARDING_STEPS, OnboardingProgress } from './OnboardingProgress';
import { ReviewStep } from './ReviewStep';

export interface OnboardingFlowProps {
  providers: DaemonProvider[] | null;
  providersError: string | null;
  retryProviders(): void | Promise<void>;
  onCreated(snapshot: DaemonSnapshot): void;
}

interface OnboardingDraft {
  name: string;
  system: string;
  provider: string;
  model: string;
  customModel: string;
  access: AccessProfile;
}

const INITIAL_DRAFT: OnboardingDraft = {
  name: 'Anima',
  system: '',
  provider: '',
  model: '',
  customModel: '',
  access: 'collaborate',
};

function defaultModel(provider: string): string {
  return MODEL_SUGGESTIONS[provider]?.[0] ?? '__custom__';
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function OnboardingFlow({
  providers,
  providersError,
  retryProviders,
  onCreated,
}: OnboardingFlowProps) {
  const [draft, setDraft] = useState<OnboardingDraft>(INITIAL_DRAFT);
  const [currentStep, setCurrentStep] = useState(0);
  const [blockingError, setBlockingError] = useState<string | null>(null);
  const [createError, setCreateError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [providersRetrying, setProvidersRetrying] = useState(false);
  const [providerRetryError, setProviderRetryError] = useState<string | null>(
    null,
  );
  const nameInputRef = useRef<HTMLInputElement>(null);
  const modelSelectRef = useRef<HTMLSelectElement>(null);
  const customModelInputRef = useRef<HTMLInputElement>(null);
  const providerRetryInFlightRef = useRef(false);
  const submitInFlightRef = useRef(false);

  useEffect(() => {
    if (!providers) {
      return;
    }

    const selectedProvider = providers.find(
      (candidate) => candidate.id === draft.provider && candidate.configured,
    );
    if (selectedProvider) {
      return;
    }

    const firstConfigured = providers.find((candidate) => candidate.configured);
    if (!firstConfigured) {
      if (draft.provider || draft.model) {
        setDraft((current) => ({ ...current, provider: '', model: '' }));
      }
      return;
    }

    setDraft((current) => ({
      ...current,
      provider: firstConfigured.id,
      model: defaultModel(firstConfigured.id),
    }));
    if (currentStep === 1) {
      setBlockingError(null);
    }
  }, [currentStep, draft.model, draft.provider, providers]);

  useEffect(() => {
    if (providersError) {
      setProviderRetryError(null);
    }
  }, [providersError]);

  const resolvedModel =
    draft.model === '__custom__'
      ? draft.customModel.trim()
      : draft.model.trim();
  const providerError = providerRetryError ?? providersError;
  const selectedProviderConfigured =
    providers?.some(
      (candidate) => candidate.id === draft.provider && candidate.configured,
    ) ?? false;
  let providerCatalogState: ProviderCatalogState;
  if (providersRetrying) {
    providerCatalogState = 'retrying';
  } else if (providerError) {
    providerCatalogState = 'error';
  } else if (providers === null) {
    providerCatalogState = 'loading';
  } else if (!providers.some((candidate) => candidate.configured)) {
    providerCatalogState = 'empty';
  } else {
    providerCatalogState = 'ready';
  }
  const intelligenceReady =
    providerCatalogState === 'ready' && selectedProviderConfigured;

  const updateDraft = <Key extends keyof OnboardingDraft>(
    key: Key,
    value: OnboardingDraft[Key],
  ) => {
    setDraft((current) => ({ ...current, [key]: value }));
    setBlockingError(null);
  };

  const changeProvider = (provider: string) => {
    setDraft((current) => ({
      ...current,
      provider,
      model: defaultModel(provider),
    }));
    setBlockingError(null);
  };

  const goBack = () => {
    setBlockingError(null);
    setCreateError(null);
    setCurrentStep((step) => Math.max(0, step - 1));
  };

  const goNext = () => {
    setBlockingError(null);

    if (currentStep === 0 && !draft.name.trim()) {
      setBlockingError('Enter an agent name.');
      nameInputRef.current?.focus();
      return;
    }

    if (currentStep === 1) {
      if (!intelligenceReady) {
        return;
      }

      if (!resolvedModel) {
        setBlockingError('Enter a model.');
        if (draft.model === '__custom__') {
          customModelInputRef.current?.focus();
        } else {
          modelSelectRef.current?.focus();
        }
        return;
      }
    }

    setCurrentStep((step) => Math.min(ONBOARDING_STEPS.length - 1, step + 1));
  };

  const handleRetryProviders = async () => {
    if (providerRetryInFlightRef.current) {
      return;
    }

    providerRetryInFlightRef.current = true;
    setProviderRetryError(null);
    setProvidersRetrying(true);
    try {
      await retryProviders();
    } catch (error) {
      setProviderRetryError(errorMessage(error));
    } finally {
      providerRetryInFlightRef.current = false;
      setProvidersRetrying(false);
    }
  };

  const submit = async () => {
    if (submitInFlightRef.current) {
      return;
    }

    submitInFlightRef.current = true;
    setCreating(true);
    setCreateError(null);
    try {
      const system = draft.system.trim();
      const response = await daemon.createAgent({
        name: draft.name.trim(),
        provider: draft.provider,
        model: resolvedModel,
        tools: toolNamesForProfile(draft.access),
        ...(system ? { system } : {}),
      });
      onCreated(response.agent);
    } catch (error) {
      setCreateError(errorMessage(error));
    } finally {
      submitInFlightRef.current = false;
      setCreating(false);
    }
  };

  let stepContent;
  switch (currentStep) {
    case 0:
      stepContent = (
        <IdentityStep
          name={draft.name}
          system={draft.system}
          onNameChange={(name) => updateDraft('name', name)}
          onSystemChange={(system) => updateDraft('system', system)}
          nameInputRef={nameInputRef}
        />
      );
      break;
    case 1:
      stepContent = (
        <ModelStep
          providers={providers}
          catalogState={providerCatalogState}
          providerError={providerError}
          provider={draft.provider}
          model={draft.model}
          customModel={draft.customModel}
          onProviderChange={changeProvider}
          onModelChange={(model) => updateDraft('model', model)}
          onCustomModelChange={(model) => updateDraft('customModel', model)}
          onRetryProviders={() => void handleRetryProviders()}
          modelSelectRef={modelSelectRef}
          customModelInputRef={customModelInputRef}
        />
      );
      break;
    case 2:
      stepContent = (
        <AccessStep
          access={draft.access}
          onAccessChange={(access) => updateDraft('access', access)}
        />
      );
      break;
    default:
      stepContent = (
        <ReviewStep
          name={draft.name}
          system={draft.system}
          provider={draft.provider}
          model={resolvedModel}
          access={draft.access}
          providers={providers}
          creating={creating}
          createError={createError}
          onBack={goBack}
          onSubmit={() => void submit()}
        />
      );
  }

  return (
    <div className="flex flex-1 items-center justify-center overflow-y-auto px-6 py-10">
      <div className="w-full max-w-2xl space-y-6">
        <header className="space-y-2">
          <h1 className="font-display text-3xl font-bold text-ink">
            Create your main agent
          </h1>
          <p className="text-sm text-ink-2">
            Set the identity, intelligence, and workspace access for your agent.
          </p>
        </header>

        <OnboardingProgress currentStep={currentStep} />
        <p
          role="status"
          aria-live="polite"
          aria-atomic="true"
          className="sr-only"
        >
          Step {currentStep + 1} of {ONBOARDING_STEPS.length}:{' '}
          {ONBOARDING_STEPS[currentStep]}
        </p>

        <div className="glass-strong rounded-3xl p-7 shadow-2xl shadow-black/50">
          {stepContent}

          {blockingError ? (
            <p
              role="alert"
              className="mt-5 rounded-xl border border-red-400/30 bg-red-400/5 p-3 text-sm text-red-300"
            >
              {blockingError}
            </p>
          ) : null}

          {currentStep < ONBOARDING_STEPS.length - 1 ? (
            <div className="mt-6 flex items-center justify-between gap-3">
              {currentStep > 0 ? (
                <button
                  type="button"
                  className="rounded-lg border border-line px-4 py-2 text-sm text-ink"
                  onClick={goBack}
                >
                  Back
                </button>
              ) : (
                <span />
              )}
              <button
                type="button"
                disabled={currentStep === 1 && !intelligenceReady}
                className="rounded-lg bg-sky-500 px-4 py-2 text-sm font-medium text-white disabled:cursor-not-allowed disabled:opacity-50"
                onClick={goNext}
              >
                Next
              </button>
            </div>
          ) : null}
        </div>
      </div>
    </div>
  );
}
