import { describe, expect, it } from 'vitest';

import { getBackendProfile } from './backend-profile';

describe('backend profile', () => {
  it('defaults to the full TypeScript server capability set', () => {
    expect(getBackendProfile(undefined)).toEqual({
      hostKey: 'server',
      supportsDocuments: true,
      supportsLiveEvents: true,
    });
  });

  it('disables unsupported features for the rust host', () => {
    expect(getBackendProfile('rust')).toEqual({
      hostKey: 'rust',
      supportsDocuments: false,
      supportsLiveEvents: false,
    });
  });
});
