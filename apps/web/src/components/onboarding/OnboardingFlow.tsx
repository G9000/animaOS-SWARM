import { useEffect, useRef, useState } from 'react';

import {
  toolNamesForProfile,
  type AccessProfile,
} from '../../lib/agent-access';
import {
  workspaceManagerProfile,
  type ManagerInitiative,
  type ManagerCommunication,
} from '../../lib/workspace-manager';
import {
  daemon,
  MODEL_SUGGESTIONS,
  type DaemonProvider,
  type DaemonSnapshot,
  type WorkspaceInspectFound,
} from '../../lib/daemon-api';
import { AccessStep } from './AccessStep';
import { WorkspaceManagerStep } from './WorkspaceManagerStep';
import { ModelStep, type ProviderCatalogState } from './ModelStep';
import { ONBOARDING_STEPS } from './OnboardingProgress';
import { OnboardingLayout } from './OnboardingLayout';
import { ResumeCard } from './ResumeCard';
import { ReviewStep } from './ReviewStep';
import { WorkspaceStep, type WorkspaceVerifyStatus } from './WorkspaceStep';
import { AgencyPicker } from './AgencyPicker';
import { AgencyTeam } from './AgencyTeam';
import {
  AGENCY_TEMPLATES,
  generatedMembers,
  teamError,
  templateMembers,
  type AgencyMember,
} from '../../lib/agency-templates';

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
  initiative: ManagerInitiative;
  communication: ManagerCommunication;
  priorities: string;
  agencyBrief: string;
  provider: string;
  model: string;
  customModel: string;
  access: AccessProfile;
}

const INITIAL_DRAFT: OnboardingDraft = {
  workspace: { companyName: '', mission: '', rootPath: '', values: [] },
  name: 'Anima',
  initiative: 'balanced',
  communication: 'concise',
  priorities: '',
  agencyBrief: '',
  provider: '',
  model: '',
  customModel: '',
  access: 'collaborate',
};

const PROVIDER_CATALOG_CHANGED_ERROR =
  'Provider catalog changed. Review your provider and model before creating the workspace manager.';
const WORKSPACE_REQUIRED_ERROR =
  'Enter a company name, workspace brief, and workspace folder.';
