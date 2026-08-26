import { ACCESS_PROFILES, type AccessProfile } from '../../lib/agent-access';
import type { DaemonProvider } from '../../lib/daemon-api';

export interface ReviewStepProps {
  name: string;
  system: string;
  provider: string;
  model: string;
  access: AccessProfile;
  providers: DaemonProvider[] | null;
  creating: boolean;
  createError: string | null;
  onBack(): void;
  onSubmit(): void;
}

export function ReviewStep({
  name,
  system,
  provider,
  model,
  access,
  providers,
  creating,
  createError,
  onBack,
  onSubmit,
}: ReviewStepProps) {
  const providerLabel =
    providers?.find((candidate) => candidate.id === provider)?.label ??
    provider;
  const accessProfile = ACCESS_PROFILES[access];

  return (
    <section aria-labelledby="onboarding-review-heading" className="space-y-5">
      <div>
        <h2
          id="onboarding-review-heading"
          className="font-display text-2xl font-semibold text-ink"
        >
          Review
        </h2>
        <p className="mt-1 text-sm text-ink-2">
          Confirm the main agent that will be created on the daemon.
        </p>
      </div>

      <dl className="grid gap-3 rounded-xl border border-line p-4 sm:grid-cols-[10rem_1fr]">
        <dt className="text-xs uppercase tracking-wide text-ink-3">Name</dt>
        <dd className="text-sm text-ink">{name.trim()}</dd>

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
          {system.trim() || 'No additional instructions'}
        </dd>
      </dl>

      {createError ? (
        <p
          role="alert"
          className="rounded-xl border border-red-400/30 bg-red-400/5 p-3 text-sm text-red-300"
        >
          {createError}
        </p>
      ) : null}

      <div className="flex items-center justify-between gap-3">
        <button
          type="button"
          className="rounded-lg border border-line px-4 py-2 text-sm text-ink"
          onClick={onBack}
          disabled={creating}
        >
          Back
        </button>
        <button
          type="button"
          className="rounded-lg bg-sky-500 px-4 py-2 text-sm font-medium text-white disabled:opacity-60"
          onClick={onSubmit}
          disabled={creating}
        >
          {creating ? 'Creating agent…' : 'Create agent'}
        </button>
      </div>
    </section>
  );
}
