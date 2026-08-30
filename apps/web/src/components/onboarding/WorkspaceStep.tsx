import { useEffect, useState, type RefObject } from 'react';

import { labelCls } from '../ui-bits';

const MAX_VALUES = 5;

function parseValues(raw: string): string[] {
  return raw
    .split(',')
    .map((value) => value.trim())
    .filter(Boolean)
    .slice(0, MAX_VALUES);
}

export interface WorkspaceVerifyStatus {
  ok: boolean;
  willCreate?: boolean;
  message?: string;
}

export interface WorkspaceStepProps {
  companyName: string;
  mission: string;
  rootPath: string;
  values: string[];
  verifying: boolean;
  verifyStatus: WorkspaceVerifyStatus | null;
  onCompanyNameChange(value: string): void;
  onMissionChange(value: string): void;
  onRootPathChange(value: string): void;
  onValuesChange(values: string[]): void;
  onVerify(): void;
  companyInputRef: RefObject<HTMLInputElement | null>;
  validationErrorId?: string;
}

export function WorkspaceStep({
  companyName,
  mission,
  rootPath,
  values,
  verifying,
  verifyStatus,
  onCompanyNameChange,
  onMissionChange,
  onRootPathChange,
  onValuesChange,
  onVerify,
  companyInputRef,
  validationErrorId,
}: WorkspaceStepProps) {
  const [valuesDraft, setValuesDraft] = useState(() => values.join(', '));

  // Sync the draft when the values prop changes externally (e.g. reset),
  // without clobbering an in-progress draft that parses to the same values.
  useEffect(() => {
    setValuesDraft((draft) => {
      const parsed = parseValues(draft);
      const unchanged =
        parsed.length === values.length &&
        parsed.every((value, index) => value === values[index]);
      return unchanged ? draft : values.join(', ');
    });
  }, [values]);

  return (
    <section
      aria-labelledby="onboarding-workspace-heading"
      className="space-y-5"
    >
      <div>
        <h2
          id="onboarding-workspace-heading"
          className="font-display text-2xl font-semibold tracking-tight text-ink"
        >
          Workspace
        </h2>
        <p className="mt-1 max-w-xl text-sm leading-relaxed text-ink-2">
          Name your company and pick the folder your agents will work in.
        </p>
      </div>

      <div>
        <label htmlFor="onboarding-company-name" className={labelCls}>
          Company name
        </label>
        <input
          ref={companyInputRef}
          id="onboarding-company-name"
          className="field"
          value={companyName}
          onChange={(event) => onCompanyNameChange(event.target.value)}
          autoComplete="off"
          required
          aria-invalid={Boolean(validationErrorId)}
          aria-describedby={validationErrorId}
        />
      </div>

      <div>
        <label htmlFor="onboarding-mission" className={labelCls}>
          Mission (one sentence)
        </label>
        <input
          id="onboarding-mission"
          className="field"
          value={mission}
          onChange={(event) => onMissionChange(event.target.value)}
          autoComplete="off"
          placeholder="What is this company for?"
        />
      </div>

      <div>
        <label htmlFor="onboarding-root-path" className={labelCls}>
          Office location
        </label>
        <div className="flex gap-2">
          <input
            id="onboarding-root-path"
            className="field flex-1"
            value={rootPath}
            onChange={(event) => onRootPathChange(event.target.value)}
            autoComplete="off"
            spellCheck={false}
          />
          <button
            type="button"
            onClick={onVerify}
            disabled={verifying}
            className="rounded-xl border border-line bg-white/[0.02] px-4 py-2 text-sm font-medium text-ink-2 transition hover:border-line-strong hover:text-ink disabled:opacity-50"
          >
            {verifying ? 'Verifying…' : 'Verify'}
          </button>
        </div>
        {verifyStatus?.ok ? (
          <p role="status" className="mt-2 text-sm text-mint">
            ✓{' '}
            {verifyStatus.willCreate
              ? 'Folder will be created'
              : 'Folder exists'}{' '}
            — the daemon will use this as the workspace root.
          </p>
        ) : null}
        {verifyStatus && !verifyStatus.ok ? (
          <p role="alert" className="mt-2 text-sm text-danger">
            {verifyStatus.message ?? 'Could not verify that folder.'}
          </p>
        ) : null}
      </div>

      <div>
        <label htmlFor="onboarding-values" className={labelCls}>
          Values (optional, up to 5, comma-separated)
        </label>
        <input
          id="onboarding-values"
          className="field"
          value={valuesDraft}
          onChange={(event) => {
            setValuesDraft(event.target.value);
            onValuesChange(parseValues(event.target.value));
          }}
          onBlur={() => {
            // Normalize the draft once editing is done: trim, drop empties,
            // cap at MAX_VALUES. onValuesChange already emitted the same parse.
            setValuesDraft(parseValues(valuesDraft).join(', '));
          }}
          autoComplete="off"
          placeholder="cite sources, never invent numbers"
        />
      </div>
    </section>
  );
}
