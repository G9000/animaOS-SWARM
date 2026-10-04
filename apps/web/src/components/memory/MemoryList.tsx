import { useState } from 'react';
import type {
  Memory,
  MemoryEvidenceTrace,
  MemoryFact,
  MemoryPatch,
} from '@animaOS-SWARM/sdk';
import { MAX_MEMORY_EDIT_CHARS } from '@animaOS-SWARM/sdk';

import { formatWhen } from '../../lib/approvals';
import {
  DELETE_MEMORY_PROMPT,
  MEMORY_TYPE_LABELS,
  describeFact,
  formatTags,
  importanceLabel,
  parseTagInput,
  stripInvisible,
  type MemoryEntry,
} from '../../lib/memory';
import { invisibleNote, revealInvisible } from '../../lib/skills';
import { ConfirmRow, charCount, formatCount } from './memory-ui';
import { RevealedText } from './RevealedText';

const TRAIL_FAILED = 'Couldn’t load the trail.';
const TRAIL_EMPTY = 'Nothing else cites this memory.';

export interface MemoryListProps {
  memories: readonly MemoryEntry[];
  /** The loaded facts, for "Where it was used". */
  facts: readonly MemoryFact[];
  onEdit: (memory: Memory, patch: MemoryPatch) => Promise<boolean>;
  onDelete: (memory: Memory) => Promise<boolean>;
  onTrace: (memory: Memory) => Promise<MemoryEvidenceTrace | null>;
  /** After an action the daemon took, so focus can move to the heading. */
  onDone: () => void;
}

export function MemoryList({
  memories,
  facts,
  onEdit,
  onDelete,
  onTrace,
  onDone,
}: MemoryListProps) {
  return (
    <ul className="memory-list">
      {memories.map((memory) => (
        <MemoryRow
          key={memory.id}
          memory={memory}
          facts={facts}
          onEdit={onEdit}
          onDelete={onDelete}
          onTrace={onTrace}
          onDone={onDone}
        />
      ))}
    </ul>
  );
}

type Mode = 'view' | 'edit' | 'delete';
type Trail =
  | { state: 'loading' }
  | { state: 'failed' }
  | { state: 'ready'; trace: MemoryEvidenceTrace };

