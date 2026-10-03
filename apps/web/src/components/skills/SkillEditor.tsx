import { useState, type FormEvent } from 'react';

import {
  invisibleNote,
  revealInvisible,
  skillInputProblem,
  slugFromName,
} from '../../lib/skills';

export interface SkillEditorValue {
  slug: string;
  name: string;
  description: string;
  body: string;
}

/** Writes or edits a skill's content (spec §15.4). The preview shows the
 *  body as plain text: drafts may hold model-written text, so nothing
 *  here is ever rendered as Markdown or HTML. */
export function SkillEditor({
  initial,
  slugLocked = false,
  bodyOnly = false,
  saveLabel,
  checkProblem,
  onSave,
  onCancel,
}: {
  initial?: Partial<SkillEditorValue>;
  /** An existing skill keeps its folder. */
  slugLocked?: boolean;
  /** A draft's approval may change only its body (spec §8.4); `initial`
   *  still carries the draft's slug, name, and description, which the
   *  check before saving reads. */
  bodyOnly?: boolean;
  saveLabel: string;
  /** Anything else the caller would refuse in this content, or null. */
  checkProblem?: (value: SkillEditorValue) => string | null;
  /** True when the daemon took it; the editor then stays for the caller
   *  to close. */
  onSave: (value: SkillEditorValue) => Promise<boolean>;
  onCancel: () => void;
}) {
  const [name, setName] = useState(initial?.name ?? '');
  const [description, setDescription] = useState(initial?.description ?? '');
  const [body, setBody] = useState(initial?.body ?? '');
  const [slug, setSlug] = useState(initial?.slug ?? '');
  const [slugEdited, setSlugEdited] = useState(Boolean(initial?.slug));
  const [preview, setPreview] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const folder = slugEdited ? slug : slugFromName(name);
  const shownBody = revealInvisible(body);
  const bodyNote = invisibleNote(shownBody.count);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    const value = { slug: folder, name, description, body };
    const found = skillInputProblem(value) ?? checkProblem?.(value) ?? null;
    setProblem(found);
    if (found) return;
    setSaving(true);
    await onSave(value);
    setSaving(false);
  };

  return (
    <form
      className="skill-editor"
      aria-label="Skill editor"
      onSubmit={(event) => void submit(event)}
    >
      {!bodyOnly && (
        <>
          <label className="skill-field">
            <span>Name</span>
            <input
              value={name}
              onChange={(event) => setName(event.target.value)}
            />
          </label>
          <label className="skill-field">
            <span>Folder name</span>
            <input
              value={folder}
              disabled={slugLocked}
              onChange={(event) => {
                setSlug(event.target.value);
                setSlugEdited(true);
              }}
            />
          </label>
          <label className="skill-field">
            <span>When to use it</span>
            <input
              value={description}
              onChange={(event) => setDescription(event.target.value)}
            />
          </label>
        </>
      )}
      {preview ? (
        <pre className="skill-body" aria-label="Preview">
          {shownBody.text}
        </pre>
      ) : (
        <label className="skill-field">
          <span>Instructions</span>
          <textarea
            rows={12}
            value={body}
            onChange={(event) => setBody(event.target.value)}
          />
        </label>
      )}
      {bodyNote && (
        <p className="skill-draft-warning" role="note">
          {bodyNote}
        </p>
      )}
      {problem && (
        <p className="skills-error" role="alert">
          {problem}
        </p>
      )}
      <div className="skill-actions">
        <button
          type="button"
          className="studio-tool-button"
          onClick={() => setPreview((value) => !value)}
        >
          {preview ? 'Edit text' : 'Preview'}
        </button>
        <button type="submit" className="studio-tool-button" disabled={saving}>
          {saveLabel}
        </button>
        <button type="button" className="studio-tool-button" onClick={onCancel}>
          Cancel
        </button>
      </div>
    </form>
  );
}
