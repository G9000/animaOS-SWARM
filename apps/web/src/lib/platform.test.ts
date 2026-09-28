import { afterEach, describe, expect, it, vi } from 'vitest';

import { isMacPlatform } from './platform';

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('isMacPlatform', () => {
  it('reads the modern platform hint first', () => {
    vi.stubGlobal('navigator', {
      userAgentData: { platform: 'macOS' },
      platform: 'Win32',
      userAgent: 'Windows',
    });
    expect(isMacPlatform()).toBe(true);
  });

  it('falls back to navigator.platform, then userAgent', () => {
    vi.stubGlobal('navigator', { platform: 'MacIntel', userAgent: '' });
    expect(isMacPlatform()).toBe(true);

    vi.stubGlobal('navigator', { platform: '', userAgent: 'iPhone' });
    expect(isMacPlatform()).toBe(true);
  });

  it('says every other platform is not a Mac', () => {
    vi.stubGlobal('navigator', { platform: 'Win32', userAgent: 'Windows' });
    expect(isMacPlatform()).toBe(false);
    vi.stubGlobal('navigator', { platform: 'Linux x86_64', userAgent: '' });
    expect(isMacPlatform()).toBe(false);
  });
});
