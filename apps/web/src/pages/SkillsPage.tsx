import { useState } from 'react';
import { DaemonHttpError } from '@animaOS-SWARM/sdk';
import type { Skill, SkillDraftProposer, SkillFile } from '@animaOS-SWARM/sdk';

import { SkillDraftCard } from '../components/skills/SkillDraftCard';
import {
  SkillEditor,
  type SkillEditorValue,
} from '../components/skills/SkillEditor';
import { COMPANION_UNREACHABLE, formatWhen } from '../lib/approvals';
import { daemon } from '../lib/daemon-api';
import {
  EDIT_NEEDS_REVIEW,
  FILE_PROBLEM_FIX,
  REVIEW_WARNING,
  STATUS_LABELS,
  invisibleNote,
  moreFileDraftsNote,
  revealInvisible,
  skillExistsProblem,
  splitFileDrafts,
} from '../lib/skills';
import { useSkills } from '../hooks/useSkills';

export interface SkillsPageProps {
  /** `LiveState.skillsVersion`. */
  version: number;
  /** `LiveState.epoch`. */
  epoch: number;
  online: boolean;
  onOpenSession: (proposer: SkillDraftProposer) => void;
}

type Editing = { skill: Skill | null; initial?: SkillEditorValue };
type Review = { skill: Skill; file: SkillFile; note: string | null };

/** Shown when a save is refused (409) while a file draft waits in the folder
 *  it would write to. */
const FILE_UNREVIEWED_HINT =
  'It is listed under Waiting for review as a draft found in the skills folder.';

/** Spec §15.4: skills with switches and status, drafts with a diff and
 *  Approve / Edit / Reject, an editor with a preview, New, Delete, and
 *  Import. */
