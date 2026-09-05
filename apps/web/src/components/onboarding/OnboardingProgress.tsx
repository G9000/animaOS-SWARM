export const ONBOARDING_STEPS = [
  'Workspace',
  'Model',
  'Team',
  'Manager',
  'Launch',
] as const;

export interface OnboardingProgressProps {
  currentStep: number;
  steps?: readonly string[];
}

export function OnboardingProgress({
  currentStep,
  steps = ONBOARDING_STEPS,
}: OnboardingProgressProps) {
  return (
    <ol aria-label="Onboarding progress" className="setup-progress">
      {steps.map((label, index) => {
        const active = index === currentStep;
        const complete = index < currentStep;
        return (
          <li
            key={label}
            aria-current={active ? 'step' : undefined}
            className="setup-progress__step"
            data-state={active ? 'current' : complete ? 'complete' : 'upcoming'}
          >
            <span className="setup-progress__number" aria-hidden="true">
              {complete ? '✓' : String(index + 1).padStart(2, '0')}
            </span>
            <span className="setup-progress__label">{label}</span>
            {complete && <span className="sr-only">, completed</span>}
            {active && (
              <span className="setup-progress__dot" aria-hidden="true" />
            )}
          </li>
        );
      })}
    </ol>
  );
}
