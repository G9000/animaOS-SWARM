import { useEffect, useRef } from 'react';

import { ACCESS_PROFILES, type AccessProfile } from '../../lib/agent-access';
import type { DaemonProvider } from '../../lib/daemon-api';
import type { AgencyMember } from '../../lib/agency-templates';
import type {
  ManagerInitiative,
  ManagerCommunication,
} from '../../lib/workspace-manager';

export interface ReviewStepProps {
  showActions?: boolean;
  workers?: AgencyMember[];
  workspace: {
    companyName: string;
    mission: string;
    rootPath: string;
  };
  initiative: ManagerInitiative;
  communication: ManagerCommunication;
  bio: string;
  name: string;
  system: string;
  provider: string;
  model: string;
  access: AccessProfile;
  providers: DaemonProvider[] | null;
  creating: boolean;
  createError: string | null;
  /** true when submit bootstraps the workspace; false when it only creates an agent. */
  bootstrapsWorkspace?: boolean;
  onBack(): void;
  onSubmit(): void;
}

export function ReviewStep({
  showActions = true,
  workers,
  workspace,
  initiative,
  communication,
  bio,
  name,
  system,
  provider,
  model,
  access,
  providers,
  creating,
  createError,
  bootstrapsWorkspace = true,
  onBack,
  onSubmit,
}: ReviewStepProps) {
  const providerLabel =
    providers?.find((candidate) => candidate.id === provider)?.label ??
    provider;
  const accessProfile = ACCESS_PROFILES[access];
  const creationSubject = workers
    ? 'agency'
    : bootstrapsWorkspace
      ? 'workspace'
      : 'manager';
  const createErrorRef = useRef<HTMLParagraphElement>(null);

  useEffect(() => {
    if (createError) {
      createErrorRef.current?.focus();
    }
  }, [createError]);

  return (
    <section aria-labelledby="onboarding-review-heading" className="space-y-5">
      <div>
        <h2
          id="onboarding-review-heading"
          className="font-display text-2xl font-semibold tracking-tight text-ink"
        >
          Review
        </h2>
        <p className="mt-1 text-sm text-ink-2">
          {workers
            ? `Confirm your workspace manager and ${workers.length} specialists. The selected model and access apply to everyone.`
            : 'Confirm your workspace manager and how it will work with you.'}
        </p>
      </div>

      <div className="rounded-2xl border border-line bg-white/[0.02] p-4">
        <p className="text-sm font-semibold text-ink">
          <span aria-hidden="true">🏢 </span>
          {workspace.companyName.trim()}
        </p>
        <p className="mt-1 text-sm text-ink-2">{workspace.mission.trim()}</p>
        <p
          className="mt-1 truncate font-mono text-xs text-ink-3"
          title={workspace.rootPath}
        >
          {workspace.rootPath}
        </p>
      </div>

      <dl className="grid gap-3 rounded-2xl border border-line bg-abyss/35 p-4 sm:grid-cols-[10rem_1fr] sm:p-5">
        <dt className="text-xs uppercase tracking-wide text-ink-3">Name</dt>
        <dd className="text-sm text-ink">
          {name.trim()}
          {bio.trim() ? (
            <span className="mt-1 block text-xs leading-relaxed text-ink-3">
              {bio.trim()}
            </span>
          ) : null}
        </dd>

        <dt className="text-xs uppercase tracking-wide text-ink-3">Role</dt>
        <dd className="text-sm text-ink">Workspace Manager</dd>

        <dt className="text-xs uppercase tracking-wide text-ink-3">
          Initiative
        </dt>
        <dd className="text-sm capitalize text-ink">{initiative}</dd>

        <dt className="text-xs uppercase tracking-wide text-ink-3">
          Communication
        </dt>
        <dd className="text-sm capitalize text-ink">{communication}</dd>

        <dt className="text-xs uppercase tracking-wide text-ink-3">
          Provider / model
        </dt>
        <dd className="text-sm text-ink">
          {providerLabel} / {model}
        </dd>

        <dt className="text-xs uppercase tracking-wide text-ink-3">
          Access profile
        </dt>
        <dd className="text-sm text-ink">{accessProfile.label}</dd>

        <dt className="text-xs uppercase tracking-wide text-ink-3">
          Highest-risk capability
        </dt>
        <dd className="text-sm text-ink">{accessProfile.risk}</dd>

        <dt className="text-xs uppercase tracking-wide text-ink-3">
          Instructions
        </dt>
        <dd className="text-sm text-ink">
          <details>
            <summary className="cursor-pointer">
              View manager instructions
            </summary>
            <p className="mt-2 whitespace-pre-wrap text-xs leading-relaxed text-ink-2">
              {system}
            </p>
          </details>
        </dd>
      </dl>

      {workers && workers.length > 0 && (
        <div className="space-y-3">
          <h3 className="text-sm font-semibold text-ink">Specialists</h3>
          {workers.map((worker) => (
            <details
              key={worker.name}
              className="rounded-xl border border-line p-4"
            >
              <summary className="cursor-pointer text-sm font-medium text-ink">
                {worker.name}
              </summary>
              <p className="mt-2 text-sm text-ink-2">{worker.bio}</p>
              <p className="mt-2 whitespace-pre-wrap text-xs leading-relaxed text-ink-3">
                {worker.system}
              </p>
            </details>
          ))}
        </div>
      )}

      {createError ? (
        <p
          ref={createErrorRef}
          role="alert"
          tabIndex={-1}
          className="rounded-xl border border-danger/30 bg-danger/5 p-3 text-sm text-danger"
        >
          {createError}
        </p>
      ) : null}

      <p className="text-xs leading-relaxed text-ink-3">
        {bootstrapsWorkspace
          ? `Creates the workspace, the company file (anima.yaml), and your ${workers ? 'team' : 'manager'} in one step — if anything fails, nothing is half-created.`
          : 'Your workspace already exists — this step sets up its manager.'}
      </p>

      {showActions && (
        <div className="flex items-center justify-between gap-3">
          <button
            type="button"
            className="rounded-xl border border-line bg-white/[0.02] px-4 py-2 text-sm font-medium text-ink-2 transition hover:border-line-strong hover:text-ink"
            onClick={onBack}
            disabled={creating}
          >
            Back
          </button>
          <button
            type="button"
            className="rounded-xl bg-accent px-5 py-2 text-sm font-semibold text-abyss shadow-lg shadow-accent/20 transition hover:bg-accent/90 disabled:opacity-60 disabled:shadow-none"
            onClick={onSubmit}
            disabled={creating}
          >
            {creating
              ? `Creating ${creationSubject}…`
              : `Create ${creationSubject}`}
          </button>
        </div>
      )}
    </section>
  );
}
