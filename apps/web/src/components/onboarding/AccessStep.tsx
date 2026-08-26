import { ACCESS_PROFILES, type AccessProfile } from '../../lib/agent-access';

export interface AccessStepProps {
  access: AccessProfile;
  onAccessChange(access: AccessProfile): void;
}

const ACCESS_ORDER: readonly AccessProfile[] = [
  'observe',
  'collaborate',
  'operate',
];

export function AccessStep({ access, onAccessChange }: AccessStepProps) {
  return (
    <section aria-labelledby="onboarding-access-heading" className="space-y-5">
      <div>
        <h2
          id="onboarding-access-heading"
          className="font-display text-2xl font-semibold text-ink"
        >
          Access
        </h2>
        <p className="mt-1 text-sm text-ink-2">
          Set what your agent can inspect, change, and run in the workspace.
        </p>
      </div>

      <fieldset className="space-y-2">
        <legend className="sr-only">Access profile</legend>
        {ACCESS_ORDER.map((profileName) => {
          const profile = ACCESS_PROFILES[profileName];
          const inputId = `onboarding-access-${profileName}`;

          return (
            <div
              key={profileName}
              className="rounded-xl border border-line p-4"
            >
              <input
                id={inputId}
                type="radio"
                name="onboarding-access"
                value={profileName}
                checked={access === profileName}
                onChange={() => onAccessChange(profileName)}
                className="mr-3 align-top"
              />
              <label htmlFor={inputId} className="inline cursor-pointer">
                <span className="font-medium text-ink">{profile.label}</span>
                <span className="mt-1 block pl-7 text-sm text-ink-2">
                  {profile.summary}
                </span>
                <span className="mt-1 block pl-7 text-xs text-ink-3">
                  {profile.risk}
                </span>
              </label>
            </div>
          );
        })}
      </fieldset>
    </section>
  );
}
