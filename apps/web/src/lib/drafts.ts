// Drafts are saved per agent and conversation in session storage (spec
// §15.5), so a reload keeps them. Without storage they live in memory only.

export function draftStorageKey(key: string): string {
  return `animaos.draft.${key.replace('\u0000', '/')}`;
}

export function loadDraft(key: string): string {
  try {
    return window.sessionStorage.getItem(draftStorageKey(key)) ?? '';
  } catch {
    return '';
  }
}

export function storeDraft(key: string, draft: string): void {
  try {
    if (draft) window.sessionStorage.setItem(draftStorageKey(key), draft);
    else window.sessionStorage.removeItem(draftStorageKey(key));
  } catch {
    // Storage is full or blocked: the draft stays in memory for this page.
  }
}
