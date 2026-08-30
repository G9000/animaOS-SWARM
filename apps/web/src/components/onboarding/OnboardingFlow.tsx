import { useEffect, useRef, useState } from 'react';

import {
  toolNamesForProfile,
  type AccessProfile,
} from '../../lib/agent-access';
import {
  presetById,
  presetTemplate,
  type PresetId,
} from '../../lib/agent-presets';
import {
  daemon,
  MODEL_SUGGESTIONS,
  PROFILE_GENERATION_UNAVAILABLE,
  type DaemonProvider,
  type DaemonSnapshot,
} from '../../lib/daemon-api';
import { AccessStep } from './AccessStep';
import { AgentStep } from './AgentStep';
import { ModelStep, type ProviderCatalogState } from './ModelStep';
import { ONBOARDING_STEPS, OnboardingProgress } from './OnboardingProgress';
import { ReviewStep } from './ReviewStep';
import {
  WorkspaceStep,
  type WorkspaceVerifyStatus,
} from './WorkspaceStep';

export interface OnboardingFlowProps {
  providers: DaemonProvider[] | null;
  providersError: string | null;
  retryProviders(): void | Promise<void>;
  onCreated(snapshot: DaemonSnapshot): void;
}

interface WorkspaceDraft {
  companyName: string;
  mission: string;
  rootPath: string;
  values: string[];
}

interface OnboardingDraft {
  workspace: WorkspaceDraft;
  name: string;
  presetId: PresetId;
  intent: string;
  bio: string;
  adjectives: string[];
  style: string;
  system: string;
  provider: string;
  model: string;
  customModel: string;
  access: AccessProfile;
}

const INITIAL_DRAFT: OnboardingDraft = {
  workspace: { companyName: '', mission: '', rootPath: '', values: [] },
  name: 'Anima',
  presetId: 'chief-of-staff',
  intent: '',
  bio: '',
  adjectives: [],
  style: '',
  system: '',
  provider: '',
  model: '',
  customModel: '',
  access: 'collaborate',
};

const PROVIDER_CATALOG_CHANGED_ERROR =
  'Provider catalog changed. Review your provider and model before creating the agent.';
const WORKSPACE_REQUIRED_ERROR =
  'Enter a company name, mission, and workspace folder.';
const NAME_REQUIRED_ERROR = 'Enter an agent name.';
const MODEL_REQUIRED_ERROR = 'Enter a model.';
const WORKSPACE_ERROR_ID = 'onboarding-workspace-error';
const NAME_ERROR_ID = 'onboarding-agent-name-error';
const CUSTOM_MODEL_ERROR_ID = 'onboarding-custom-model-error';

