import { afterEach, describe, expect, it, vi } from 'vitest';

import { draftStorageKey, loadDraft, storeDraft } from './drafts';

afterEach(() => {
  sessionStorage.clear();
  vi.restoreAllMocks();
});

describe('drafts', () => {
  it('keeps a draft per agent and conversation in session storage', () => {
    const key = 'agent-main\u0000session:chat:1';
    storeDraft(key, 'Half a thought');

    expect(
      sessionStorage.getItem('animaos.draft.agent-main/session:chat:1'),
    ).toBe('Half a thought');
    expect(loadDraft(key)).toBe('Half a thought');
    storeDraft(key, '');
    expect(loadDraft(key)).toBe('');
    expect(draftStorageKey('a\u0000home')).toBe('animaos.draft.a/home');
  });

  it('never throws when storage is blocked', () => {
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
      throw new DOMException('denied', 'SecurityError');
    });
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new DOMException('full', 'QuotaExceededError');
    });

    expect(() => storeDraft('k', 'text')).not.toThrow();
    expect(loadDraft('k')).toBe('');
  });
});
