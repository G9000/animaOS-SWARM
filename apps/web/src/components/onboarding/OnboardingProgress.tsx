export const ONBOARDING_STEPS = [
  'Identity',
  'Intelligence',
  'Access',
  'Review',
] as const;

export interface OnboardingProgressProps {
  currentStep: number;
}

export function OnboardingProgress({ currentStep }: OnboardingProgressProps) {
  return (
    <ol aria-label="Onboarding progress" className="grid grid-cols-4 gap-2">
      {ONBOARDING_STEPS.map((label, index) => (
        <li
          key={label}
          aria-current={index === currentStep ? 'step' : undefined}
          className="min-w-0"
        >
          <span className="block font-mono text-[10px] uppercase tracking-wider text-ink-3">
            {index + 1}
          </span>
          <span className="block truncate text-xs text-ink-2">{label}</span>
        </li>
      ))}
    </ol>
  );
}
