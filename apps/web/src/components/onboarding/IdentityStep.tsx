import type { RefObject } from 'react';

import { labelCls } from '../ui-bits';

export interface IdentityStepProps {
  name: string;
  system: string;
  onNameChange(value: string): void;
  onSystemChange(value: string): void;
  nameInputRef: RefObject<HTMLInputElement | null>;
  validationErrorId?: string;
}

export function IdentityStep({
  name,
  system,
  onNameChange,
  onSystemChange,
  nameInputRef,
  validationErrorId,
}: IdentityStepProps) {
  return (
    <section
      aria-labelledby="onboarding-identity-heading"
      className="space-y-5"
    >
      <div>
        <h2
          id="onboarding-identity-heading"
          className="font-display text-2xl font-semibold text-ink"
        >
          Identity
        </h2>
        <p className="mt-1 text-sm text-ink-2">
          Give your main agent a name and optional standing instructions.
        </p>
      </div>

      <div>
        <label htmlFor="onboarding-agent-name" className={labelCls}>
          Agent name
        </label>
        <input
          ref={nameInputRef}
          id="onboarding-agent-name"
          className="field"
          value={name}
          onChange={(event) => onNameChange(event.target.value)}
          autoComplete="off"
          required
          aria-invalid={Boolean(validationErrorId)}
          aria-describedby={validationErrorId}
        />
      </div>

      <div>
        <label htmlFor="onboarding-system" className={labelCls}>
          Instructions (optional)
        </label>
        <textarea
          id="onboarding-system"
          className="field min-h-28 resize-y"
          value={system}
          onChange={(event) => onSystemChange(event.target.value)}
          placeholder="How should your agent work with you?"
        />
      </div>
    </section>
  );
}
