import { useId, useState } from 'react';
import type { AgencyMember } from '../../lib/agency-templates';
import {
  ACCESS_PROFILES,
  toolNamesForProfile,
  type AccessProfile,
} from '../../lib/agent-access';
import type { DaemonProvider } from '../../lib/daemon-api';

export function AgencyTeam({
  workers,
  onChange,
  onNameCommit,
  onRepairName,
  onRemove,
  onAdd,
  onSettingsChange,
  providers = [],
  defaultProvider = '',
  defaultModel = '',
  defaultAccess = 'collaborate',
}: {
  workers: AgencyMember[];
  onChange(
    index: number,
    field: 'name' | 'bio' | 'system',
    value: string,
  ): void;
  onRemove(index: number): void;
  onNameCommit?(): void;
  onRepairName?(index: number, previousName: string): string | null;
  onAdd?(): void;
  onSettingsChange?(index: number, settings: Partial<AgencyMember>): void;
  providers?: DaemonProvider[];
  defaultProvider?: string;
  defaultModel?: string;
  defaultAccess?: AccessProfile;
}) {
  const [editingIndex, setEditingIndex] = useState<number | null>(null);
  const editorId = useId();

  function removeMember(index: number) {
    setEditingIndex((current) => {
      if (current === null || current === index) return null;
      return current > index ? current - 1 : current;
    });
    onRemove(index);
  }

  return (
    <section
      aria-labelledby="agency-specialists"
      className="mt-6 space-y-3 border-t border-line pt-5"
    >
      <h3 id="agency-specialists" className="text-base font-semibold text-ink">
        Your specialists <span className="text-ink-3">({workers.length})</span>
      </h3>
      <p className="text-xs leading-relaxed text-ink-2">
        Each specialist should own a distinct outcome. Edit responsibilities,
        model, and access, or combine roles for a smaller team.
      </p>
      <p className="text-xs text-ink-3">
        Every agent starts with your setup model. Explicit overrides are saved
        for each agent in anima.yaml and restored when you reopen the workspace.
      </p>
      {workers.map((worker, index) => (
        <div
          key={index}
          className="min-w-0 space-y-3 rounded-xl border border-line bg-white/[0.02] p-4"
        >
          <div className="flex min-w-0 flex-wrap items-start justify-between gap-3">
            <div className="min-w-0 flex-1 basis-40">
              <p className="break-words text-sm font-semibold text-ink">
                {worker.name || `Specialist ${index + 1}`}
              </p>
              <p className="mt-1 line-clamp-2 break-words text-xs leading-relaxed text-ink-2">
                {worker.bio || 'Add a role for this specialist.'}
              </p>
            </div>
            <div className="flex shrink-0 gap-2">
              <button
                type="button"
                aria-label={`Edit ${worker.name || `specialist ${index + 1}`}`}
                aria-expanded={editingIndex === index}
                aria-controls={`${editorId}-${index}`}
                onClick={() =>
                  setEditingIndex(editingIndex === index ? null : index)
                }
                className="rounded-lg border border-line px-3 py-2 text-xs text-ink-2 hover:text-ink focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ink-2"
              >
                Edit
              </button>
              <button
                type="button"
                aria-label={`Remove ${worker.name || `specialist ${index + 1}`}`}
                onClick={() => removeMember(index)}
                className="rounded-lg border border-line px-3 py-2 text-xs text-ink-2 hover:text-danger focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ink-2"
              >
                Remove
              </button>
            </div>
          </div>
          <div id={`${editorId}-${index}`} hidden={editingIndex !== index}>
            {editingIndex === index && (
              <div className="min-w-0 space-y-3 border-t border-line pt-3">
                <label className="block min-w-0 text-xs text-ink-2">
                  Specialist {index + 1} name
                  <input
                    className="field mt-1"
                    value={worker.name}
                    onBlur={onNameCommit}
                    onChange={(event) =>
                      onChange(index, 'name', event.target.value)
                    }
                    required
                  />
                </label>
                {onRepairName && (
                  <NameReferenceRepair
                    key={index}
                    index={index}
                    name={worker.name}
                    onApply={(previous) => onRepairName(index, previous)}
                  />
                )}
                <label className="block text-xs text-ink-2">
                  Specialist {index + 1} role
                  <textarea
                    className="field mt-1"
                    rows={2}
                    value={worker.bio}
                    onChange={(event) =>
                      onChange(index, 'bio', event.target.value)
                    }
                    required
                  />
                </label>
                <label className="block text-xs text-ink-2">
                  Specialist {index + 1} instructions
                  <textarea
                    className="field mt-1"
                    rows={4}
                    value={worker.system}
                    onChange={(event) =>
                      onChange(index, 'system', event.target.value)
                    }
                    required
                  />
                </label>
                {onSettingsChange && (
                  <fieldset className="space-y-3">
                    <legend className="text-sm font-semibold text-ink">
                      Model and capabilities
                    </legend>
                    <label className="block text-xs text-ink-2">
                      Specialist {index + 1} provider
                      <select
                        className="field mt-1"
                        value={worker.provider ?? ''}
                        onChange={(event) =>
                          onSettingsChange(index, {
                            provider: event.target.value || undefined,
                            model: undefined,
                          })
                        }
                      >
                        <option value="">
                          Use team provider ({defaultProvider})
                        </option>
                        {worker.provider &&
                          !providers.some(
                            (provider) =>
                              provider.configured &&
                              provider.id === worker.provider,
                          ) && (
                            <option value={worker.provider}>
                              {worker.provider} (not connected)
                            </option>
                          )}
                        {providers
                          .filter((provider) => provider.configured)
                          .map((provider) => (
                            <option key={provider.id} value={provider.id}>
                              {provider.label}
                            </option>
                          ))}
                      </select>
                    </label>
                    <label className="block text-xs text-ink-2">
                      Specialist {index + 1} model
                      <input
                        className="field mt-1"
                        value={worker.model ?? ''}
                        placeholder={
                          worker.provider && worker.provider !== defaultProvider
                            ? 'Enter a model for this provider'
                            : `Use team model (${defaultModel})`
                        }
                        onChange={(event) =>
                          onSettingsChange(index, {
                            model: event.target.value || undefined,
                            ...(event.target.value.trim()
                              ? { provider: worker.provider || defaultProvider }
                              : {}),
                          })
                        }
                      />
                    </label>
                    <label className="block text-xs text-ink-2">
                      Specialist {index + 1} access
                      <select
                        className="field mt-1"
                        value={worker.access ?? ''}
                        onChange={(event) =>
                          onSettingsChange(index, {
                            access: (event.target.value || undefined) as
                              | AccessProfile
                              | undefined,
                            tools: undefined,
                          })
                        }
                      >
                        <option value="">Use team access</option>
                        {Object.entries(ACCESS_PROFILES).map(
                          ([id, profile]) => (
                            <option key={id} value={id}>
                              {profile.label}
                            </option>
                          ),
                        )}
                      </select>
                    </label>
                    <p className="text-xs text-ink-3">
                      {ACCESS_PROFILES[worker.access ?? defaultAccess].risk}
                    </p>
                    {!!worker.suggestedTools?.length && (
                      <p className="break-words text-xs text-ink-3">
                        Suggested tools: {worker.suggestedTools.join(', ')}.
                        Suggestions do not grant access. Service connections can
                        be set up after launch.
                      </p>
                    )}
                    <details>
                      <summary className="cursor-pointer text-xs text-ink-2">
                        Choose individual tools
                      </summary>
                      <div className="mt-2 grid gap-2 sm:grid-cols-2">
                        {toolNamesForProfile(
                          worker.access ?? defaultAccess,
                        ).map((tool) => {
                          const selected =
                            worker.tools ??
                            toolNamesForProfile(worker.access ?? defaultAccess);
                          return (
                            <label
                              key={tool}
                              className="flex items-center gap-2 text-xs text-ink-2"
                            >
                              <input
                                type="checkbox"
                                checked={selected.includes(tool)}
                                onChange={(event) =>
                                  onSettingsChange(index, {
                                    tools: event.target.checked
                                      ? [...selected, tool]
                                      : selected.filter(
                                          (name) => name !== tool,
                                        ),
                                  })
                                }
                              />
                              {tool}
                            </label>
                          );
                        })}
                      </div>
                    </details>
                  </fieldset>
                )}
              </div>
            )}
          </div>
        </div>
      ))}
      {onAdd && (
        <button
          type="button"
          onClick={onAdd}
          disabled={workers.length >= 9}
          className="rounded-xl border border-line px-4 py-2 text-sm text-ink disabled:opacity-50"
        >
          Add specialist
        </button>
      )}
      {!workers.length && (
        <p className="text-sm text-ink-3">
          Add at least one specialist, or choose Manager only.
        </p>
      )}
    </section>
  );
}

function NameReferenceRepair({ index, name, onApply }: {
  index: number;
  name: string;
  onApply(previous: string): string | null;
}) {
  const [previous, setPrevious] = useState('');
  const [message, setMessage] = useState<string | null>(null);
  return (
    <details className="text-xs text-ink-2">
      <summary className="cursor-pointer">Fix an old name in the text</summary>
      <p className="mt-2">If this draft still mentions an old name, enter it to update references to {name || 'this agent'} throughout the team.</p>
      <label className="mt-2 block">
        Specialist {index + 1} previous name
        <input className="field mt-1" value={previous} onChange={(event) => {
          setPrevious(event.target.value); setMessage(null);
        }} placeholder="Old name, e.g. Luis" />
      </label>
      <button type="button" className="mt-2 rounded-lg border border-line px-3 py-2" onClick={() => {
        const error = onApply(previous);
        setMessage(error ?? `Updated name references to ${name}.`);
        if (!error) setPrevious('');
      }}>Update name references</button>
      {message && <p role="status" className="mt-2">{message}</p>}
    </details>
  );
}
