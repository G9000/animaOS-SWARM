import { useId, useState } from 'react';
import type { AgencyMember } from '../../lib/agency-templates';

export function AgencyTeam({
  workers,
  onChange,
  onRemove,
}: {
  workers: AgencyMember[];
  onChange(
    index: number,
    field: 'name' | 'bio' | 'system',
    value: string,
  ): void;
  onRemove(index: number): void;
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
        Edit roles and instructions, or remove anyone you don’t need. Every
        agent uses the model and access you choose.
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
                    onChange={(event) =>
                      onChange(index, 'name', event.target.value)
                    }
                    required
                  />
                </label>
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
              </div>
            )}
          </div>
        </div>
      ))}
      {!workers.length && (
        <p className="text-sm text-ink-3">
          Your workspace manager will be created on its own.
        </p>
      )}
    </section>
  );
}
