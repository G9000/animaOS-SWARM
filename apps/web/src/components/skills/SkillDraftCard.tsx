import { useEffect, useMemo, useState } from 'react';
import type {
  SkillDraft,
  SkillDraftApproval,
  SkillDraftProposer,
} from '@animaOS-SWARM/sdk';

import { daemon } from '../../lib/daemon-api';
import { lineDiff, type DiffLine } from '../../lib/skill-diff';
import {
  SOURCE_LABELS,
  invisibleNote,
  revealInvisible,
} from '../../lib/skills';
import { SkillEditor } from './SkillEditor';

const DIFF_PREFIX: Record<DiffLine['kind'], string> = {
  same: '  ',
  added: '+ ',
  removed: '- ',
};

/** A draft waiting for the owner (spec §15.4). Its text was written by the
 *  model, an import, or a file: it is shown only as text. */
export function SkillDraftCard({
  draft,
  onApprove,
  onReject,
  onOpenSession,
}: {
  draft: SkillDraft;
  onApprove: (
    draft: SkillDraft,
    approval: SkillDraftApproval,
  ) => Promise<boolean>;
  onReject: (draft: SkillDraft) => Promise<boolean>;
  onOpenSession?: (proposer: SkillDraftProposer) => void;
}) {
  const [current, setCurrent] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  const [busy, setBusy] = useState(false);

  // A draft that replaces a skill is compared with that skill's file now.
  useEffect(() => {
    setCurrent(null);
    if (draft.currentHash === null) return;
    const controller = new AbortController();
    daemon.skill(draft.slug, { signal: controller.signal }).then(
      (detail) => {
        if (!controller.signal.aborted) setCurrent(detail.file?.body ?? null);
      },
      () => undefined,
    );
    return () => controller.abort();
  }, [draft.slug, draft.currentHash]);

  const approve = async (approval: SkillDraftApproval) => {
    setBusy(true);
    const kept = await onApprove(
      draft,
      draft.source === 'file' && draft.fileHash
        ? { ...approval, hash: draft.fileHash }
        : approval,
    );
    setBusy(false);
    if (kept) setEditing(false);
    return kept;
  };
  const reject = async () => {
    setBusy(true);
    await onReject(draft);
    setBusy(false);
  };
  const diff = useMemo(
    () => (current === null ? null : lineDiff(current, draft.body)),
    [current, draft.body],
  );
  const blocked = busy || draft.problem !== null;
  // What the owner reads is shown with the invisible characters as markers;
  // the diff is computed on the raw text and each line is revealed.
  const name = revealInvisible(draft.name);
  const description = revealInvisible(draft.description);
  const body = revealInvisible(draft.body);
  const hidden =
    name.count +
    description.count +
    body.count +
    (diff ? revealInvisible(current ?? '').count : 0);
  const note = invisibleNote(hidden);

  return (
    <section className="skill-draft" aria-label={`Skill draft: ${name.text}`}>
      <header className="skill-draft-header">
        <strong>{name.text}</strong>
        <code>/{draft.slug}</code>
        <span className="skill-draft-source">
          {SOURCE_LABELS[draft.source]}
        </span>
        {draft.proposedBy && onOpenSession && (
          <button
            type="button"
            className="studio-tool-button"
            onClick={() => draft.proposedBy && onOpenSession(draft.proposedBy)}
          >
            Open the chat
          </button>
        )}
      </header>
      {description.text && (
        <p className="skill-draft-description">{description.text}</p>
      )}
      {note && (
        <p className="skill-draft-warning" role="note">
          {note}
        </p>
      )}
      {draft.stale && (
        <p className="skill-draft-warning" role="note">
          This skill changed since the draft was made; the comparison is with
          its current version.
        </p>
      )}
      {draft.problem && (
        <p className="skill-draft-warning" role="note">
          {draft.problem}
        </p>
      )}
      {editing ? (
        <SkillEditor
          bodyOnly
          initial={{
            slug: draft.slug,
            name: draft.name,
            description: draft.description,
            body: draft.body,
          }}
          saveLabel="Approve edited version"
          onSave={(value) => approve({ body: value.body })}
          onCancel={() => setEditing(false)}
        />
      ) : (
        <>
          {diff ? (
            <pre
              className="skill-diff"
              aria-label="Changes from the current version"
            >
              {diff.map((line, index) => (
                <span key={index} className={`skill-diff-${line.kind}`}>
                  {DIFF_PREFIX[line.kind]}
                  {revealInvisible(line.text).text}
                  {'\n'}
                </span>
              ))}
            </pre>
          ) : (
            <pre className="skill-body" aria-label="Instructions">
              {body.text}
            </pre>
          )}
          <div className="skill-actions">
            <button
              type="button"
              className="studio-tool-button"
              disabled={blocked}
              onClick={() => void approve({})}
            >
              Approve
            </button>
            <button
              type="button"
              className="studio-tool-button"
              disabled={blocked}
              onClick={() => setEditing(true)}
            >
              Edit
            </button>
            <button
              type="button"
              className="studio-tool-button"
              disabled={busy}
              onClick={() => void reject()}
            >
              Reject
            </button>
          </div>
        </>
      )}
    </section>
  );
}
