import type { RefObject } from 'react';

import { labelCls } from '../ui-bits';

export interface WorkspaceManagerStepProps {
  name: string;
  initiative: 'guided' | 'balanced' | 'proactive';
  communication: 'concise' | 'detailed';
  priorities: string;
  instructions: string;
  onNameChange(value: string): void;
  onInitiativeChange(value: WorkspaceManagerStepProps['initiative']): void;
  onCommunicationChange(
    value: WorkspaceManagerStepProps['communication'],
  ): void;
  onPrioritiesChange(value: string): void;
  nameInputRef: RefObject<HTMLInputElement | null>;
  validationErrorId?: string;
}

const initiativeOptions = [
  {
    value: 'guided',
    label: 'Guided',
    description: 'Check with you before choosing the next step.',
  },
  {
    value: 'balanced',
    label: 'Balanced',
    description: 'Move agreed work forward and check in at key decisions.',
  },
  {
    value: 'proactive',
    label: 'Proactive',
    description:
      'Anticipate next steps and surface blockers within agreed work.',
  },
] as const;

const communicationOptions = [
  {
    value: 'concise',
    label: 'Concise',
    description: 'Key decisions, progress, and next steps.',
  },
  {
    value: 'detailed',
    label: 'Detailed',
    description: 'More context, reasoning, and explanation.',
  },
] as const;

const choiceClass = (selected: boolean) =>
  `flex cursor-pointer items-start gap-3 rounded-2xl border p-3 transition sm:p-4 ${
    selected
      ? 'border-accent/60 bg-accent/[0.08]'
      : 'border-line bg-white/[0.02] hover:border-line-strong'
  }`;

export function WorkspaceManagerStep({
  name,
  initiative,
  communication,
  priorities,
  instructions,
  onNameChange,
  onInitiativeChange,
  onCommunicationChange,
  onPrioritiesChange,
  nameInputRef,
  validationErrorId,
}: WorkspaceManagerStepProps) {
  return (
    <section
      aria-labelledby="onboarding-manager-heading"
      className="min-w-0 space-y-8"
    >
      <div>
        <h2
          id="onboarding-manager-heading"
          className="font-display text-2xl font-semibold tracking-tight text-ink"
        >
          Workspace Manager
        </h2>
        <p className="mt-1 max-w-xl text-sm leading-relaxed text-ink-2">
          Keeps your workspace organized, tracks priorities and context, and
          coordinates specialist work. Its role and personality are predefined;
          choose how it works with you.
        </p>
        <div
          className="mt-3 flex flex-wrap gap-1.5"
          aria-label="Manager personality"
        >
          {['Calm', 'Organized', 'Transparent'].map((trait) => (
            <span
              key={trait}
              className="rounded-full border border-line px-2.5 py-1 text-xs text-ink-2"
            >
              {trait}
            </span>
          ))}
        </div>
      </div>

      <div>
        <label htmlFor="onboarding-manager-name" className={labelCls}>
          Manager name
        </label>
        <input
          ref={nameInputRef}
          id="onboarding-manager-name"
          className="field"
          value={name}
          onChange={(event) => onNameChange(event.target.value)}
          autoComplete="off"
          aria-invalid={Boolean(validationErrorId)}
          aria-describedby={validationErrorId}
        />
      </div>

      <fieldset
        aria-describedby="onboarding-manager-initiative-help"
        className="min-w-0"
      >
        <legend className={labelCls}>Initiative</legend>
        <p
          id="onboarding-manager-initiative-help"
          className="mb-3 text-xs leading-relaxed text-ink-2"
        >
          How your manager approaches active work with you. Tool permissions are
          set in Access; this does not start background work.
        </p>
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
          {initiativeOptions.map((option) => (
            <label
              key={option.value}
              className={choiceClass(initiative === option.value)}
            >
              <input
                type="radio"
                name="onboarding-manager-initiative"
                value={option.value}
                checked={initiative === option.value}
                onChange={() => onInitiativeChange(option.value)}
                aria-label={option.label}
                aria-describedby={`manager-initiative-${option.value}-description`}
                className="mt-0.5 h-4 w-4 shrink-0 accent-accent"
              />
              <span className="min-w-0">
                <span className="block text-sm font-semibold text-ink">
                  {option.label}
                </span>
                <span
                  id={`manager-initiative-${option.value}-description`}
                  className="mt-1 block text-xs leading-relaxed text-ink-2"
                >
                  {option.description}
                </span>
              </span>
            </label>
          ))}
        </div>
      </fieldset>

      <fieldset className="min-w-0">
        <legend className={labelCls}>Communication</legend>
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          {communicationOptions.map((option) => (
            <label
              key={option.value}
              className={choiceClass(communication === option.value)}
            >
              <input
                type="radio"
                name="onboarding-manager-communication"
                value={option.value}
                checked={communication === option.value}
                onChange={() => onCommunicationChange(option.value)}
                aria-label={option.label}
                aria-describedby={`manager-communication-${option.value}-description`}
                className="mt-0.5 h-4 w-4 shrink-0 accent-accent"
              />
              <span className="min-w-0">
                <span className="block text-sm font-semibold text-ink">
                  {option.label}
                </span>
                <span
                  id={`manager-communication-${option.value}-description`}
                  className="mt-1 block text-xs leading-relaxed text-ink-2"
                >
                  {option.description}
                </span>
              </span>
            </label>
          ))}
        </div>
      </fieldset>

      <div>
        <label htmlFor="onboarding-manager-priorities" className={labelCls}>
          Workspace preferences
        </label>
        <p
          id="onboarding-manager-preferences-help"
          className="mb-2 text-xs text-ink-2"
        >
          Optional. Add priorities or ways of working your manager should keep
          in mind.
        </p>
        <textarea
          id="onboarding-manager-priorities"
          className="field min-h-20 resize-y"
          value={priorities}
          onChange={(event) => onPrioritiesChange(event.target.value)}
          aria-describedby="onboarding-manager-preferences-help"
          placeholder="For example, focus on launch readiness and flag decisions that need my input."
        />
      </div>

      <details className="min-w-0 rounded-2xl border border-line bg-white/[0.02]">
        <summary className="cursor-pointer rounded-2xl px-4 py-3 text-sm font-medium text-ink focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent">
          View manager instructions
        </summary>
        <p className="whitespace-pre-wrap break-words border-t border-line px-4 py-3 text-xs leading-relaxed text-ink-2">
          {instructions}
        </p>
      </details>
    </section>
  );
}
