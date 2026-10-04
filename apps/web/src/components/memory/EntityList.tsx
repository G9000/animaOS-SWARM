import { useState } from 'react';
import type { AgentRelationship, MemoryEntity } from '@animaOS-SWARM/sdk';

import { DELETE_ENTITY_PROMPT } from '../../lib/memory';
import { ConfirmRow } from './memory-ui';
import { RevealedText } from './RevealedText';

const KIND_LABELS: Record<MemoryEntity['kind'], string> = {
  user: 'Person',
  agent: 'Companion',
  system: 'System',
  external: 'Thing',
};

/** Deleting the user entity is soft: the evaluator recreates it (ruling 2). */
export const USER_ENTITY_NOTE =
  'Your companion adds this again when it learns about you.';

export function entityKey(entity: Pick<MemoryEntity, 'kind' | 'id'>): string {
  return `${entity.kind}:${entity.id}`;
}

export interface EntityListProps {
  entities: readonly MemoryEntity[];
  relationships: readonly AgentRelationship[];
  onRemove: (entity: MemoryEntity) => Promise<boolean>;
  /** Called as a removal starts, so the page knows which entity owns the
   *  error it then shows. */
  onRemoving: (entity: MemoryEntity) => void;
  /** The daemon's message for the entity whose removal failed. */
  error: { key: string; message: string } | null;
  /** After an action the daemon took, so focus can move to the heading. */
  onDone: () => void;
}

export function EntityList({
  entities,
  relationships,
  onRemove,
  onRemoving,
  error,
  onDone,
}: EntityListProps) {
  return (
    <>
      <ul className="memory-list" aria-label="People and things">
        {entities.map((entity) => (
          <EntityRow
            key={entityKey(entity)}
            entity={entity}
            onRemove={onRemove}
            onRemoving={onRemoving}
            error={error?.key === entityKey(entity) ? error.message : null}
            onDone={onDone}
          />
        ))}
      </ul>
      {relationships.length > 0 && (
        <section className="memory-group" aria-label="Connections">
          <h4>Connections</h4>
          <ul className="memory-list">
            {relationships.map((relationship) => (
              <li key={relationship.id} className="memory-item">
                <span className="memory-content">
                  <RevealedText text={relationship.sourceAgentName} />
                  {' → '}
                  <RevealedText text={relationship.targetAgentName} />
                  {' · '}
                  <RevealedText text={relationship.relationshipType} />
                </span>
                {relationship.summary && (
                  <span className="memory-meta">
                    <RevealedText text={relationship.summary} />
                  </span>
                )}
              </li>
            ))}
          </ul>
        </section>
      )}
    </>
  );
}

function EntityRow({
  entity,
  onRemove,
  onRemoving,
  error,
  onDone,
}: {
  entity: MemoryEntity;
  onRemove: EntityListProps['onRemove'];
  onRemoving: EntityListProps['onRemoving'];
  error: string | null;
  onDone: () => void;
}) {
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  return (
    <li className="memory-item">
      <div className="memory-item-body">
        <div className="memory-meta">
          <strong className="memory-content">
            <RevealedText text={entity.name} />
          </strong>
          <span className="memory-label">{KIND_LABELS[entity.kind]}</span>
        </div>
        {entity.aliases.length > 0 && (
          <span className="memory-meta">
            Also called{' '}
            {entity.aliases.map((alias, index) => (
              <span key={`${index}:${alias}`}>
                {index > 0 && ', '}
                <RevealedText text={alias} />
              </span>
            ))}
          </span>
        )}
        {entity.summary && (
          <span className="memory-meta">
            <RevealedText text={entity.summary} />
          </span>
        )}
      </div>
      {error && (
        <p className="memory-error" role="alert">
          {error}
        </p>
      )}
      {confirming ? (
        <ConfirmRow
          prompt={DELETE_ENTITY_PROMPT}
          confirmLabel="Remove"
          onConfirm={async () => {
            if (busy) return;
            setBusy(true);
            onRemoving(entity);
            const removed = await onRemove(entity);
            setBusy(false);
            if (removed) onDone();
            else setConfirming(false);
          }}
          onKeep={() => setConfirming(false)}
        />
      ) : (
        <div className="memory-actions">
          <button
            type="button"
            className="studio-tool-button"
            onClick={() => setConfirming(true)}
          >
            Remove
          </button>
          {entity.kind === 'user' && (
            <small className="memory-note">{USER_ENTITY_NOTE}</small>
          )}
        </div>
      )}
    </li>
  );
}
