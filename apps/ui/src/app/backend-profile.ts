export interface BackendProfile {
  hostKey: string;
  supportsDocuments: boolean;
  supportsLiveEvents: boolean;
}

export function getBackendProfile(hostKey: string | undefined): BackendProfile {
  if (hostKey === 'rust') {
    return {
      hostKey: 'rust',
      supportsDocuments: false,
      supportsLiveEvents: false,
    };
  }

  return {
    hostKey: 'server',
    supportsDocuments: true,
    supportsLiveEvents: true,
  };
}
