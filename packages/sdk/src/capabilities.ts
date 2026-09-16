/** Registered handlers are not a connection check or an agent permission grant. */
export interface DaemonCapabilityTool {
  name: string;
  description: string;
  category: 'workspace' | 'terminal' | 'memory' | 'team' | 'research' | 'productivity' | 'utility';
  requirements: string[];
}

export interface DaemonCapabilities {
  schemaVersion: 1;
  tools: DaemonCapabilityTool[];
  persistence: {
    /** Configured storage backend, not a successful storage health probe. */
    controlPlane: string;
    memory: string;
    /** A step journal is configured; this does not promise automatic run resumption. */
    executionJournal: boolean;
  };
  extensions: Array<{
    id: string;
    label: string;
    status: 'planned';
    description: string;
  }>;
  limitations: string[];
}
