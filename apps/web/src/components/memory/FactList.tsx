import { useState } from 'react';
import type { MemoryFact } from '@animaOS-SWARM/sdk';
import { MAX_FACT_VALUE_CHARS } from '@animaOS-SWARM/sdk';

import { formatWhen } from '../../lib/approvals';
import {
  DELETE_FACT_PROMPT,
  FACT_EDIT_HINT,
  REPLACED_LABEL,
  describeFact,
} from '../../lib/memory';
import {
  ConfirmRow,
  InvisibleCharacters,
  charCount,
  formatCount,
} from './memory-ui';
import { RevealedText } from './RevealedText';

export interface FactListProps {
  facts: readonly MemoryFact[];
  onReplace: (fact: MemoryFact, value: string) => Promise<boolean>;
  onForget: (fact: MemoryFact) => Promise<boolean>;
  /** After an action the daemon took, so focus can move to the heading. */
  onDone: () => void;
}

export function FactList({
  facts,
  onReplace,
  onForget,
  onDone,
}: FactListProps) {
  return (
    <ul className="memory-list">
      {facts.map((fact) => (
        <FactRow
          key={fact.id}
          fact={fact}
          onReplace={onReplace}
          onForget={onForget}
          onDone={onDone}
        />
      ))}
    </ul>
  );
}

function FactRow({
  fact,
  onReplace,
  onForget,
  onDone,
}: { fact: MemoryFact } & Omit<FactListProps, 'facts'>) {
  const [mode, setMode] = useState<'view' | 'edit' | 'forget'>('view');
  const [value, setValue] = useState(fact.value ?? '');
  const [busy, setBusy] = useState(false);
  const described = describeFact(fact);
  const active = fact.status === 'active';
  const trimmed = value.trim();
  const length = charCount(trimmed);
  const tooLong = length > MAX_FACT_VALUE_CHARS;
  const canSave = trimmed !== '' && trimmed !== fact.value && !tooLong && !busy;

  return (
    <li className="memory-item">
      <div className="memory-item-body">
        <div className="memory-meta">
          <span className="memory-label">
            <RevealedText text={described.label} />
          </span>
          {!active && <span className="memory-replaced">{REPLACED_LABEL}</span>}
          <time dateTime={new Date(fact.observedAt).toISOString()}>
            {formatWhen(fact.observedAt)}
          </time>
        </div>
        {mode === 'edit' ? (
          <form
            className="memory-form"
            aria-label="Edit fact"
            onSubmit={async (event) => {
              event.preventDefault();
              if (!canSave) return;
              setBusy(true);
              const kept = await onReplace(fact, trimmed);
              setBusy(false);
              if (!kept) return;
              setMode('view');
              onDone();
            }}
          >
            <label className="memory-field">
              Fact
              <textarea
                value={value}
                onChange={(event) => setValue(event.target.value)}
              />
            </label>
            <small className="memory-hint">{FACT_EDIT_HINT}</small>
            {tooLong && (
              <p className="memory-error" role="alert">
                {formatCount(length, MAX_FACT_VALUE_CHARS)}
              </p>
            )}
            <InvisibleCharacters text={value} onStrip={setValue} />
            <div className="memory-actions">
              <button
                type="submit"
                className="studio-tool-button"
                disabled={!canSave}
              >
                Save
              </button>
              <button
                type="button"
                className="studio-tool-button"
                onClick={() => {
                  setValue(fact.value ?? '');
                  setMode('view');
                }}
              >
                Cancel
              </button>
            </div>
          </form>
        ) : (
          <p className="memory-content">
            <RevealedText text={described.value} />
          </p>
        )}
      </div>
      {mode === 'forget' && (
        <ConfirmRow
          prompt={DELETE_FACT_PROMPT}
          confirmLabel="Forget"
          onConfirm={async () => {
            if (busy) return;
            setBusy(true);
            const gone = await onForget(fact);
            setBusy(false);
            if (gone) onDone();
            else setMode('view');
          }}
          onKeep={() => setMode('view')}
        />
      )}
      {mode === 'view' && active && (
        <div className="memory-actions">
          {fact.value !== null && (
            <button
              type="button"
              className="studio-tool-button"
              onClick={() => setMode('edit')}
            >
              Edit
            </button>
          )}
          <button
            type="button"
            className="studio-tool-button"
            onClick={() => setMode('forget')}
          >
            Forget
          </button>
        </div>
      )}
    </li>
  );
}
