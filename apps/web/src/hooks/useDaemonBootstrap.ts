import { useCallback, useEffect, useState } from 'react';

import {
  daemon,
  type DaemonProvider,
  type DaemonSnapshot,
} from '../lib/daemon-api';

export type DaemonConnection = 'unknown' | 'online' | 'offline';

export interface DaemonBootstrap {
  connection: DaemonConnection;
  loaded: boolean;
  agents: DaemonSnapshot[];
  providers: DaemonProvider[] | null;
  providersError: string | null;
  refreshAgents(): Promise<void>;
  retryProviders(): Promise<void>;
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

export function useDaemonBootstrap(): DaemonBootstrap {
  const [connection, setConnection] = useState<DaemonConnection>('unknown');
  const [loaded, setLoaded] = useState(false);
  const [agents, setAgents] = useState<DaemonSnapshot[]>([]);
  const [providers, setProviders] = useState<DaemonProvider[] | null>(null);
  const [providersError, setProvidersError] = useState<string | null>(null);

  const refreshAgents = useCallback(async () => {
    try {
      const response = await daemon.listAgents();
      setAgents(sortAgentSnapshots(response.agents));
      setConnection('online');
    } catch {
      setConnection('offline');
    } finally {
      setLoaded(true);
    }
  }, []);

  const retryProviders = useCallback(async () => {
    try {
      const response = await daemon.listProviders();
      setProviders(response.providers);
      setProvidersError(null);
    } catch (error) {
      setProvidersError(errorMessage(error));
    }
  }, []);

  const acceptAgentSnapshot = useCallback((snapshot: DaemonSnapshot) => {
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
    setAgents((current) =>
      sortAgentSnapshots(current.filter(({ state }) => state.id !== id)),
    );
  }, []);

  useEffect(() => {
    let cancelled = false;

    void Promise.allSettled([
      daemon.health(),
      daemon.listAgents(),
      daemon.listProviders(),
    ]).then(([healthResult, agentsResult, providersResult]) => {
      if (cancelled) {
        return;
      }

      if (agentsResult.status === 'fulfilled') {
        setAgents(sortAgentSnapshots(agentsResult.value.agents));
      }

      setConnection(
        healthResult.status === 'fulfilled' &&
          agentsResult.status === 'fulfilled'
          ? 'online'
          : 'offline',
      );
      setLoaded(true);

      if (providersResult.status === 'fulfilled') {
        setProviders(providersResult.value.providers);
        setProvidersError(null);
      } else {
        setProvidersError(errorMessage(providersResult.reason));
      }
    });

    const poll = window.setInterval(() => {
      void refreshAgents();
    }, 5_000);

    return () => {
      cancelled = true;
      window.clearInterval(poll);
    };
  }, [refreshAgents]);

  return {
    connection,
    loaded,
    agents,
    providers,
    providersError,
    refreshAgents,
    retryProviders,
    acceptAgentSnapshot,
    removeAgentSnapshot,
  };
}
