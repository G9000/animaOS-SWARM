import type { RefObject } from 'react';

import { AGENT_PRESETS, type PresetId } from '../../lib/agent-presets';
import { labelCls } from '../ui-bits';

export interface AgentStepProps {
  name: string;
  presetId: PresetId;
  intent: string;
  bio: string;
  adjectives: string[];
  style: string;
  system: string;
  generating: boolean;
  /** false when the daemon reported PROFILE_GENERATION_UNAVAILABLE */
  generateAvailable: boolean;
  generateError: string | null;
  onNameChange(value: string): void;
  onPresetChange(value: PresetId): void;
  onIntentChange(value: string): void;
  onBioChange(value: string): void;
  onStyleChange(value: string): void;
  onSystemChange(value: string): void;
  onGenerate(): void;
  nameInputRef: RefObject<HTMLInputElement | null>;
  validationErrorId?: string;
}

export function AgentStep({
  name,
  presetId,
  intent,
  bio,
  adjectives,
  style,
  system,
  generating,
  generateAvailable,
  generateError,
  onNameChange,
  onPresetChange,
  onIntentChange,
  onBioChange,
  onStyleChange,
  onSystemChange,
  onGenerate,
  nameInputRef,
  validationErrorId,
}: AgentStepProps) {
  return (
    <section
      aria-labelledby="onboarding-agent-heading"
      className="space-y-5"
    >
      <div>
        <h2
          id="onboarding-agent-heading"
          className="font-display text-2xl font-semibold tracking-tight text-ink"
        >
          Agent
        </h2>
        <p className="mt-1 max-w-xl text-sm leading-relaxed text-ink-2">
          Pick a personality, describe what you want in plain words, and let
          the model write the proper profile.
        </p>
      </div>

      <div>
        <label htmlFor="onboarding-agent-name" className={labelCls}>
          Name
        </label>
        <input
          ref={nameInputRef}
          id="onboarding-agent-name"
          className="field"
          value={name}
          onChange={(event) => onNameChange(event.target.value)}
          autoComplete="off"
          aria-invalid={Boolean(validationErrorId)}
          aria-describedby={validationErrorId}
        />
      </div>

      <fieldset className="grid grid-cols-1 gap-2 sm:grid-cols-2">
        <legend className="sr-only">Personality preset</legend>
        {AGENT_PRESETS.map((preset) => {
          const selected = preset.id === presetId;
          const inputId = `onboarding-preset-${preset.id}`;
          return (
            <div
              key={preset.id}
              className={`rounded-2xl border p-4 transition ${
                selected
                  ? 'border-accent/60 bg-accent/[0.08]'
                  : 'border-line bg-white/[0.02] hover:border-line-strong'
              }`}
            >
              <input
                id={inputId}
                type="radio"
                name="onboarding-preset"
                value={preset.id}
                checked={selected}
                onChange={() => onPresetChange(preset.id)}
                className="mr-3 h-4 w-4 align-top"
              />
              <label htmlFor={inputId} className="inline cursor-pointer">
                <span className="text-sm font-semibold text-ink">
                  <span aria-hidden="true" className="mr-1.5">
                    {preset.icon}
                  </span>
                  {preset.label}
                </span>
                <span className="mt-1 block pl-7 text-xs leading-relaxed text-ink-2">
                  {preset.tagline}
                </span>
              </label>
            </div>
          );
        })}
      </fieldset>

      <div>
        <label htmlFor="onboarding-intent" className={labelCls}>
          What do you want {name.trim() || 'your agent'} to do for you?
        </label>
        <textarea
          id="onboarding-intent"
          className="field min-h-20 resize-y"
          value={intent}
          onChange={(event) => onIntentChange(event.target.value)}
          placeholder="Plain words are fine — rough is fine."
        />
        <div className="mt-2 flex items-center gap-3">
          <button
            type="button"
            onClick={onGenerate}
            disabled={generating || !intent.trim() || !generateAvailable}
            className="rounded-xl bg-accent px-4 py-2 text-sm font-semibold text-abyss transition hover:bg-accent/90 disabled:cursor-not-allowed disabled:opacity-50"
          >
            {generating ? (
              'Generating…'
            ) : system ? (
              <>
                <span aria-hidden="true">↻ </span>Regenerate profile
              </>
            ) : (
              <>
                <span aria-hidden="true">✨ </span>Generate profile
              </>
            )}
          </button>
          {!generateAvailable ? (
            <span className="text-xs text-ink-3">
              No generative provider configured — the preset template is filled
              in below; edit freely.
            </span>
          ) : null}
        </div>
        {generateError ? (
          <p role="alert" className="mt-2 text-sm text-danger">
            {generateError}
          </p>
        ) : null}
      </div>

      <div>
        <label htmlFor="onboarding-bio" className={labelCls}>
          Bio
        </label>
        <input
          id="onboarding-bio"
          className="field"
          value={bio}
          onChange={(event) => onBioChange(event.target.value)}
          autoComplete="off"
        />
      </div>

      {adjectives.length > 0 ? (
        <div>
          <p className={labelCls}>Traits</p>
          <div className="flex flex-wrap gap-1.5">
            {adjectives.map((adjective, index) => (
              <span
                key={`${adjective}-${index}`}
                className="rounded-full border border-line px-2.5 py-1 text-xs text-ink-2"
              >
                {adjective}
              </span>
            ))}
          </div>
        </div>
      ) : null}

      <div>
        <label htmlFor="onboarding-style" className={labelCls}>
          Style
        </label>
        <input
          id="onboarding-style"
          className="field"
          value={style}
          onChange={(event) => onStyleChange(event.target.value)}
          autoComplete="off"
        />
      </div>

      <div>
        <label htmlFor="onboarding-system" className={labelCls}>
          Instructions
        </label>
        <textarea
          id="onboarding-system"
          className="field min-h-32 resize-y"
          value={system}
          onChange={(event) => onSystemChange(event.target.value)}
          placeholder="Generated instructions appear here — edit anything."
        />
      </div>
    </section>
  );
}
