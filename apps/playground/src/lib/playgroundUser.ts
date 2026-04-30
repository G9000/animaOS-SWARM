const fallbackId = 'playground-user';
const storageKey = 'animaos.playground.userId';

export function playgroundUserId(): string {
  let userId = fallbackId;

  try {
    const existing = localStorage.getItem(storageKey);
    userId = existing?.trim() || `playground-user-${crypto.randomUUID()}`;
    localStorage.setItem(storageKey, userId);
  } catch {
    userId = fallbackId;
  }

  return userId;
}

export function playgroundUserMetadata(): Record<string, string> {
  return {
    userId: playgroundUserId(),
    userName: 'Playground User',
  };
}