export function SkillsPage({
  version,
  epoch,
  online,
  onOpenSession,
}: SkillsPageProps) {
  const view = useSkills({ version, epoch, enabled: online });
  const [editing, setEditing] = useState<Editing | null>(null);
  const [review, setReview] = useState<Review | null>(null);
  const [confirming, setConfirming] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  // The folder of the last save, for the hint when it was refused.
  const [savedSlug, setSavedSlug] = useState<string | null>(null);

  // A new action does not keep showing the last one's notice.
  const begin = () => {
    setNotice(null);
    setSavedSlug(null);
  };
  const detailOf = async (skill: Skill) => {
    begin();
    try {
      return await daemon.skill(skill.slug);
    } catch (caught) {
      setNotice(
        caught instanceof DaemonHttpError
          ? caught.message
          : COMPANION_UNREACHABLE,
      );
      return null;
    }
  };
  const edit = async (skill: Skill) => {
    const detail = await detailOf(skill);
    if (!detail) return;
    // Saving would approve whatever is in the editor: a file that changed
    // since the owner approved it, or that has a problem, is reviewed first.
    if (
      detail.file &&
      (detail.file.problem !== null ||
        detail.file.hash !== (detail.skill?.approvedHash ?? skill.approvedHash))
    ) {
      setEditing(null);
      setReview({ skill, file: detail.file, note: EDIT_NEEDS_REVIEW });
      return;
    }
    setEditing({
      skill,
      initial: {
        slug: skill.slug,
        name: skill.name,
        description: skill.description,
        body: detail.file?.body ?? '',
      },
    });
  };
  const openReview = async (skill: Skill) => {
    const detail = await detailOf(skill);
    if (detail?.file) setReview({ skill, file: detail.file, note: null });
  };

  const reviewed = review && {
    name: revealInvisible(review.file.name ?? ''),
    description: revealInvisible(review.file.description ?? ''),
    body: revealInvisible(review.file.body ?? ''),
  };
  const reviewNote =
    reviewed &&
    invisibleNote(
      reviewed.name.count + reviewed.description.count + reviewed.body.count,
    );
  const shown = splitFileDrafts(view.pending);
  const moreFiles = moreFileDraftsNote(shown.hidden);
  const error = view.error ?? notice;
  // A save refused while a file draft waits in its folder: say where it is.
  const unreviewedFile =
    view.errorStatus === 409 &&
    savedSlug !== null &&
    view.pending.some((draft) => draft.id === `file:${savedSlug}`);

  if (view.unavailable) {
    return (
      <div className="skills-page">
        <p className="skills-empty" role="status">
          {view.unavailable}
        </p>
      </div>
    );
  }

  return (
    <div className="skills-page">
      {error && (
        <p className="skills-error" role="alert">
          {error}
          {unreviewedFile && ` ${FILE_UNREVIEWED_HINT}`}
        </p>
      )}
      <section className="skills-section" aria-labelledby="skills-waiting">
        <h2 id="skills-waiting">Waiting for review</h2>
        {view.pending.length === 0 ? (
          <p className="skills-empty">No skill drafts are waiting for you.</p>
        ) : (
          shown.shown.map((draft) => (
            <SkillDraftCard
              key={draft.id}
              draft={draft}
              onApprove={(target, approval) => {
                begin();
                return view.approveDraft(target, approval);
              }}
              onReject={(target) => {
                begin();
                return view.rejectDraft(target);
              }}
              onOpenSession={onOpenSession}
            />
          ))
        )}
        {moreFiles && <p className="skills-empty">{moreFiles}</p>}
      </section>
      <section className="skills-section" aria-labelledby="skills-list">
        <div className="skills-section-header">
          <h2 id="skills-list">Skills</h2>
          <button
            type="button"
            className="studio-tool-button"
            onClick={() => {
              begin();
              setEditing({ skill: null });
            }}
          >
            New skill
          </button>
          <label className="studio-tool-button skills-import">
            Import SKILL.md
            <input
              type="file"
              accept=".md,text/markdown"
              className="skills-import-input"
              onChange={(event) => {
                const file = event.target.files?.[0];
                event.target.value = '';
                if (file) {
                  begin();
                  void view.importFile(file);
                }
              }}
            />
          </label>
        </div>
        {editing && (
          <SkillEditor
            initial={editing.initial}
            slugLocked={editing.skill !== null}
            saveLabel="Save skill"
            checkProblem={
              editing.skill === null
                ? (value) =>
                    view.skills.some((skill) => skill.slug === value.slug)
                      ? skillExistsProblem(value.slug)
                      : null
                : undefined
            }
            onSave={async (value) => {
              begin();
              setSavedSlug(value.slug);
              const kept = await view.save(value.slug, {
                name: value.name,
                description: value.description,
                body: value.body,
              });
              if (kept) setEditing(null);
              return kept;
            }}
            onCancel={() => setEditing(null)}
          />
        )}
        {review && reviewed && (
          <section
            className="skill-review"
            aria-label={`Review changes to ${revealInvisible(review.skill.name).text}`}
          >
            <p className="skill-draft-warning" role="note">
              {REVIEW_WARNING}
            </p>
            {review.note && (
              <p className="skill-draft-warning" role="note">
                {review.note}
              </p>
            )}
            {reviewNote && (
              <p className="skill-draft-warning" role="note">
                {reviewNote}
              </p>
            )}
            {review.file.problem ? (
              <>
                <p className="skill-draft-warning" role="note">
                  {review.file.problem}
                </p>
                <p className="skill-draft-warning" role="note">
                  {FILE_PROBLEM_FIX}
                </p>
              </>
            ) : (
              <>
                {reviewed.name.text && <strong>{reviewed.name.text}</strong>}
                {reviewed.description.text && (
                  <p className="skill-draft-description">
                    {reviewed.description.text}
                  </p>
                )}
                <pre className="skill-body">{reviewed.body.text}</pre>
              </>
            )}
            <div className="skill-actions">
              <button
                type="button"
                className="studio-tool-button"
                disabled={!review.file.hash || review.file.problem !== null}
                onClick={async () => {
                  if (!review.file.hash) return;
                  begin();
                  // Refused (the file changed again, or is gone): read it
                  // again so the review shows what is there now.
                  if (await view.approveChanged(review.skill, review.file.hash))
                    setReview(null);
                  else await openReview(review.skill);
                }}
              >
                Approve this version
              </button>
              <button
                type="button"
                className="studio-tool-button"
                onClick={() => setReview(null)}
              >
                Close
              </button>
            </div>
          </section>
        )}
        {view.loaded && view.skills.length === 0 ? (
          <p className="skills-empty">
            No skills yet. Write one, import a SKILL.md, or approve a draft.
          </p>
        ) : (
          <ul className="skills-list">
            {view.skills.map((skill) => (
              <li
                key={skill.slug}
                aria-label={revealInvisible(skill.name).text}
                className="skills-row"
              >
                <div className="skills-row-text">
                  <strong>{revealInvisible(skill.name).text}</strong>
                  <code>/{skill.slug}</code>
                  <span
                    className={`skills-status skills-status-${skill.status}`}
                  >
                    {STATUS_LABELS[skill.status]}
                  </span>
                  <span className="skills-description">
                    {revealInvisible(skill.description).text}
                  </span>
                </div>
                <div className="skill-actions">
                  <label className="skills-switch">
                    <input
                      type="checkbox"
                      checked={skill.enabled}
                      aria-label={`${revealInvisible(skill.name).text} is ${skill.enabled ? 'on' : 'off'}`}
                      onChange={(event) => {
                        begin();
                        void view.setEnabled(skill, event.target.checked);
                      }}
                    />
                    {skill.enabled ? 'On' : 'Off'}
                  </label>
                  {skill.status === 'changed' && (
                    <button
                      type="button"
                      className="studio-tool-button"
                      onClick={() => void openReview(skill)}
                    >
                      Review changes
                    </button>
                  )}
                  <button
                    type="button"
                    className="studio-tool-button"
                    onClick={() => void edit(skill)}
                  >
                    Edit
                  </button>
                  {confirming === skill.slug ? (
                    <>
                      <span className="skills-confirm">
                        Its folder moves to the workspace trash.
                      </span>
                      <button
                        type="button"
                        className="studio-tool-button"
                        onClick={async () => {
                          begin();
                          if (await view.remove(skill)) setConfirming(null);
                        }}
                      >
                        Delete /{skill.slug}
                      </button>
                      <button
                        type="button"
                        className="studio-tool-button"
                        onClick={() => setConfirming(null)}
                      >
                        Keep it
                      </button>
                    </>
                  ) : (
                    <button
                      type="button"
                      className="studio-tool-button"
                      onClick={() => setConfirming(skill.slug)}
                    >
                      Delete
                    </button>
                  )}
                </div>
              </li>
            ))}
          </ul>
        )}
      </section>
      <section className="skills-section" aria-labelledby="skills-decided">
        <h2 id="skills-decided">Recently decided</h2>
        {view.decided.length === 0 ? (
          <p className="skills-empty">No drafts decided in the last 30 days.</p>
        ) : (
          <ul className="skills-decided">
            {view.decided.map((draft) => (
              <li key={draft.id}>
                <strong>{revealInvisible(draft.name).text}</strong>
                <code>/{draft.slug}</code>
                <span>
                  {draft.status === 'approved' ? 'Approved' : 'Rejected'}
                </span>
                {draft.decidedAtMs !== null && (
                  <time dateTime={new Date(draft.decidedAtMs).toISOString()}>
                    {formatWhen(draft.decidedAtMs)}
                  </time>
                )}
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}
