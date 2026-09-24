import { useCallback, useEffect, useState } from 'react';

/** The pages of spec §15.1; later milestones build the ones this release hides. */
export const HASH_PAGES = [
  'approvals',
  'automations',
  'memory',
  'skills',
  'work',
  'files',
  'connectors',
  'usage',
  'logs',
  'health',
  'capabilities',
] as const;
export type HashPage = (typeof HASH_PAGES)[number];

export type HashRoute =
  | { kind: 'home' }
  | { kind: 'session'; sessionId: string }
  | { kind: 'page'; page: HashPage };

const SESSION_ID = /^[A-Za-z0-9._:-]{1,200}$/;

/** `#/s/<sessionId>` or `#/<page>`; anything else is a new chat. */
export function parseHashRoute(hash: string): HashRoute {
  const path = hash.startsWith('#') ? hash.slice(1) : hash;
  if (path.startsWith('/s/')) {
    try {
      const sessionId = decodeURIComponent(path.slice(3));
      if (SESSION_ID.test(sessionId)) return { kind: 'session', sessionId };
    } catch {
      // A malformed escape opens a new chat.
    }
    return { kind: 'home' };
  }
  const page = path.replace(/^\/+/, '');
  return (HASH_PAGES as readonly string[]).includes(page)
    ? { kind: 'page', page: page as HashPage }
    : { kind: 'home' };
}

export function formatHashRoute(route: HashRoute): string {
  switch (route.kind) {
    case 'home':
      return '#/';
    case 'session':
      return `#/s/${encodeURIComponent(route.sessionId)}`;
    case 'page':
      return `#/${route.page}`;
  }
}

export function sameRoute(left: HashRoute, right: HashRoute): boolean {
  return formatHashRoute(left) === formatHashRoute(right);
}

export type Navigate = (route: HashRoute, options?: { replace?: boolean }) => void;

/** The current hash route; a reload restores it and Back/Forward follow it. */
export function useHashRoute(): [HashRoute, Navigate] {
  const [route, setRoute] = useState<HashRoute>(() =>
    parseHashRoute(window.location.hash),
  );
  useEffect(() => {
    const update = () => setRoute(parseHashRoute(window.location.hash));
    window.addEventListener('hashchange', update);
    window.addEventListener('popstate', update);
    return () => {
      window.removeEventListener('hashchange', update);
      window.removeEventListener('popstate', update);
    };
  }, []);
  const navigate = useCallback<Navigate>((next, options = {}) => {
    const hash = formatHashRoute(next);
    if (options.replace) {
      window.history.replaceState(window.history.state, '', hash);
    } else if (window.location.hash !== hash) {
      window.history.pushState(window.history.state, '', hash);
    }
    setRoute(parseHashRoute(hash));
  }, []);
  return [route, navigate];
}
