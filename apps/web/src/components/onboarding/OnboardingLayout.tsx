import { useEffect, useId, useRef, type ReactNode } from 'react';
import { OnboardingProgress } from './OnboardingProgress';
import './onboarding-layout.css';

export interface OnboardingLayoutProps {
  steps: readonly string[];
  currentStep: number;
  title: string;
  subtitle: string;
  summary: { workspace: string; template: string; team: string };
  children: ReactNode;
  footer?: ReactNode;
  resumeMode?: boolean;
}

export function OnboardingLayout({
  steps,
  currentStep,
  title,
  subtitle,
  summary,
  children,
  footer,
  resumeMode = false,
}: OnboardingLayoutProps) {
  const headingId = useId();
  const shellRef = useRef<HTMLDivElement>(null);
  const stepLabel = steps[currentStep] ?? steps[0] ?? 'Workspace';

  useEffect(() => {
    if (shellRef.current) shellRef.current.scrollTop = 0;
  }, [currentStep, resumeMode]);

  return (
    <div className="setup-shell" ref={shellRef}>
      <header className="setup-shell__topbar">
        <div className="setup-shell__brand">
          <span aria-hidden="true">✳</span> animaOS
        </div>
        <span className="setup-shell__context">
          {resumeMode ? 'Welcome back' : 'Workspace setup'}
        </span>
      </header>
      <div className="setup-shell__body">
        <aside className="setup-shell__sidebar" aria-label="Setup overview">
          <p className="setup-shell__eyebrow">
            {resumeMode ? 'Your workspace' : 'Make yourself at home'}
          </p>
          {!resumeMode && (
            <OnboardingProgress currentStep={currentStep} steps={steps} />
          )}
          <section className="setup-summary" aria-label="Your setup so far">
            <h2>Your setup</h2>
            <dl>
              <div>
                <dt>Workspace</dt>
                <dd>{summary.workspace || 'Not named yet'}</dd>
              </div>
              <div>
                <dt>Starting point</dt>
                <dd>{summary.template || 'Choose a starting point'}</dd>
              </div>
              <div>
                <dt>Team</dt>
                <dd>{summary.team || 'To be configured'}</dd>
              </div>
            </dl>
          </section>
        </aside>
        <main className="setup-shell__main" aria-labelledby={headingId}>
          {!resumeMode && (
            <div className="setup-shell__mobile-progress">
              <div className="setup-shell__mobile-step">
                <span>{stepLabel}</span>
                <span>
                  Step {currentStep + 1} of {steps.length}
                </span>
              </div>
              <div className="setup-shell__segments" aria-hidden="true">
                {steps.map((step, index) => (
                  <span key={step} data-reached={index <= currentStep} />
                ))}
              </div>
            </div>
          )}
          <p
            role="status"
            aria-live="polite"
            aria-atomic="true"
            className="sr-only"
          >
            {resumeMode
              ? 'Open existing workspace'
              : `Step ${currentStep + 1} of ${steps.length}: ${stepLabel}`}
          </p>
          <div className="setup-shell__panel">
            <header className="setup-shell__heading">
              <p className="setup-shell__eyebrow">
                {resumeMode
                  ? 'Continue where you left off'
                  : `Step ${String(currentStep + 1).padStart(2, '0')} / ${String(steps.length).padStart(2, '0')}`}
              </p>
              <h1 id={headingId} tabIndex={-1}>
                {title}
              </h1>
              <p className="setup-shell__subtitle">{subtitle}</p>
            </header>
            <div className="setup-shell__content">{children}</div>
            {footer && (
              <footer className="setup-shell__footer">{footer}</footer>
            )}
          </div>
        </main>
      </div>
    </div>
  );
}
