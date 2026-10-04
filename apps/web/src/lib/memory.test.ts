import { DaemonHttpError } from '@animaOS-SWARM/sdk';
import { describe, expect, it } from 'vitest';

import { factFixture, memoryFixture } from '../test/memory';
import { COMPANION_UNREACHABLE } from './approvals';
import {
  DELETE_ENTITY_PROMPT,
  DELETE_FACT_PROMPT,
  DELETE_MEMORY_PROMPT,
  ENTITIES_EMPTY,
  ENTITY_GONE,
  FACT_EDIT_HINT,
  FACT_GONE,
  FACTS_EMPTY,
  MEMORY_EMPTY,
  MEMORY_GONE,
  MEMORY_PAGE_LIMIT,
  MEMORY_SEARCH_EMPTY,
  MEMORY_SORT_LABELS,
  MEMORY_TYPE_LABELS,
  MEMORY_TYPES,
  REPLACED_LABEL,
  describeFact,
  filterByType,
  formatTags,
  groupFacts,
  humanizePredicate,
  importanceLabel,
  memoryErrorMessage,
  parseTagInput,
  sortMemories,
  stripInvisible,
} from './memory';

describe('memory helpers', () => {
  it('filters by type and keeps the order', () => {
    const list = [
      memoryFixture('a', { type: 'fact' }),
      memoryFixture('b', { type: 'reflection' }),
      memoryFixture('c', { type: 'fact' }),
    ];
    expect(filterByType(list, 'fact').map((m) => m.id)).toEqual(['a', 'c']);
    expect(filterByType(list, 'all')).toEqual(list);
  });

  it('sorts newest, oldest, and by importance with newest breaking ties, and keeps relevance order as given', () => {
    const list = [
      memoryFixture('a', { createdAt: 1, importance: 0.9 }),
      memoryFixture('b', { createdAt: 3, importance: 0.2 }),
      memoryFixture('c', { createdAt: 2, importance: 0.9 }),
    ];
    const ids = (sort: Parameters<typeof sortMemories>[1]) =>
      sortMemories(list, sort).map((m) => m.id);
    expect(ids('newest')).toEqual(['b', 'c', 'a']);
    expect(ids('oldest')).toEqual(['a', 'c', 'b']);
    expect(ids('importance')).toEqual(['c', 'a', 'b']);
    expect(ids('relevance')).toEqual(['a', 'b', 'c']);
    expect(list.map((m) => m.id)).toEqual(['a', 'b', 'c']);
  });

  it('parses and formats tags', () => {
    expect(parseTagInput(' a, b ,,a,  c ')).toEqual(['a', 'b', 'c']);
    expect(parseTagInput('')).toEqual([]);
    expect(formatTags(['a', 'b'])).toBe('a, b');
    expect(formatTags(null)).toBe('');
    expect(formatTags(undefined)).toBe('');
  });

  it('labels importance at the boundaries 0.7 and 0.4', () => {
    expect(importanceLabel(0.7)).toBe('High');
    expect(importanceLabel(0.69)).toBe('Medium');
    expect(importanceLabel(0.4)).toBe('Medium');
    expect(importanceLabel(0.39)).toBe('Low');
  });

  it('humanizes predicates', () => {
    expect(humanizePredicate('communication_preference')).toBe(
      'communication preference',
    );
    expect(humanizePredicate('lives_in')).toBe('lives in');
  });

  it('describes a fact by its value, else its object', () => {
    expect(describeFact(factFixture('f', { value: 'Short answers' }))).toEqual({
      label: 'communication preference',
      value: 'Short answers',
    });
    expect(
      describeFact(
        factFixture('f', {
          predicate: 'works_with',
          value: null,
          objectName: 'Ada',
        }),
      ),
    ).toEqual({ label: 'works with', value: 'Ada' });
    expect(
      describeFact(factFixture('f', { value: null, objectName: null })).value,
    ).toBe('');
  });

  it('groups preference facts apart from the rest', () => {
    const facts = [
      factFixture('a', { predicate: 'communication_preference' }),
      factFixture('b', { predicate: 'lives_in' }),
      factFixture('c', { predicate: 'Food_PREFERENCE' }),
    ];
    const grouped = groupFacts(facts);
    expect(grouped.preferences.map((f) => f.id)).toEqual(['a', 'c']);
    expect(grouped.about.map((f) => f.id)).toEqual(['b']);
  });

  it('strips only the characters the daemon refuses', () => {
    const refused = [
      0xe0041, // tag
      0xe0000,
      0xe007f,
      0x202a, // bidi embeddings and overrides
      0x202e,
      0x2066, // isolates
      0x2069,
      0xe0100, // supplementary variation selectors
      0xe01ef,
    ].map((code) => String.fromCodePoint(code));
    expect(stripInvisible(`a${refused.join('b')}c`)).toBe('abbbbbbbbc');
    expect(stripInvisible('plain')).toBe('plain');
  });

  it('keeps zero-width joiners, spaces, and separators so emoji sequences survive', () => {
    const zwj = String.fromCodePoint(0x200d);
    const zwsp = String.fromCodePoint(0x200b);
    const separator = String.fromCodePoint(0x2028);
    const family = `${String.fromCodePoint(0x1f469)}${zwj}${String.fromCodePoint(0x1f467)}`;
    const text = `a${zwsp}${family}${separator}b`;
    expect(stripInvisible(text)).toBe(text);
  });

  it('maps a daemon refusal to its message and a network failure to the unreachable text', () => {
    expect(
      memoryErrorMessage(new DaemonHttpError(409, { error: 'Nope' })),
    ).toEqual({ message: 'Nope', status: 409 });
    expect(memoryErrorMessage(new TypeError('Failed to fetch'))).toEqual({
      message: COMPANION_UNREACHABLE,
      status: null,
    });
  });

  it('owner-facing strings', () => {
    expect(MEMORY_PAGE_LIMIT).toBe(200);
    expect(MEMORY_TYPES).toEqual([
      'fact',
      'observation',
      'task_result',
      'reflection',
    ]);
    expect(MEMORY_TYPE_LABELS.task_result).toBe('Task result');
    expect(MEMORY_SORT_LABELS.relevance).toBe('Most relevant');
    expect(MEMORY_EMPTY).toBe(
      'Nothing is remembered yet. Your companion saves what matters as you talk.',
    );
    expect(MEMORY_SEARCH_EMPTY).toBe('No memories match that search.');
    expect(FACTS_EMPTY).toBe(
      'Nothing about you is saved yet. Tell your companion what matters to you.',
    );
    expect(ENTITIES_EMPTY).toBe('No people or things yet.');
    expect(MEMORY_GONE).toBe('That memory is already gone.');
    expect(FACT_GONE).toBe('That fact is already gone.');
    expect(ENTITY_GONE).toBe('That is already gone.');
    expect(DELETE_MEMORY_PROMPT).toBe(
      'Delete this memory? Your companion will forget it. This can’t be undone.',
    );
    expect(DELETE_FACT_PROMPT).toBe(
      'Forget this? Your companion will stop using it.',
    );
    expect(DELETE_ENTITY_PROMPT).toBe(
      'Remove this from People & things? Its connections go with it.',
    );
    expect(FACT_EDIT_HINT).toBe(
      'Saving replaces this fact. The old value stays as history.',
    );
    expect(REPLACED_LABEL).toBe('Replaced');
  });
});