const NAME_REQUIRED_ERROR = 'Enter a manager name.';
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
  const [agencyChoice, setAgencyChoice] = useState('scratch');
  const [showAgencyPicker, setShowAgencyPicker] = useState(true);
  const [maxTeamSize, setMaxTeamSize] = useState(4);
  const [workers, setWorkers] = useState<AgencyMember[]>([]);
  const [generatingTeam, setGeneratingTeam] = useState(false);
  const [teamGenerationError, setTeamGenerationError] = useState<string | null>(
    null,
  );
  const [teamGenerated, setTeamGenerated] = useState(false);
  const teamRequestRef = useRef(0);
  const teamInFlightRef = useRef(false);
  const [workspaceConfigured, setWorkspaceConfigured] = useState(false);
  const [currentStep, setCurrentStep] = useState(0);
  const [blockingError, setBlockingError] = useState<string | null>(null);
  const [createError, setCreateError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [verifying, setVerifying] = useState(false);
  const [verifyStatus, setVerifyStatus] =
    useState<WorkspaceVerifyStatus | null>(null);
  const [resumeMode, setResumeMode] = useState(false);
  const [browsing, setBrowsing] = useState(false);
  const browseInFlightRef = useRef(false);
  const browseRequestIdRef = useRef(0);
  const [inspectPreview, setInspectPreview] =
    useState<WorkspaceInspectFound | null>(null);
  const [inspectNote, setInspectNote] = useState<string | null>(null);
  const [resuming, setResuming] = useState(false);
  const [resumeError, setResumeError] = useState<string | null>(null);
  const [providersRetrying, setProvidersRetrying] = useState(false);
  const [providerRetryError, setProviderRetryError] = useState<string | null>(
    null,
  );
  const companyInputRef = useRef<HTMLInputElement>(null);
  const missionInputRef = useRef<HTMLTextAreaElement>(null);
  const rootPathInputRef = useRef<HTMLInputElement>(null);
  const nameInputRef = useRef<HTMLInputElement>(null);
  const modelSelectRef = useRef<HTMLSelectElement>(null);
  const customModelInputRef = useRef<HTMLInputElement>(null);
  const providerRetryInFlightRef = useRef(false);
  const verifyInFlightRef = useRef(false);
  const verifyRequestIdRef = useRef(0);
  const inspectRequestIdRef = useRef(0);
  const resumeInFlightRef = useRef(false);
  const submitInFlightRef = useRef(false);
  const mountedRef = useRef(false);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  // Pre-fill the workspace folder from the daemon's default root. Failure is
  // non-blocking: the field simply stays empty for manual entry. When the
  // daemon reports an already-configured workspace (e.g. the user reset the
  // only agent), prefill the persisted workspace instead — submit will then
  // re-use createAgent rather than re-bootstrapping.
  useEffect(() => {
    let active = true;
    daemon
      .getWorkspace()
      .then((state) => {
        if (!active) {
          return;
        }
        if (state.configured && state.workspace) {
          const existing = state.workspace;
          setWorkspaceConfigured(true);
          setDraft((current) => ({
            ...current,
            workspace: {
              companyName: existing.companyName,
              mission: existing.mission,
              rootPath: existing.rootPath,
              values: existing.values,
            },
          }));
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
  const generationContext = JSON.stringify([
    draft.workspace,
    draft.provider,
    resolvedModel,
    agencyChoice,
    maxTeamSize,
  ]);
  const generationContextRef = useRef(generationContext);
  generationContextRef.current = generationContext;
  const selectedTemplate = AGENCY_TEMPLATES.find(
    (template) => template.id === agencyChoice,
  );
  const hasAgency = agencyChoice !== 'scratch' && !workspaceConfigured;
  const visibleSteps = hasAgency
    ? ONBOARDING_STEPS
    : ONBOARDING_STEPS.filter((step) => step !== 'Team');
  const visibleStepIndex =
    !hasAgency && currentStep > 1 ? currentStep - 1 : currentStep;
  const managerProfile = workspaceManagerProfile({
    name: draft.name,
    companyName: draft.workspace.companyName,
    mission: draft.workspace.mission,
    initiative: draft.initiative,
    communication: draft.communication,
    priorities: draft.priorities,
    agencyBrief: hasAgency ? draft.agencyBrief : '',
  });

  const selectAgency = (choice: string) => {
    teamRequestRef.current += 1;
    setAgencyChoice(choice);
    setShowAgencyPicker(false);
    setTeamGenerationError(null);
    setTeamGenerated(false);
    setBlockingError(null);
    const template = AGENCY_TEMPLATES.find(
      (candidate) => candidate.id === choice,
    );
    const members = template ? templateMembers(template) : [];
    setWorkers(members.slice(1));
    setDraft((current) => ({
      ...current,
      agencyBrief: members[0]?.system ?? '',
      workspace: template
        ? {
            ...current.workspace,
            companyName:
              !current.workspace.companyName ||
              current.workspace.companyName.startsWith('My ')
                ? `My ${template.name}`
                : current.workspace.companyName,
            mission: template.mission,
            values: [...template.values],
          }
        : current.workspace,
    }));
  };

  const generateTeam = async () => {
    if (teamInFlightRef.current || !intelligenceReady || !resolvedModel) return;
    teamInFlightRef.current = true;
    const requestId = ++teamRequestRef.current;
    const context = generationContextRef.current;
    setGeneratingTeam(true);
    setTeamGenerationError(null);
    try {
      const agency = await daemon.generateAgency({
        name: draft.workspace.companyName.trim(),
        description: draft.workspace.mission.trim(),
        maxTeamSize,
        provider: draft.provider,
        model: resolvedModel,
      });
      if (
        !mountedRef.current ||
        requestId !== teamRequestRef.current ||
        context !== generationContextRef.current
      )
        return;
      const [lead, ...specialists] = generatedMembers(agency);
      if (specialists.length + 1 > maxTeamSize || specialists.length === 0) {
        throw new Error(
          'The generated team is outside your selected size limit.',
        );
      }
      setDraft((current) => ({
        ...current,
        agencyBrief: selectedTemplate
          ? `${lead.system}\n\nReusable starter:\n${selectedTemplate.starter.content}`
          : lead.system,
      }));
      setWorkers(specialists);
      setTeamGenerated(true);
      setBlockingError(null);
    } catch (error) {
      if (
        mountedRef.current &&
        requestId === teamRequestRef.current &&
        context === generationContextRef.current
      ) {
        setTeamGenerationError(
          `${errorMessage(error)} Your current team is unchanged. Retry or go back to choose a template.`,
        );
      }
    } finally {
      teamInFlightRef.current = false;
      if (mountedRef.current) setGeneratingTeam(false);
    }
  };
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
    currentStep === 3 && blockingError === NAME_REQUIRED_ERROR
      ? NAME_ERROR_ID
      : undefined;
  const blockingErrorId =
    workspaceValidationErrorId ??
    customModelValidationErrorId ??
    nameValidationErrorId;
  // Belt-and-braces gate: even if a stale inspect ever slipped a preview into
  // state after the user left resume mode, the card only renders in mode.
  const showingResumeCard = resumeMode && inspectPreview !== null;

  useEffect(() => {
    if (currentStep < 2 || intelligenceReady) {
      return;
    }

    setCreateError(null);
    setBlockingError(PROVIDER_CATALOG_CHANGED_ERROR);
    setCurrentStep(1);
  }, [currentStep, intelligenceReady]);

  const focusFirstEmptyWorkspaceField = () => {
    // In resume mode only the folder field is rendered; the company/mission
    // inputs are hidden, so focusing them would be a no-op dead end.
    if (resumeMode) {
      rootPathInputRef.current?.focus();
      return;
    }
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

    if (currentStep === 3 && !draft.name.trim()) {
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
    resumeMode,
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
    browseRequestIdRef.current += 1;
    // Invalidate any in-flight verify/inspect: their results were computed for
    // the old path and must not surface as state for the new one.
    verifyRequestIdRef.current += 1;
    inspectRequestIdRef.current += 1;
    updateWorkspace('rootPath', rootPath);
    setVerifyStatus(null);
    setInspectPreview(null);
    setInspectNote(null);
    setResumeError(null);
  };

  const changeResumeMode = (mode: boolean) => {
    browseRequestIdRef.current += 1;
    // Invalidate any in-flight verify/inspect: their results describe the
    // mode being left and must not surface after the switch.
    verifyRequestIdRef.current += 1;
    inspectRequestIdRef.current += 1;
    setResumeMode(mode);
    setBlockingError(null);
    setVerifyStatus(null);
    setInspectPreview(null);
    setInspectNote(null);
    setResumeError(null);
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
    teamRequestRef.current += 1;
    browseRequestIdRef.current += 1;
    setBlockingError(null);
    setCreateError(null);
    setCurrentStep((step) =>
      step === 3 && !hasAgency ? 1 : Math.max(0, step - 1),
    );
  };

  const goNext = () => {
    browseRequestIdRef.current += 1;
    setBlockingError(null);

    // Resume mode hides the nav, so this guard is defense-in-depth: the
    // completeness check references fields that are hidden in resume mode.
    if (
      currentStep === 0 &&
      !resumeMode &&
      !workspaceComplete(draft.workspace)
    ) {
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
      if (hasAgency) {
        const error =
          agencyChoice === 'generate' && !teamGenerated
            ? 'Generate your team first, or go back to choose a template.'
            : workers.length + 1 > maxTeamSize
              ? 'Your preview exceeds the team size limit. Remove specialists or generate a new team.'
              : teamError(null, workers);
        if (error) {
          setBlockingError(error);
          return;
        }
      }
    }
    if (currentStep === 3) {
      if (!draft.name.trim()) {
        setBlockingError(NAME_REQUIRED_ERROR);
        nameInputRef.current?.focus();
        return;
      }
      if (hasAgency && teamError(draft.name, workers)) {
        setBlockingError(teamError(draft.name, workers));
        return;
      }
    }

    setCurrentStep((step) =>
      step === 1 && !hasAgency
        ? 3
        : Math.min(ONBOARDING_STEPS.length - 1, step + 1),
    );
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

  const inspectWorkspace = async (selectedPath?: string) => {
    const rootPath = (selectedPath ?? draft.workspace.rootPath).trim();
    // Reuse the verify in-flight guard + spinner: verify and inspect are
    // mutually exclusive modes, so one busy state covers both.
    if (!rootPath || verifyInFlightRef.current) {
      return;
    }

    const requestId = ++inspectRequestIdRef.current;
    verifyInFlightRef.current = true;
    setVerifying(true);
    setBlockingError(null);
    setInspectNote(null);
    setResumeError(null);
    try {
      const result = await daemon.inspectWorkspace(rootPath);
      // Bail when the root path changed while the request was in flight —
      // this preview describes a folder the draft no longer points at.
      if (!mountedRef.current || requestId !== inspectRequestIdRef.current) {
        return;
      }
      if (result.found) {
        setInspectPreview(result);
        setInspectNote(null);
      } else {
        setInspectNote('No workspace file found here — set up fresh below.');
      }
    } catch (error) {
      if (mountedRef.current && requestId === inspectRequestIdRef.current) {
        setInspectNote(errorMessage(error));
      }
    } finally {
      verifyInFlightRef.current = false;
      if (mountedRef.current) {
        setVerifying(false);
      }
    }
  };

  const browseWorkspace = async () => {
    if (browseInFlightRef.current || verifyInFlightRef.current) return;
    browseInFlightRef.current = true;
    const requestId = ++browseRequestIdRef.current;
    setBrowsing(true);
    setVerifyStatus(null);
    try {
      const { rootPath } = await daemon.pickWorkspaceFolder();
      if (
        !mountedRef.current ||
        requestId !== browseRequestIdRef.current ||
        !rootPath
      )
        return;
      changeRootPath(rootPath);
      if (resumeMode) await inspectWorkspace(rootPath);
    } catch (error) {
      if (mountedRef.current && requestId === browseRequestIdRef.current) {
        setVerifyStatus({ ok: false, message: errorMessage(error) });
      }
    } finally {
      browseInFlightRef.current = false;
      if (mountedRef.current) setBrowsing(false);
    }
  };

  const resumeWorkspace = async () => {
    if (resumeInFlightRef.current) {
      return;
    }

    const rootPath = draft.workspace.rootPath.trim();
    if (!rootPath) {
      return;
    }

    resumeInFlightRef.current = true;
    setResuming(true);
    setResumeError(null);
    try {
      const response = await daemon.resumeWorkspace(rootPath);
      if (mountedRef.current) {
        onCreated(response.orchestrator);
      }
    } catch (error) {
      if (mountedRef.current) {
        setResumeError(errorMessage(error));
      }
    } finally {
      resumeInFlightRef.current = false;
      if (mountedRef.current) {
        setResuming(false);
      }
    }
  };

  const setupFresh = () => {
    setInspectPreview(null);
    setResumeMode(false);
    setInspectNote(null);
    setResumeError(null);
    setBlockingError(null);
  };

  const submit = async () => {
    if (submitInFlightRef.current) {
      return;
    }
    if (hasAgency && teamError(draft.name, workers)) {
      setBlockingError(teamError(draft.name, workers));
      setCurrentStep(2);
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
      setCurrentStep(3);
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
      if (workspaceConfigured) {
        // The workspace is already bootstrapped (bootstrapWorkspace would 409):
        // hire the agent into the existing workspace via the classic route.
        // Note: createAgent's wire type carries no bio/adjectives/style, so
        // those profile fields are folded into `system` only.
        const response = await daemon.createAgent({
          settings: { additional: { workspaceRole: 'lead' } },
          name,
          ...(draft.provider ? { provider: draft.provider } : {}),
          model: resolvedModel,
          system: managerProfile.system,
          tools: toolNamesForProfile(draft.access),
        });
        if (mountedRef.current) {
          onCreated(response.agent);
        }
        return;
      }

      const response = await daemon.bootstrapWorkspace({
        ...(hasAgency
          ? {
              workers: workers.map((worker) => ({
                ...worker,
                name: worker.name.trim(),
                bio: worker.bio.trim(),
                system: `Workspace: ${draft.workspace.companyName.trim()}\nMission: ${draft.workspace.mission.trim()}\n\n${worker.system.trim()}`,
                provider: draft.provider,
                model: resolvedModel,
                tools: toolNamesForProfile(draft.access),
              })),
            }
          : {}),
        workspace: {
          rootPath: draft.workspace.rootPath.trim(),
          companyName: draft.workspace.companyName.trim(),
          mission: draft.workspace.mission.trim(),
          values: draft.workspace.values,
        },
        agent: {
          name,
          presetId: 'chief-of-staff',
          ...managerProfile,
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
        <>
          {!resumeMode && !workspaceConfigured && showAgencyPicker && (
            <AgencyPicker selected={agencyChoice} onSelect={selectAgency} />
          )}
          {!resumeMode && !workspaceConfigured && !showAgencyPicker && (
            <div className="mb-6 flex items-center justify-between gap-3 rounded-xl border border-line bg-white/40 p-4">
              <div className="min-w-0">
                <p className="text-sm font-semibold text-ink">
                  {selectedTemplate?.name ??
                    (agencyChoice === 'generate'
                      ? 'Custom agency'
                      : 'Manager only')}
                </p>
                <p className="mt-1 text-xs text-ink-2">
                  {hasAgency
                    ? `Your manager and ${workers.length} specialists`
                    : 'Your own workspace, with Anima to help.'}
                </p>
              </div>
              <button
                type="button"
                className="shrink-0 text-sm font-medium text-accent"
                onClick={() => setShowAgencyPicker(true)}
              >
                Change template
              </button>
            </div>
          )}
          <WorkspaceStep
            companyName={draft.workspace.companyName}
            mission={draft.workspace.mission}
            rootPath={draft.workspace.rootPath}
            values={draft.workspace.values}
            verifying={verifying}
            verifyStatus={verifyStatus}
            onCompanyNameChange={(value) =>
              updateWorkspace('companyName', value)
            }
            onMissionChange={(value) => updateWorkspace('mission', value)}
            onRootPathChange={changeRootPath}
            onValuesChange={(values) => updateWorkspace('values', values)}
            onVerify={() => void verifyWorkspace()}
            browsing={browsing}
            onBrowse={() => void browseWorkspace()}
            resumeMode={resumeMode}
            onResumeModeChange={changeResumeMode}
            onInspect={() => void inspectWorkspace()}
            companyInputRef={companyInputRef}
            missionInputRef={missionInputRef}
            rootPathInputRef={rootPathInputRef}
            validationErrorId={workspaceValidationErrorId}
          />
          {hasAgency && !resumeMode && (
            <p className="mt-3 text-xs leading-relaxed text-ink-3">
              Use the workspace brief to describe your audience, goals, content
              channels, or routines.{' '}
              {agencyChoice === 'generate'
                ? 'Choose your model next, then generate and review your team.'
                : 'Your template includes a full team. Personalize it after choosing your model.'}
            </p>
          )}
        </>
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
        <>
          {hasAgency && (
            <div className="mb-5 space-y-3 rounded-2xl border border-accent/25 bg-accent/[0.04] p-4">
              <h2 className="font-display text-xl font-semibold text-ink">
                Shape your team
              </h2>
              <p className="text-sm text-ink-2">
                {agencyChoice === 'generate'
                  ? 'Propose specialists and agency responsibilities from your workspace brief, then review them below.'
                  : 'Your template is ready. Keep it as-is, edit the team below, or generate a new proposal from your workspace brief.'}
              </p>
              <p className="text-xs text-ink-3">
                Generation replaces this team preview and uses your selected
                model.
              </p>
              <div className="space-y-2 py-3">
                <label
                  htmlFor="onboarding-team-limit"
                  className="block text-sm font-medium text-ink"
                >
                  Maximum team size
                </label>
                <select
                  id="onboarding-team-limit"
                  className="field"
                  value={maxTeamSize}
                  disabled={generatingTeam}
                  onChange={(event) => {
                    setMaxTeamSize(Number(event.target.value));
                    setTeamGenerationError(null);
                    setBlockingError(null);
                  }}
                >
                  {Array.from({ length: 9 }, (_, index) => index + 2).map(
                    (size) => (
                      <option key={size} value={size}>
                        {size} agents total · 1 manager + up to {size - 1}{' '}
                        specialists
                      </option>
                    ),
                  )}
                </select>
                <p className="text-xs leading-relaxed text-ink-3">
                  AI chooses the smallest useful team from your brief, up to
                  this limit. This includes your workspace manager. Review or
                  remove specialists before creating.
                </p>
              </div>
              <button
                type="button"
                onClick={() => void generateTeam()}
                disabled={generatingTeam || draft.provider === 'deterministic'}
                className="rounded-xl bg-accent px-4 py-2 text-sm font-semibold text-abyss disabled:opacity-50"
              >
                {generatingTeam ? 'Generating team…' : 'Generate team'}
              </button>
              {draft.provider === 'deterministic' && (
                <p className="text-xs text-ink-3">
                  Choose a generative provider in Model to generate a team.
                  Ready-made templates work without generation.
                </p>
              )}
              {teamGenerationError && (
                <p role="alert" className="text-sm text-danger">
                  {teamGenerationError}
                </p>
              )}
              {teamGenerated && (
                <p role="status" className="text-sm text-mint">
                  Your team preview is ready. Review it before creating.
                </p>
              )}
            </div>
          )}
          <fieldset disabled={generatingTeam} className="min-w-0">
            {hasAgency && (
              <AgencyTeam
                workers={workers}
                onChange={(index, field, value) => {
                  setWorkers((current) =>
                    current.map((worker, i) =>
                      i === index ? { ...worker, [field]: value } : worker,
                    ),
                  );
                  setBlockingError(null);
                }}
                onRemove={(index) =>
                  setWorkers((current) => current.filter((_, i) => i !== index))
                }
              />
            )}
          </fieldset>
          {selectedTemplate && (
            <details className="mt-5 rounded-xl border border-line p-4">
              <summary className="cursor-pointer text-sm font-medium text-ink">
                Starter template: {selectedTemplate.starter.title}
              </summary>
              <p className="mt-2 text-xs text-ink-3">
                Included in your manager’s instructions. Ask your manager to
                start with this template.
              </p>
              <pre className="mt-3 whitespace-pre-wrap break-words text-xs leading-relaxed text-ink-2">
                {selectedTemplate.starter.content}
              </pre>
            </details>
          )}
        </>
      );
      break;
    case 3:
      stepContent = (
        <>
          <WorkspaceManagerStep
            name={draft.name}
            initiative={draft.initiative}
            communication={draft.communication}
            priorities={draft.priorities}
            instructions={managerProfile.system}
            onNameChange={(name) => updateDraft('name', name)}
            onInitiativeChange={(initiative) =>
              updateDraft('initiative', initiative)
            }
            onCommunicationChange={(communication) =>
              updateDraft('communication', communication)
            }
            onPrioritiesChange={(priorities) =>
              updateDraft('priorities', priorities)
            }
            nameInputRef={nameInputRef}
            validationErrorId={nameValidationErrorId}
          />

          <div className="mt-7 border-t border-line pt-6">
            <AccessStep
              access={draft.access}
              onAccessChange={(access) => updateDraft('access', access)}
            />
          </div>
        </>
      );
      break;
    default:
      stepContent = (
        <ReviewStep
          showActions={false}
          workers={hasAgency ? workers : undefined}
          workspace={draft.workspace}
          initiative={draft.initiative}
          communication={draft.communication}
          bio={managerProfile.bio}
          name={draft.name}
          system={managerProfile.system}
          provider={draft.provider}
          model={resolvedModel}
          access={draft.access}
          providers={providers}
          creating={creating}
          createError={createError}
          bootstrapsWorkspace={!workspaceConfigured}
          onBack={goBack}
          onSubmit={() => void submit()}
        />
      );
  }

  const creationSubject = workspaceConfigured
    ? 'manager'
    : hasAgency
      ? 'agency'
      : 'workspace';
  const title = resumeMode
    ? showingResumeCard
      ? 'Resume your workspace'
      : 'Open existing workspace'
    : 'Set up your workspace';
  const subtitle = resumeMode
    ? 'Pick up where you left off.'
    : workspaceConfigured
      ? 'Your workspace is ready — set up its manager.'
      : 'A little setup. A workspace that works your way.';

  return (
    <OnboardingLayout
      steps={visibleSteps}
      currentStep={visibleStepIndex}
      title={title}
      subtitle={subtitle}
      resumeMode={resumeMode}
      summary={{
        workspace: draft.workspace.companyName.trim() || 'Your workspace',
        template:
          selectedTemplate?.name ??
          (agencyChoice === 'generate' ? 'Custom agency' : 'Manager only'),
        team: hasAgency
          ? agencyChoice === 'generate' && !teamGenerated
            ? 'Team not generated yet'
            : `1 manager + ${workers.length} specialists`
          : '1 workspace manager',
      }}
      footer={
        !resumeMode ? (
          <div className="setup-actions">
            {currentStep > 0 ? (
              <button
                type="button"
                className="setup-back"
                onClick={goBack}
                disabled={creating}
              >
                Back
              </button>
            ) : (
              <span />
            )}
            <span className="setup-next-hint">
              {currentStep === 4
                ? 'Everything can be refined later.'
                : `Next: ${visibleSteps[visibleStepIndex + 1]}`}
            </span>
            <button
              type="button"
              className="setup-next"
              onClick={currentStep === 4 ? () => void submit() : goNext}
              disabled={
                creating ||
                generatingTeam ||
                (currentStep === 1 && !intelligenceReady)
              }
            >
              {currentStep === 4
                ? creating
                  ? `Creating ${creationSubject}…`
                  : `Create ${creationSubject}`
                : 'Next'}
            </button>
          </div>
        ) : undefined
      }
    >
      {currentStep === 0 && !workspaceConfigured && !resumeMode && (
        <button
          type="button"
          className="mb-5 text-sm font-medium text-accent"
          onClick={() => changeResumeMode(true)}
        >
          Open existing workspace
        </button>
      )}
      {showingResumeCard && inspectPreview ? (
        <ResumeCard
          preview={inspectPreview}
          rootPath={draft.workspace.rootPath.trim()}
          resuming={resuming}
          resumeError={resumeError}
          onResume={() => void resumeWorkspace()}
          onSetupFresh={setupFresh}
        />
      ) : (
        stepContent
      )}
      {!showingResumeCard && inspectNote && (
        <p className="mt-5 rounded-xl border border-line p-3 text-sm text-ink-2">
          {inspectNote}
        </p>
      )}
      {blockingError && !showingResumeCard && (
        <p
          id={blockingErrorId}
          role="alert"
          className="mt-5 rounded-xl border border-danger/30 bg-danger/5 p-3 text-sm text-danger"
        >
          {blockingError}
        </p>
      )}
    </OnboardingLayout>
  );
}
