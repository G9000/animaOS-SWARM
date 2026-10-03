import {
  MAX_SKILL_BODY_BYTES,
  MAX_SKILL_DESCRIPTION_CHARS,
  MAX_SKILL_NAME_CHARS,
  SKILL_SLUG_PATTERN,
  type SkillDraft,
  type SkillDraftSource,
  type SkillStatus,
} from '@animaOS-SWARM/sdk';

export const STATUS_LABELS: Record<SkillStatus, string> = {
  active: 'Active',
  changed: 'Changed on disk — review it',
  missing: 'SKILL.md is missing',
  invalid: 'SKILL.md is not valid',
};

export const SOURCE_LABELS: Record<SkillDraftSource, string> = {
  agent: 'Proposed by your companion',
  import: 'Imported',
  file: 'Found in the skills folder (not written on this page)',
};

export const SKILL_SLUG_PROBLEM =
  'Use 1–64 lowercase letters, digits, or hyphens for the folder name, and not a reserved name (import, con, nul, …).';
export const SKILL_NAME_PROBLEM =
  'Give the skill a one-line name of at most 64 characters.';
export const SKILL_DESCRIPTION_PROBLEM =
  'Say when to use it in one line of at most 300 characters.';
export const SKILL_BODY_MISSING = 'Write the instructions.';
export const SKILL_BODY_TOO_LARGE = 'The instructions must be at most 32 KiB.';

/** Copy for the Skills page (Task 12). */
export const EDIT_NEEDS_REVIEW =
  'This skill’s file changed since you approved it. Review it first, then edit.';
export const REVIEW_WARNING =
  'Anything with write access to the workspace, including your companion, can change this file. Read it in full before approving.';

/** The daemon's reserved slugs (`RESERVED_SKILL_SLUGS`): `import` collides
 *  with `POST /api/skills/import`, the rest are Windows device names. */
export const RESERVED_SKILL_SLUGS: readonly string[] = [
  'import',
  'con',
  'prn',
  'aux',
  'nul',
  'com0',
  'com1',
  'com2',
  'com3',
  'com4',
  'com5',
  'com6',
  'com7',
  'com8',
  'com9',
  'lpt0',
  'lpt1',
  'lpt2',
  'lpt3',
  'lpt4',
  'lpt5',
  'lpt6',
  'lpt7',
  'lpt8',
  'lpt9',
];

export function isReservedSkillSlug(slug: string): boolean {
  return RESERVED_SKILL_SLUGS.includes(slug);
}

/** C0 and C1 control characters (Rust's `char::is_control`), which the
 *  daemon refuses in names and descriptions. */
export function hasControlCharacter(value: string): boolean {
  for (const character of value) {
    const code = character.codePointAt(0) ?? 0;
    if (code < 0x20 || (code >= 0x7f && code < 0xa0)) return true;
  }
  return false;
}

function oneLine(value: string, max: number): boolean {
  const trimmed = value.trim();
  return (
    trimmed.length > 0 &&
    [...trimmed].length <= max &&
    !hasControlCharacter(trimmed)
  );
}

/** The daemon's slug for a name (`skills::slugify`); '' when none. */
export function slugFromName(name: string): string {
  let slug = '';
  let afterHyphen = false;
  for (const character of name.toLowerCase()) {
    if (slug.length >= 64) break;
    if (/^[a-z0-9]$/.test(character)) {
      slug += character;
      afterHyphen = false;
    } else if (slug && !afterHyphen) {
      slug += '-';
      afterHyphen = true;
    }
  }
  slug = slug.replace(/-+$/, '');
  return SKILL_SLUG_PATTERN.test(slug) && !isReservedSkillSlug(slug)
    ? slug
    : '';
}

/** The first thing the daemon would refuse in this content, or null. */
export function skillInputProblem(input: {
  slug: string;
  name: string;
  description: string;
  body: string;
}): string | null {
  if (!SKILL_SLUG_PATTERN.test(input.slug) || isReservedSkillSlug(input.slug))
    return SKILL_SLUG_PROBLEM;
  if (!oneLine(input.name, MAX_SKILL_NAME_CHARS)) return SKILL_NAME_PROBLEM;
  if (!oneLine(input.description, MAX_SKILL_DESCRIPTION_CHARS))
    return SKILL_DESCRIPTION_PROBLEM;
  if (!input.body.trim()) return SKILL_BODY_MISSING;
  if (new TextEncoder().encode(input.body).length > MAX_SKILL_BODY_BYTES)
    return SKILL_BODY_TOO_LARGE;
  return null;
}

/** Shows the characters a reader cannot see (format characters, such as
 *  zero-width, tag, and direction controls, and the line and paragraph
 *  separators) as `⟨U+XXXX⟩` markers, and counts them. The daemon refuses
 *  the worst of these; this is what the owner sees of the rest. */
export function revealInvisible(text: string): {
  text: string;
  count: number;
} {
  let count = 0;
  const revealed = text.replace(/[\p{Cf}\u2028\u2029]/gu, (character) => {
    count += 1;
    const hex = (character.codePointAt(0) ?? 0)
      .toString(16)
      .toUpperCase()
      .padStart(4, '0');
    return `⟨U+${hex}⟩`;
  });
  return { text: revealed, count };
}

export function invisibleNote(count: number): string | null {
  return count === 0
    ? null
    : `This text contains ${count} invisible character${count === 1 ? '' : 's'}`;
}

/** File drafts shown on the page at most; the rest stay in the folder. */
export const MAX_FILE_DRAFTS_SHOWN = 20;

/** Every other draft, then the first `MAX_FILE_DRAFTS_SHOWN` file drafts
 *  in the order given; `hidden` counts the file drafts left out. */
export function splitFileDrafts(drafts: readonly SkillDraft[]): {
  shown: SkillDraft[];
  hidden: number;
} {
  const files = drafts.filter((draft) => draft.source === 'file');
  return {
    shown: [
      ...drafts.filter((draft) => draft.source !== 'file'),
      ...files.slice(0, MAX_FILE_DRAFTS_SHOWN),
    ],
    hidden: Math.max(0, files.length - MAX_FILE_DRAFTS_SHOWN),
  };
}

export function moreFileDraftsNote(hidden: number): string | null {
  return hidden === 0 ? null : `${hidden} more in the skills folder`;
}
