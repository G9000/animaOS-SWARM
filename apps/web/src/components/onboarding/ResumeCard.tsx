import type { WorkspaceInspectFound } from '../../lib/daemon-api';
import { labelCls } from '../ui-bits';

export interface ResumeCardProps {
  preview: WorkspaceInspectFound;
  rootPath: string;
  resuming: boolean;
  resumeError: string | null;
  onResume(): void;
  onSetupFresh(): void;
}

export function ResumeCard({
  preview,
  rootPath,
  resuming,
  resumeError,
  onResume,
  onSetupFresh,
}: ResumeCardProps) {
  return (
    <section
      aria-labelledby="onboarding-resume-heading"
      className="space-y-5"
    >
      <div>
        <h2
          id="onboarding-resume-heading"
          className="font-display text-2xl font-semibold tracking-tight text-ink"
        >
          Resume your workspace
        </h2>
        <p className="mt-1 max-w-xl text-sm leading-relaxed text-ink-2">
          We found an existing workspace here — pick up where you left off.
        </p>
      </div>

      <div className="rounded-2xl border border-line bg-white/[0.02] p-4">
        <p className="text-sm font-semibold text-ink">
          <span aria-hidden="true" className="mr-1.5">
            🏢
          </span>
          {preview.companyName}
        </p>
        {preview.mission ? (
          <p className="mt-1 text-sm text-ink-2">{preview.mission}</p>
        ) : null}
        <p
          className="mt-1 truncate font-mono text-xs text-ink-3"
          title={rootPath}
        >
          {rootPath}
        </p>
      </div>

      <div className="rounded-2xl border border-line bg-white/[0.02] p-4">
        <p className={labelCls}>Agents</p>
        <ul className="space-y-2">
          <li className="flex items-baseline justify-between gap-3">
            <span className="text-sm font-medium text-ink">
              {preview.orchestrator.name}{' '}
              <span className="rounded-full border border-line px-2 py-0.5 text-[10px] font-normal uppercase tracking-wide text-ink-3">
                Main agent
              </span>
            </span>
            <span className="text-xs text-ink-3">
              {preview.orchestrator.provider} / {preview.orchestrator.model}
            </span>
          </li>
          {preview.workers.map((worker) => (
            <li
              key={worker.name}
              className="flex items-baseline justify-between gap-3"
            >
              <span className="text-sm font-medium text-ink">
                {worker.name}
              </span>
              <span className="text-xs text-ink-3">
                {worker.provider} / {worker.model}
              </span>
            </li>
          ))}
        </ul>
      </div>

      {!preview.providerAvailable ? (
        <p className="text-xs leading-relaxed text-amber">
          The provider for these agents isn&apos;t configured on this machine —
          they&apos;ll resume offline until you add the key.
        </p>
      ) : null}

      <div className="flex flex-wrap items-center gap-3">
        <button
          type="button"
          onClick={onResume}
          disabled={resuming}
          className="rounded-xl bg-accent px-4 py-2 text-sm font-semibold text-abyss transition hover:bg-accent/90 disabled:cursor-not-allowed disabled:opacity-50"
        >
          {resuming ? 'Resuming…' : 'Resume workspace'}
        </button>
        <button
          type="button"
          onClick={onSetupFresh}
          className="rounded-xl border border-line bg-white/[0.02] px-4 py-2 text-sm font-medium text-ink-2 transition hover:border-line-strong hover:text-ink"
        >
          Set up fresh instead
        </button>
      </div>

      {resumeError ? (
        <p role="alert" className="text-sm text-danger">
          {resumeError}
        </p>
      ) : null}
    </section>
  );
}
