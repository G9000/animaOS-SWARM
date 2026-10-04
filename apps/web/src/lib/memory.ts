import {
  DaemonHttpError,
  type Memory,
  type MemoryFact,
  type MemoryType,
} from '@animaOS-SWARM/sdk';

import { COMPANION_UNREACHABLE } from './approvals';

/** Memories read at once (the daemon's recent and search limit). */
export const MEMORY_PAGE_LIMIT = 200;

export const MEMORY_TYPES = [
  'fact',
  'observation',
  'task_result',
  'reflection',
] as const;

export const MEMORY_TYPE_LABELS: Record<MemoryType, string> = {
  fact: 'Fact',
  observation: 'Observation',
  task_result: 'Task result',
  reflection: 'Reflection',
};

/** 'relevance' keeps the daemon's order and is offered only while a search
 *  is active. */
export type MemorySort = 'relevance' | 'newest' | 'oldest' | 'importance';

export const MEMORY_SORT_LABELS: Record<MemorySort, string> = {
  relevance: 'Most relevant',
  newest: 'Newest',
  oldest: 'Oldest',
  importance: 'Most important',
};

/** A search hit carries its score. */
export type MemoryEntry = Memory & { score?: number };

export function filterByType(
  list: readonly MemoryEntry[],
  type: MemoryType | 'all',
): MemoryEntry[] {
  return type === 'all'
    ? [...list]
    : list.filter((memory) => memory.type === type);
}

/** Stable; ties newest first; 'relevance' returns the list as given. */
export function sortMemories(
  list: readonly MemoryEntry[],
  sort: MemorySort,
): MemoryEntry[] {
  const copy = [...list];
  if (sort === 'relevance') return copy;
  const newestFirst = (a: MemoryEntry, b: MemoryEntry) =>
    b.createdAt - a.createdAt;
  switch (sort) {
    case 'newest':
      return copy.sort(newestFirst);
    case 'oldest':
      return copy.sort(
        (a, b) => a.createdAt - b.createdAt || newestFirst(a, b),
      );
    case 'importance':
      return copy.sort(
        (a, b) => b.importance - a.importance || newestFirst(a, b),
      );
  }
}

/** Split on commas, trim, drop empties, keep each tag once. */
export function parseTagInput(text: string): string[] {
  const tags = text
    .split(',')
    .map((tag) => tag.trim())
    .filter((tag) => tag.length > 0);
  return [...new Set(tags)];
}

export function formatTags(tags: readonly string[] | null | undefined): string {
  return tags ? tags.join(', ') : '';
}

export function importanceLabel(importance: number): 'High' | 'Medium' | 'Low' {
  if (importance >= 0.7) return 'High';
  if (importance >= 0.4) return 'Medium';
  return 'Low';
}

export function humanizePredicate(predicate: string): string {
  return predicate.replace(/_/g, ' ');
}

/** The predicate as words, and the value, else the object's name. */
export function describeFact(fact: MemoryFact): {
  label: string;
  value: string;
} {
  return {
    label: humanizePredicate(fact.predicate),
    value: fact.value ?? fact.objectName ?? '',
  };
}

/** Preference facts apart from everything else the companion knows. */
export function groupFacts(facts: readonly MemoryFact[]): {
  preferences: MemoryFact[];
  about: MemoryFact[];
} {
  const preferences: MemoryFact[] = [];
  const about: MemoryFact[] = [];
  for (const fact of facts) {
    (fact.predicate.toLowerCase().includes('preference')
      ? preferences
      : about
    ).push(fact);
  }
  return { preferences, about };
}

/** Removes what the daemon refuses in memory text (it mirrors the daemon's
 *  `is_smuggling_character`): Unicode tag characters, bidirectional
 *  embeddings, overrides and isolates, and supplementary variation
 *  selectors. Zero-width joiners and spaces stay, so emoji sequences and
 *  joined scripts survive. */
export function stripInvisible(text: string): string {
  return text.replace(
    /[\u{E0000}-\u{E007F}\u{202A}-\u{202E}\u{2066}-\u{2069}\u{E0100}-\u{E01EF}]/gu,
    '',
  );
}

/** The daemon's own refusals are owner-readable; anything else is a network
 *  failure. */
export function memoryErrorMessage(error: unknown): {
  message: string;
  status: number | null;
} {
  return error instanceof DaemonHttpError
    ? { message: error.message, status: error.status }
    : { message: COMPANION_UNREACHABLE, status: null };
}

export const MEMORY_EMPTY =
  'Nothing is remembered yet. Your companion saves what matters as you talk.';
export const MEMORY_SEARCH_EMPTY = 'No memories match that search.';
export const FACTS_EMPTY =
  'Nothing about you is saved yet. Tell your companion what matters to you.';
export const ENTITIES_EMPTY = 'No people or things yet.';
export const MEMORY_GONE = 'That memory is already gone.';
export const FACT_GONE = 'That fact is already gone.';
export const ENTITY_GONE = 'That is already gone.';
export const DELETE_MEMORY_PROMPT =
  'Delete this memory? Your companion will forget it. This can’t be undone.';
export const DELETE_FACT_PROMPT =
  'Forget this? Your companion will stop using it.';
export const DELETE_ENTITY_PROMPT =
  'Remove this from People & things? Its connections go with it.';
export const FACT_EDIT_HINT =
  'Saving replaces this fact. The old value stays as history.';
export const REPLACED_LABEL = 'Replaced';
