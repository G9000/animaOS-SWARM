import { act, renderHook } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';

import {
  formatHashRoute,
  parseHashRoute,
  useHashRoute,
  type HashRoute,
} from './hash-route';

afterEach(() => {
  window.history.replaceState(null, '', '/');
});

describe('hash routes', () => {
  it('parses sessions and pages and falls back to a new chat', () => {
    expect(parseHashRoute('')).toEqual({ kind: 'home' });
    expect(parseHashRoute('#/')).toEqual({ kind: 'home' });
    expect(parseHashRoute('#/s/chat%3Aabc')).toEqual({
      kind: 'session',
      sessionId: 'chat:abc',
    });
    expect(parseHashRoute('#/s/legacy-room:0f')).toEqual({
      kind: 'session',
      sessionId: 'legacy-room:0f',
    });
    expect(parseHashRoute('#/work')).toEqual({ kind: 'page', page: 'work' });
    expect(parseHashRoute('#/capabilities')).toEqual({
      kind: 'page',
      page: 'capabilities',
    });
    for (const hash of ['#/unknown', '#/s/', '#/s/bad%20id', '#/s/%E0%A4%A']) {
      expect(parseHashRoute(hash)).toEqual({ kind: 'home' });
    }
  });

  it('formats routes that parse back to themselves', () => {
    const routes: HashRoute[] = [
      { kind: 'home' },
      { kind: 'session', sessionId: 'chat:abc' },
      { kind: 'page', page: 'connectors' },
    ];
    for (const route of routes) {
      expect(parseHashRoute(formatHashRoute(route))).toEqual(route);
    }
    expect(formatHashRoute({ kind: 'session', sessionId: 'chat:abc' })).toBe(
      '#/s/chat%3Aabc',
    );
  });

  it('follows the location and navigates with or without a history entry', () => {
    window.history.replaceState(null, '', '/#/work');
    const { result } = renderHook(() => useHashRoute());
    expect(result.current[0]).toEqual({ kind: 'page', page: 'work' });

    const before = window.history.length;
    act(() => result.current[1]({ kind: 'session', sessionId: 'chat:1' }));
    expect(window.location.hash).toBe('#/s/chat%3A1');
    expect(window.history.length).toBe(before + 1);
    expect(result.current[0]).toEqual({ kind: 'session', sessionId: 'chat:1' });

    act(() => result.current[1]({ kind: 'home' }, { replace: true }));
    expect(window.location.hash).toBe('#/');
    expect(window.history.length).toBe(before + 1);

    act(() => {
      window.history.replaceState(null, '', '/#/files');
      window.dispatchEvent(new HashChangeEvent('hashchange'));
    });
    expect(result.current[0]).toEqual({ kind: 'page', page: 'files' });
  });
});
