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
    <ol
      aria-label="Onboarding progress"
      className="grid grid-cols-4 gap-1.5 sm:gap-2"
    >
      {ONBOARDING_STEPS.map((label, index) => {
        const active = index === currentStep;
        const complete = index < currentStep;

        return (
          <li
            key={label}
            aria-current={active ? 'step' : undefined}
            className={`min-w-0 rounded-xl border px-2 py-2.5 text-center transition sm:px-3 ${
              active
                ? 'border-accent/45 bg-accent/[0.08]'
                : complete
                  ? 'border-mint/25 bg-mint/[0.04]'
                  : 'border-line bg-panel/35'
            }`}
          >
            <span
              className={`mx-auto flex h-5 w-5 items-center justify-center rounded-full font-mono text-[9px] ${
                active
                  ? 'bg-accent text-abyss'
                  : complete
                    ? 'bg-mint/15 text-mint'
                    : 'bg-white/[0.04] text-ink-3'
              }`}
            >
              {complete ? '✓' : index + 1}
            </span>
            <span
              className={`mt-1.5 block truncate text-[10px] sm:text-xs ${
                active ? 'font-medium text-ink' : 'text-ink-3'
              }`}
              title={label}
            >
              {label}
            </span>
          </li>
        );
      })}
    </ol>
  );
}
