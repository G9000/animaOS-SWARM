import { useCallback, useEffect, useRef, useState } from 'react';

import {
  daemon,
  type DaemonProvider,
  type DaemonSnapshot,
  type DaemonWorkspaceState,
} from '../lib/daemon-api';

export type DaemonConnection = 'unknown' | 'online' | 'offline';

/** Full agent snapshots are re-read this often without the event stream. */
export const BOOTSTRAP_POLL_MS = 5_000;
/** With the stream open, agent summaries are re-read this often (spec §15.5). */
export const BOOTSTRAP_SUMMARY_POLL_MS = 30_000;

export interface DaemonBootstrapOptions {
  /** The companion's event stream is open: poll summaries, not snapshots. */
  live?: boolean;
}

export interface DaemonBootstrap {
  connection: DaemonConnection;
  loaded: boolean;
  agents: DaemonSnapshot[];
  providers: DaemonProvider[] | null;
  providersError: string | null;
  workspace: DaemonWorkspaceState | null;
  refreshAgents(): Promise<void>;
  retryProviders(): Promise<void>;
  refreshWorkspace(): Promise<void>;
  acceptAgentSnapshot(snapshot: DaemonSnapshot): void;
  removeAgentSnapshot(id: string): void;
}

function sortAgentSnapshots(
  snapshots: readonly DaemonSnapshot[],
): DaemonSnapshot[] {
  return [...snapshots].sort((left, right) => {
    const creationOrder = left.state.createdAtMs - right.state.createdAtMs;
    if (creationOrder !== 0) {
      return creationOrder;
    }

    if (left.state.id < right.state.id) {
      return -1;
    }
    if (left.state.id > right.state.id) {
      return 1;
    }
    return 0;
  });
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function useDaemonBootstrap(
  options: DaemonBootstrapOptions = {},
): DaemonBootstrap {
  const live = options.live ?? false;
  const [connection, setConnection] = useState<DaemonConnection>('unknown');
  const [loaded, setLoaded] = useState(false);
  const [agents, setAgents] = useState<DaemonSnapshot[]>([]);
  const [providers, setProviders] = useState<DaemonProvider[] | null>(null);
  const [providersError, setProvidersError] = useState<string | null>(null);
  const [workspace, setWorkspace] = useState<DaemonWorkspaceState | null>(null);
  const mountedRef = useRef(false);
  const agentRequestGenerationRef = useRef(0);
  const collectionMutationEpochRef = useRef(0);
  const providerRequestGenerationRef = useRef(0);
  const workspaceRequestGenerationRef = useRef(0);

  const refreshAgents = useCallback(async () => {
    const requestGeneration = ++agentRequestGenerationRef.current;
    const mutationEpoch = collectionMutationEpochRef.current;

    try {
      const response = await daemon.listAgents();
      if (
        !mountedRef.current ||
        requestGeneration !== agentRequestGenerationRef.current
      ) {
        return;
      }

      if (mutationEpoch === collectionMutationEpochRef.current) {
        setAgents(sortAgentSnapshots(response.agents));
      }
      setConnection('online');
    } catch {
      if (
        !mountedRef.current ||
        requestGeneration !== agentRequestGenerationRef.current
      ) {
        return;
      }
      setConnection('offline');
    } finally {
      if (
        mountedRef.current &&
        requestGeneration === agentRequestGenerationRef.current
      ) {
        setLoaded(true);
      }
    }
  }, []);

  /** Agent records without transcripts (`view=summary`); each keeps the
   *  messages of its last full snapshot. */
  const refreshSummaries = useCallback(async () => {
    const requestGeneration = ++agentRequestGenerationRef.current;
    const mutationEpoch = collectionMutationEpochRef.current;

    try {
      const summaries = await daemon.listAgentSummaries();
      if (
        !mountedRef.current ||
        requestGeneration !== agentRequestGenerationRef.current
      ) {
        return;
      }

      if (mutationEpoch === collectionMutationEpochRef.current) {
        setAgents((current) =>
          sortAgentSnapshots(
            summaries.map((summary) => ({
              state: summary.state,
              messageCount: summary.messageCount,
              eventCount: summary.eventCount,
              messages:
                current.find(({ state }) => state.id === summary.state.id)
                  ?.messages ?? [],
            })),
          ),
        );
      }
      setConnection('online');
    } catch {
      if (
        !mountedRef.current ||
        requestGeneration !== agentRequestGenerationRef.current
      ) {
        return;
      }
      setConnection('offline');
    }
  }, []);

  const retryProviders = useCallback(async () => {
    const requestGeneration = ++providerRequestGenerationRef.current;

    try {
      const response = await daemon.listProviders();
      if (
        !mountedRef.current ||
        requestGeneration !== providerRequestGenerationRef.current
      ) {
        return;
      }
      setProviders(response.providers);
      setProvidersError(null);
    } catch (error) {
      if (
        !mountedRef.current ||
        requestGeneration !== providerRequestGenerationRef.current
      ) {
        return;
      }
      setProvidersError(errorMessage(error));
    }
  }, []);

  const refreshWorkspace = useCallback(async () => {
    const requestGeneration = ++workspaceRequestGenerationRef.current;

    try {
      const state = await daemon.getWorkspace();
      if (
        !mountedRef.current ||
        requestGeneration !== workspaceRequestGenerationRef.current
      ) {
        return;
      }
      setWorkspace(state);
    } catch {
      if (
        !mountedRef.current ||
        requestGeneration !== workspaceRequestGenerationRef.current
      ) {
        return;
      }
      // The workspace is optional context: never fail the bootstrap over it.
      setWorkspace(null);
    }
  }, []);

  const acceptAgentSnapshot = useCallback((snapshot: DaemonSnapshot) => {
    collectionMutationEpochRef.current += 1;
    setAgents((current) => {
      const matchingIndex = current.findIndex(
        ({ state }) => state.id === snapshot.state.id,
      );
      const next = [...current];

      if (matchingIndex === -1) {
        next.push(snapshot);
      } else {
        next[matchingIndex] = snapshot;
      }

      return sortAgentSnapshots(next);
    });
  }, []);

  const removeAgentSnapshot = useCallback((id: string) => {
    collectionMutationEpochRef.current += 1;
    setAgents((current) =>
      sortAgentSnapshots(current.filter(({ state }) => state.id !== id)),
    );
  }, []);

  useEffect(() => {
    mountedRef.current = true;
    let active = true;

    const requestGeneration = ++agentRequestGenerationRef.current;
    const mutationEpoch = collectionMutationEpochRef.current;
    void Promise.allSettled([daemon.health(), daemon.listAgents()]).then(
      ([healthResult, agentsResult]) => {
        if (!active || !mountedRef.current) {
          return;
        }

        if (requestGeneration === agentRequestGenerationRef.current) {
          if (
            agentsResult.status === 'fulfilled' &&
            mutationEpoch === collectionMutationEpochRef.current
          ) {
            setAgents(sortAgentSnapshots(agentsResult.value.agents));
          }

          setConnection(
            healthResult.status === 'fulfilled' &&
              agentsResult.status === 'fulfilled'
              ? 'online'
              : 'offline',
          );
        }
        setLoaded(true);
      },
    );
    void retryProviders();
    void refreshWorkspace();

    return () => {
      active = false;
      mountedRef.current = false;
      agentRequestGenerationRef.current += 1;
      providerRequestGenerationRef.current += 1;
      workspaceRequestGenerationRef.current += 1;
    };
  }, [retryProviders, refreshWorkspace]);

  // Polling starts once the first read settled and waits for each poll to
  // settle; with the stream open it reads summaries every 30 s instead.
  useEffect(() => {
    if (!loaded) return;
    let active = true;
    let pollTimer: number | undefined;

    const schedulePoll = () => {
      if (!active || !mountedRef.current) {
        return;
      }

      pollTimer = window.setTimeout(
        () => {
          pollTimer = undefined;
          void Promise.allSettled([
            live ? refreshSummaries() : refreshAgents(),
            refreshWorkspace(),
          ]).then(schedulePoll);
        },
        live ? BOOTSTRAP_SUMMARY_POLL_MS : BOOTSTRAP_POLL_MS,
      );
    };

    schedulePoll();
    return () => {
      active = false;
      if (pollTimer !== undefined) {
        window.clearTimeout(pollTimer);
      }
    };
  }, [loaded, live, refreshAgents, refreshSummaries, refreshWorkspace]);

  return {
    connection,
    loaded,
    agents,
    providers,
    providersError,
    workspace,
    refreshAgents,
    retryProviders,
    refreshWorkspace,
    acceptAgentSnapshot,
    removeAgentSnapshot,
  };
}