function MemoryRow({
  memory,
  facts,
  onEdit,
  onDelete,
  onTrace,
  onDone,
}: { memory: MemoryEntry } & Omit<MemoryListProps, 'memories'>) {
  const [mode, setMode] = useState<Mode>('view');
  const [trail, setTrail] = useState<Trail | null>(null);
  const [busy, setBusy] = useState(false);

  const toggleTrail = async () => {
    if (trail) {
      setTrail(null);
      return;
    }
    setTrail({ state: 'loading' });
    const trace = await onTrace(memory);
    setTrail(trace ? { state: 'ready', trace } : { state: 'failed' });
  };

  return (
    <li className="memory-item">
      {mode === 'edit' ? (
        <MemoryEditor
          memory={memory}
          onSave={async (patch) => {
            const kept = await onEdit(memory, patch);
            if (kept) {
              setMode('view');
              onDone();
            }
            return kept;
          }}
          onCancel={() => setMode('view')}
        />
      ) : (
        <div className="memory-item-body">
          <div className="memory-meta">
            <span className="memory-label">
              {MEMORY_TYPE_LABELS[memory.type]}
            </span>
            <span>{importanceLabel(memory.importance)} importance</span>
            <time dateTime={new Date(memory.createdAt).toISOString()}>
              {formatWhen(memory.createdAt)}
            </time>
          </div>
          <p className="memory-content">
            <RevealedText text={memory.content} />
          </p>
          {memory.tags && memory.tags.length > 0 && (
            <ul className="memory-tags" aria-label="Tags">
              {memory.tags.map((tag) => (
                <li key={tag} className="memory-chip">
                  <RevealedText text={tag} />
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
      {mode === 'delete' && (
        <ConfirmRow
          prompt={DELETE_MEMORY_PROMPT}
          confirmLabel="Delete memory"
          onConfirm={async () => {
            if (busy) return;
            setBusy(true);
            const gone = await onDelete(memory);
            setBusy(false);
            if (gone) onDone();
            else setMode('view');
          }}
          onKeep={() => setMode('view')}
        />
      )}
      {mode === 'view' && (
        <div className="memory-actions">
          <button
            type="button"
            className="studio-tool-button"
            onClick={() => setMode('edit')}
          >
            Edit
          </button>
          <button
            type="button"
            className="studio-tool-button"
            onClick={() => setMode('delete')}
          >
            Delete
          </button>
          <button
            type="button"
            className="studio-tool-button"
            aria-expanded={trail !== null}
            onClick={() => void toggleTrail()}
          >
            Where it was used
          </button>
        </div>
      )}
      {trail && <TrailView trail={trail} memory={memory} facts={facts} />}
    </li>
  );
}

function TrailView({
  trail,
  memory,
  facts,
}: {
  trail: Trail;
  memory: Memory;
  facts: readonly MemoryFact[];
}) {
  if (trail.state === 'loading')
    return (
      <p className="memory-note" role="status">
        Loading…
      </p>
    );
  if (trail.state === 'failed')
    return <p className="memory-note">{TRAIL_FAILED}</p>;
  const relationships = trail.trace.relationships;
  const citing = facts.filter((fact) =>
    fact.evidenceMemoryIds.includes(memory.id),
  );
  if (relationships.length === 0 && citing.length === 0)
    return <p className="memory-note">{TRAIL_EMPTY}</p>;
  return (
    <div className="memory-trail">
      {relationships.length > 0 && (
        <section aria-label="Relationships">
          <h4>Relationships</h4>
          <ul>
            {relationships.map((relationship) => (
              <li key={relationship.id}>
                <RevealedText text={relationship.relationshipType} />
                {': '}
                <RevealedText text={relationship.sourceAgentName} />
                {' → '}
                <RevealedText text={relationship.targetAgentName} />
                {relationship.summary && (
                  <>
                    {' · '}
                    <RevealedText text={relationship.summary} />
                  </>
                )}
              </li>
            ))}
          </ul>
        </section>
      )}
      {citing.length > 0 && (
        <section aria-label="Facts">
          <h4>Facts</h4>
          <ul>
            {citing.map((fact) => {
              const described = describeFact(fact);
              return (
                <li key={fact.id}>
                  <RevealedText text={described.label} />
                  {': '}
                  <RevealedText text={described.value} />
                </li>
              );
            })}
          </ul>
        </section>
      )}
    </div>
  );
}

function sameTags(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((tag, index) => tag === b[index]);
}

function MemoryEditor({
  memory,
  onSave,
  onCancel,
}: {
  memory: Memory;
  onSave: (patch: MemoryPatch) => Promise<boolean>;
  onCancel: () => void;
}) {
  const [content, setContent] = useState(memory.content);
  const [importance, setImportance] = useState(memory.importance);
  const [tagText, setTagText] = useState(formatTags(memory.tags));
  const [saving, setSaving] = useState(false);

  const trimmed = content.trim();
  const length = charCount(trimmed);
  const tooLong = length > MAX_MEMORY_EDIT_CHARS;
  const tags = parseTagInput(tagText);

  const patch: MemoryPatch = {};
  if (trimmed !== memory.content) patch.content = trimmed;
  if (Math.abs(importance - memory.importance) > 1e-9)
    patch.importance = importance;
  if (!sameTags(tags, memory.tags ?? []))
    patch.tags = tags.length === 0 ? null : tags;
  const canSave =
    Object.keys(patch).length > 0 && !tooLong && trimmed !== '' && !saving;
  const hidden = revealInvisible(content).count;

  return (
    <form
      className="memory-form"
      aria-label="Edit memory"
      onSubmit={async (event) => {
        event.preventDefault();
        if (!canSave) return;
        setSaving(true);
        const kept = await onSave(patch);
        // A kept edit closes the form, which unmounts this component.
        if (!kept) setSaving(false);
      }}
    >
      <label className="memory-field">
        Memory text
        <textarea
          value={content}
          onChange={(event) => setContent(event.target.value)}
        />
      </label>
      {tooLong && (
        <p className="memory-error" role="alert">
          {formatCount(length, MAX_MEMORY_EDIT_CHARS)}
        </p>
      )}
      {hidden > 0 && (
        <div className="memory-edit-row">
          <small className="memory-hidden-note">{invisibleNote(hidden)}</small>
          <button
            type="button"
            className="studio-tool-button"
            onClick={() => setContent(stripInvisible(content))}
          >
            Remove invisible characters
          </button>
        </div>
      )}
      <div className="memory-field">
        <span>Importance</span>
        <span className="memory-edit-row">
          <input
            type="range"
            aria-label="Importance"
            min={0}
            max={1}
            step={0.05}
            value={importance}
            onChange={(event) => setImportance(Number(event.target.value))}
          />
          <output>{importance.toFixed(2)}</output>
        </span>
      </div>
      <label className="memory-field">
        Tags
        <input
          type="text"
          value={tagText}
          onChange={(event) => setTagText(event.target.value)}
        />
      </label>
      <div className="memory-actions">
        <button type="submit" className="studio-tool-button" disabled={!canSave}>
          Save
        </button>
        <button type="button" className="studio-tool-button" onClick={onCancel}>
          Cancel
        </button>
      </div>
    </form>
  );
}