function defaultModel(provider: string): string {
  return MODEL_SUGGESTIONS[provider]?.[0] ?? '__custom__';
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function templateContext(draft: OnboardingDraft) {
  return {
    companyName: draft.workspace.companyName.trim(),
    mission: draft.workspace.mission.trim(),
    agentName: draft.name.trim(),
  };
}

/** Fill only the profile fields that are still empty from the preset template. */
function fillEmptyFromTemplate(
  draft: OnboardingDraft,
  presetId: PresetId = draft.presetId,
): OnboardingDraft {
  const template = presetTemplate(presetId, templateContext(draft));
  return {
    ...draft,
    presetId,
    bio: draft.bio || template.bio,
    adjectives: draft.adjectives.length ? draft.adjectives : template.adjectives,
    style: draft.style || template.style,
    system: draft.system || template.system,
  };
}

function workspaceComplete(workspace: WorkspaceDraft): boolean {
  return Boolean(
    workspace.companyName.trim() &&
      workspace.mission.trim() &&
      workspace.rootPath.trim(),
  );
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
  const [verifying, setVerifying] = useState(false);
  const [verifyStatus, setVerifyStatus] =
    useState<WorkspaceVerifyStatus | null>(null);
  const [generating, setGenerating] = useState(false);
  const [generateAvailable, setGenerateAvailable] = useState(true);
  const [generateError, setGenerateError] = useState<string | null>(null);
  const [providersRetrying, setProvidersRetrying] = useState(false);
  const [providerRetryError, setProviderRetryError] = useState<string | null>(
    null,
  );
  const companyInputRef = useRef<HTMLInputElement>(null);
  const missionInputRef = useRef<HTMLInputElement>(null);
  const rootPathInputRef = useRef<HTMLInputElement>(null);
  const nameInputRef = useRef<HTMLInputElement>(null);
  const modelSelectRef = useRef<HTMLSelectElement>(null);
  const customModelInputRef = useRef<HTMLInputElement>(null);
  const providerRetryInFlightRef = useRef(false);
  const verifyInFlightRef = useRef(false);
  const verifyRequestIdRef = useRef(0);
  const generateInFlightRef = useRef(false);
  const submitInFlightRef = useRef(false);
  const mountedRef = useRef(false);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  // Pre-fill the workspace folder from the daemon's default root. Failure is
  // non-blocking: the field simply stays empty for manual entry.
  useEffect(() => {
    let active = true;
    daemon
      .getWorkspace()
      .then((state) => {
        if (!active) {
          return;
        }
        setDraft((current) =>
          current.workspace.rootPath
            ? current
            : {
                ...current,
                workspace: {
                  ...current.workspace,
                  rootPath: state.defaultRoot,
                },
              },
        );
      })
      .catch(() => undefined);
    return () => {
      active = false;
    };
  }, []);

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
  const workspaceValidationErrorId =
    currentStep === 0 && blockingError === WORKSPACE_REQUIRED_ERROR
      ? WORKSPACE_ERROR_ID
      : undefined;
  const customModelValidationErrorId =
    currentStep === 1 &&
    draft.model === '__custom__' &&
    blockingError === MODEL_REQUIRED_ERROR
      ? CUSTOM_MODEL_ERROR_ID
      : undefined;
  const nameValidationErrorId =
    currentStep === 2 && blockingError === NAME_REQUIRED_ERROR
      ? NAME_ERROR_ID
      : undefined;
  const blockingErrorId =
    workspaceValidationErrorId ??
    customModelValidationErrorId ??
    nameValidationErrorId;

  useEffect(() => {
    if (currentStep < 2 || intelligenceReady) {
      return;
    }

    setCreateError(null);
    setBlockingError(PROVIDER_CATALOG_CHANGED_ERROR);
    setCurrentStep(1);
  }, [currentStep, intelligenceReady]);

  const focusFirstEmptyWorkspaceField = () => {
    if (!draft.workspace.companyName.trim()) {
      companyInputRef.current?.focus();
      return;
    }
    if (!draft.workspace.mission.trim()) {
      missionInputRef.current?.focus();
      return;
    }
    if (!draft.workspace.rootPath.trim()) {
      rootPathInputRef.current?.focus();
    }
  };

  useEffect(() => {
    if (!blockingError) {
      return;
    }

    if (currentStep === 0 && !workspaceComplete(draft.workspace)) {
      focusFirstEmptyWorkspaceField();
      return;
    }

    if (currentStep === 1 && intelligenceReady && !resolvedModel) {
      if (draft.model === '__custom__') {
        customModelInputRef.current?.focus();
      } else {
        modelSelectRef.current?.focus();
      }
      return;
    }

    if (currentStep === 2 && !draft.name.trim()) {
      nameInputRef.current?.focus();
    }
  }, [
    blockingError,
    currentStep,
    draft.model,
    draft.name,
    draft.workspace,
    intelligenceReady,
    resolvedModel,
  ]);

  const updateDraft = <Key extends keyof OnboardingDraft>(
    key: Key,
    value: OnboardingDraft[Key],
  ) => {
    setDraft((current) => ({ ...current, [key]: value }));
    setBlockingError(null);
  };

  const updateWorkspace = <Key extends keyof WorkspaceDraft>(
    key: Key,
    value: WorkspaceDraft[Key],
  ) => {
    setDraft((current) => ({
      ...current,
      workspace: { ...current.workspace, [key]: value },
    }));
    setBlockingError(null);
  };

  const changeRootPath = (rootPath: string) => {
    // Invalidate any in-flight verify: its result was computed for the old
    // path and must not surface as status for the new one.
    verifyRequestIdRef.current += 1;
    updateWorkspace('rootPath', rootPath);
    setVerifyStatus(null);
  };

  const changeProvider = (provider: string) => {
    setDraft((current) => ({
      ...current,
      provider,
      model: defaultModel(provider),
    }));
    setBlockingError(null);
  };

  const changePreset = (presetId: PresetId) => {
    setDraft((current) => {
      const untouched =
        !current.bio &&
        current.adjectives.length === 0 &&
        !current.style &&
        !current.system;
      return untouched
        ? fillEmptyFromTemplate(current, presetId)
        : { ...current, presetId };
    });
    setBlockingError(null);
  };

  const goBack = () => {
    setBlockingError(null);
    setCreateError(null);
    setCurrentStep((step) => Math.max(0, step - 1));
  };

  const goNext = () => {
    setBlockingError(null);

    if (currentStep === 0 && !workspaceComplete(draft.workspace)) {
      setBlockingError(WORKSPACE_REQUIRED_ERROR);
      focusFirstEmptyWorkspaceField();
      return;
    }

    if (currentStep === 1) {
      if (!intelligenceReady) {
        return;
      }

      if (!resolvedModel) {
        setBlockingError(MODEL_REQUIRED_ERROR);
        if (draft.model === '__custom__') {
          customModelInputRef.current?.focus();
        } else {
          modelSelectRef.current?.focus();
        }
        return;
      }
    }

    if (currentStep === 2) {
      if (!draft.name.trim()) {
        setBlockingError(NAME_REQUIRED_ERROR);
        nameInputRef.current?.focus();
        return;
      }

      if (!draft.system.trim()) {
        // Guarantee a valid profile before Review: fill any still-empty
        // profile fields (system, bio, adjectives, style) from the template.
        setDraft((current) => fillEmptyFromTemplate(current));
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
      if (mountedRef.current) {
        setProviderRetryError(errorMessage(error));
      }
    } finally {
      providerRetryInFlightRef.current = false;
      if (mountedRef.current) {
        setProvidersRetrying(false);
      }
    }
  };

  const verifyWorkspace = async () => {
    if (verifyInFlightRef.current) {
      return;
    }

    const requestId = ++verifyRequestIdRef.current;
    verifyInFlightRef.current = true;
    setVerifying(true);
    setVerifyStatus(null);
    try {
      const response = await daemon.validateWorkspace({
        rootPath: draft.workspace.rootPath.trim(),
        companyName: draft.workspace.companyName.trim(),
        mission: draft.workspace.mission.trim(),
        values: draft.workspace.values,
      });
      // Bail when the root path changed while the request was in flight —
      // this result describes a folder the draft no longer points at.
      if (!mountedRef.current || requestId !== verifyRequestIdRef.current) {
        return;
      }
      setVerifyStatus({
        ok: true,
        willCreate: response.rootPathExists === false,
      });
    } catch (error) {
      if (mountedRef.current && requestId === verifyRequestIdRef.current) {
        setVerifyStatus({ ok: false, message: errorMessage(error) });
      }
    } finally {
      verifyInFlightRef.current = false;
      if (mountedRef.current) {
        setVerifying(false);
      }
    }
  };

  const generateProfile = async () => {
    if (generateInFlightRef.current) {
      return;
    }

    generateInFlightRef.current = true;
    setGenerating(true);
    setGenerateError(null);
    // Snapshot the profile fields so a successful generation only overwrites
    // fields the user has not touched while the request was in flight.
    const before = {
      bio: draft.bio,
      adjectives: draft.adjectives,
      style: draft.style,
      system: draft.system,
    };
    try {
      const { profile } = await daemon.generateProfile({
        presetId: draft.presetId,
        intent: draft.intent.trim(),
        provider: draft.provider,
        model: resolvedModel,
        workspace: {
          companyName: draft.workspace.companyName.trim(),
          mission: draft.workspace.mission.trim(),
          values: draft.workspace.values,
        },
      });
      if (!mountedRef.current) {
        return;
      }
      setDraft((current) => ({
        ...current,
        bio: current.bio === before.bio ? profile.bio : current.bio,
        adjectives:
          current.adjectives === before.adjectives
            ? profile.adjectives
            : current.adjectives,
        style: current.style === before.style ? profile.style : current.style,
        system:
          current.system === before.system ? profile.system : current.system,
      }));
    } catch (error) {
      if (!mountedRef.current) {
        return;
      }
      const message = errorMessage(error);
      if (message.startsWith(PROFILE_GENERATION_UNAVAILABLE)) {
        setGenerateAvailable(false);
        setDraft((current) => fillEmptyFromTemplate(current));
      } else {
        setGenerateError(message);
      }
    } finally {
      generateInFlightRef.current = false;
      if (mountedRef.current) {
        setGenerating(false);
      }
    }
  };

  const submit = async () => {
    if (submitInFlightRef.current) {
      return;
    }

    // The guards below are defense-in-depth: goNext already revalidates each
    // step before advancing, so submit can only be reached with a valid
    // draft. They protect against state changing while on Review (e.g. the
    // provider catalog refreshing underneath the user).
    if (!workspaceComplete(draft.workspace)) {
      setCreateError(null);
      setBlockingError(WORKSPACE_REQUIRED_ERROR);
      setCurrentStep(0);
      return;
    }

    const name = draft.name.trim();
    if (!name) {
      setCreateError(null);
      setBlockingError(NAME_REQUIRED_ERROR);
      setCurrentStep(2);
      return;
    }

    if (!intelligenceReady) {
      setCreateError(null);
      setBlockingError(PROVIDER_CATALOG_CHANGED_ERROR);
      setCurrentStep(1);
      return;
    }

    if (!resolvedModel) {
      setCreateError(null);
      setBlockingError(MODEL_REQUIRED_ERROR);
      setCurrentStep(1);
      return;
    }

    submitInFlightRef.current = true;
    setCreating(true);
    setCreateError(null);
    try {
      const response = await daemon.bootstrapWorkspace({
        workspace: {
          rootPath: draft.workspace.rootPath.trim(),
          companyName: draft.workspace.companyName.trim(),
          mission: draft.workspace.mission.trim(),
          values: draft.workspace.values,
        },
        agent: {
          name,
          presetId: draft.presetId,
          bio: draft.bio.trim(),
          ...(draft.adjectives.length
            ? { adjectives: draft.adjectives }
            : {}),
          ...(draft.style.trim() ? { style: draft.style.trim() } : {}),
          system: draft.system.trim(),
          ...(draft.provider ? { provider: draft.provider } : {}),
          model: resolvedModel,
          tools: toolNamesForProfile(draft.access),
        },
      });
      if (mountedRef.current) {
        onCreated(response.agent);
      }
    } catch (error) {
      if (mountedRef.current) {
        setCreateError(errorMessage(error));
      }
    } finally {
      submitInFlightRef.current = false;
      if (mountedRef.current) {
        setCreating(false);
      }
    }
  };

  let stepContent;
  switch (currentStep) {
    case 0:
      stepContent = (
        <WorkspaceStep
          companyName={draft.workspace.companyName}
          mission={draft.workspace.mission}
          rootPath={draft.workspace.rootPath}
          values={draft.workspace.values}
          verifying={verifying}
          verifyStatus={verifyStatus}
          onCompanyNameChange={(value) => updateWorkspace('companyName', value)}
          onMissionChange={(value) => updateWorkspace('mission', value)}
          onRootPathChange={changeRootPath}
          onValuesChange={(values) => updateWorkspace('values', values)}
          onVerify={() => void verifyWorkspace()}
          companyInputRef={companyInputRef}
          missionInputRef={missionInputRef}
          rootPathInputRef={rootPathInputRef}
          validationErrorId={workspaceValidationErrorId}
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
          customModelValidationErrorId={customModelValidationErrorId}
        />
      );
      break;
    case 2:
      stepContent = (
        <AgentStep
          name={draft.name}
          presetId={draft.presetId}
          intent={draft.intent}
          bio={draft.bio}
          adjectives={draft.adjectives}
          style={draft.style}
          system={draft.system}
          generating={generating}
          generateAvailable={generateAvailable}
          generateError={generateError}
          onNameChange={(name) => updateDraft('name', name)}
          onPresetChange={changePreset}
          onIntentChange={(intent) => updateDraft('intent', intent)}
          onBioChange={(bio) => updateDraft('bio', bio)}
          onStyleChange={(style) => updateDraft('style', style)}
          onSystemChange={(system) => updateDraft('system', system)}
          onGenerate={() => void generateProfile()}
          nameInputRef={nameInputRef}
          validationErrorId={nameValidationErrorId}
        />
      );
      break;
    case 3:
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
          workspace={draft.workspace}
          presetLabel={presetById(draft.presetId)?.label ?? draft.presetId}
          bio={draft.bio}
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
    <div className="relative z-[1] flex flex-1 items-center justify-center overflow-y-auto px-4 py-6 sm:px-6 sm:py-10">
      <div className="w-full max-w-2xl space-y-6">
        <header className="space-y-2 text-center">
          <p className="font-mono text-[10px] uppercase tracking-[0.22em] text-ink-3">
            Guided Focus · Workspace
          </p>
          <h1 className="font-display text-3xl font-semibold tracking-[-0.035em] text-ink sm:text-4xl">
            Set up your workspace
          </h1>
          <p className="mx-auto max-w-lg text-sm leading-relaxed text-ink-2">
            Name your company, pick its folder, and hire your first agent.
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

        <div className="glass-strong rounded-3xl p-5 shadow-2xl shadow-black/60 sm:p-8">
          {stepContent}

          {blockingError ? (
            <p
              id={blockingErrorId}
              role="alert"
              className="mt-5 rounded-xl border border-danger/30 bg-danger/5 p-3 text-sm text-danger"
            >
              {blockingError}
            </p>
          ) : null}

          {currentStep < ONBOARDING_STEPS.length - 1 ? (
            <div className="mt-6 flex items-center justify-between gap-3">
              {currentStep > 0 ? (
                <button
                  type="button"
                  className="rounded-xl border border-line bg-white/[0.02] px-4 py-2 text-sm font-medium text-ink-2 transition hover:border-line-strong hover:text-ink"
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
                className="rounded-xl bg-accent px-5 py-2 text-sm font-semibold text-abyss shadow-lg shadow-accent/20 transition hover:bg-accent/90 disabled:cursor-not-allowed disabled:opacity-50 disabled:shadow-none"
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
