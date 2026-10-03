import { describe, expect, it } from 'vitest';

import {
  MAX_FILE_DRAFTS_SHOWN,
  RESERVED_SKILL_SLUGS,
  SKILL_BODY_MISSING,
  SKILL_BODY_TOO_LARGE,
  SKILL_DESCRIPTION_PROBLEM,
  SKILL_NAME_PROBLEM,
  SKILL_SLUG_PROBLEM,
  SOURCE_LABELS,
  hasControlCharacter,
  invisibleNote,
  isReservedSkillSlug,
  moreFileDraftsNote,
  revealInvisible,
  skillInputProblem,
  slugFromName,
  splitFileDrafts,
} from './skills';
import { skillDraftFixture } from '../test/skills';

const good = {
  slug: 'weekly-review',
  name: 'Weekly Review',
  description: 'Review the week',
  body: 'List what shipped.',
};

describe('slugFromName', () => {
  it('derives the daemon’s slug', () => {
    expect(slugFromName('Weekly Review!')).toBe('weekly-review');
    expect(slugFromName('--Plan  B--')).toBe('plan-b');
    expect(slugFromName('!!!')).toBe('');
    expect(slugFromName('a'.repeat(70))).toBe('a'.repeat(64));
  });

  it('gives no slug for a reserved name', () => {
    expect(slugFromName('Con')).toBe('');
    expect(slugFromName('COM1')).toBe('');
    expect(slugFromName('Import')).toBe('');
    expect(slugFromName('COM10')).toBe('com10');
  });
});

describe('reserved slugs', () => {
  it('mirror the daemon’s 25 names', () => {
    expect(RESERVED_SKILL_SLUGS).toHaveLength(25);
    for (const slug of ['import', 'con', 'prn', 'aux', 'nul', 'com0', 'lpt9'])
      expect(isReservedSkillSlug(slug)).toBe(true);
    expect(isReservedSkillSlug('console')).toBe(false);
  });
});

describe('skillInputProblem', () => {
  it('accepts valid content and names the first problem', () => {
    expect(skillInputProblem(good)).toBeNull();
    expect(skillInputProblem({ ...good, slug: 'Bad' })).toBe(
      SKILL_SLUG_PROBLEM,
    );
    expect(skillInputProblem({ ...good, name: 'two\nlines' })).toBe(
      SKILL_NAME_PROBLEM,
    );
    expect(skillInputProblem({ ...good, name: 'x'.repeat(65) })).toBe(
      SKILL_NAME_PROBLEM,
    );
    expect(skillInputProblem({ ...good, description: '' })).toBe(
      SKILL_DESCRIPTION_PROBLEM,
    );
    expect(skillInputProblem({ ...good, body: '  ' })).toBe(SKILL_BODY_MISSING);
    expect(
      skillInputProblem({ ...good, body: 'é'.repeat(16 * 1024 + 1) }),
    ).toBe(SKILL_BODY_TOO_LARGE);
  });

  it('refuses reserved slugs but not names that merely start like one', () => {
    for (const slug of ['nul', 'lpt9', 'import'])
      expect(skillInputProblem({ ...good, slug })).toBe(SKILL_SLUG_PROBLEM);
    expect(skillInputProblem({ ...good, slug: 'console' })).toBeNull();
  });

  it('treats C0 and C1 control characters as control characters', () => {
    expect(hasControlCharacter('tab\t')).toBe(true);
    expect(hasControlCharacter('\u0085')).toBe(true);
    expect(hasControlCharacter('plain é')).toBe(false);
  });
});

describe('labels', () => {
  it('say a file draft was not written on the page', () => {
    expect(SOURCE_LABELS.file).toBe(
      'Found in the skills folder (not written on this page)',
    );
  });
});

describe('revealInvisible', () => {
  it('shows format characters as visible markers and counts them', () => {
    expect(revealInvisible('a\u200Bb')).toEqual({
      text: 'a⟨U+200B⟩b',
      count: 1,
    });
    expect(revealInvisible('\u{E0041}')).toEqual({
      text: '⟨U+E0041⟩',
      count: 1,
    });
    expect(revealInvisible('x\u2028y')).toEqual({
      text: 'x⟨U+2028⟩y',
      count: 1,
    });
    expect(revealInvisible('plain é\n\t')).toEqual({
      text: 'plain é\n\t',
      count: 0,
    });
  });

  it('describes the count', () => {
    expect(invisibleNote(0)).toBeNull();
    expect(invisibleNote(1)).toBe('This text contains 1 invisible character');
    expect(invisibleNote(2)).toBe('This text contains 2 invisible characters');
  });
});

describe('splitFileDrafts', () => {
  it('keeps every other draft and the first 20 file drafts', () => {
    const files = Array.from({ length: 22 }, (_, index) =>
      skillDraftFixture(`file:s${index}`, { source: 'file' }),
    );
    const agents = ['skd_1', 'skd_2', 'skd_3'].map((id) =>
      skillDraftFixture(id),
    );
    const { shown, hidden } = splitFileDrafts([...files, ...agents]);
    expect(MAX_FILE_DRAFTS_SHOWN).toBe(20);
    expect(shown).toHaveLength(23);
    expect(hidden).toBe(2);
    expect(shown.filter((draft) => draft.source === 'file')).toEqual(
      files.slice(0, 20),
    );
    expect(moreFileDraftsNote(2)).toBe('2 more in the skills folder');
    expect(moreFileDraftsNote(0)).toBeNull();
  });
});
