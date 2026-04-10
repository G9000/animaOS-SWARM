import {
  startTransition,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import styles from './app.module.css';
import {
  applyLiveEvent,
  buildLiveActivity,
  buildLiveOutputDelta,
  type AgentEntry,
  type DashboardSnapshot,
  type HealthSnapshot,
  type LiveEvent,
  type SectionId,
  type SwarmEntry,
  type Tone,
} from './live-events';
import {
  formatDocumentResult,
  formatTaskHistoryResult,
  type SearchResult,
} from './search-results';

type ConnectionState = 'connecting' | 'ready' | 'refreshing' | 'offline';

interface TaskOutcome {
  status: string;
  data?: unknown;
  error?: string;
  durationMs?: number;
}

interface ActivityEntry {
  id: string;
  scope: SectionId | 'system';
  title: string;
  body: string;
  tone: Tone;
  timestamp: number;
}

interface AgentListResponse {
  agents: AgentEntry[];
}

interface SwarmListResponse {
  swarms: SwarmEntry[];
}

interface SearchResponse {
  results: SearchResult[];
}

const NAV_ITEMS: Array<{
  id: SectionId;
  label: string;
  eyebrow: string;
  description: string;
}> = [
  {
    id: 'agents',
    label: 'Agents',
    eyebrow: 'Roster',
    description: 'Create operators, assign work, inspect output.',
  },
  {
    id: 'swarms',
    label: 'Swarms',
    eyebrow: 'Coordination',
    description: 'Stage strategies, run delegations, watch capacity.',
  },
  {
    id: 'search',
    label: 'Search',
    eyebrow: 'Memory',
    description: 'Query task history and document knowledge.',
  },
  {
    id: 'health',
    label: 'Health',
    eyebrow: 'System',
    description: 'Track service state, uptime, and exposed endpoints.',
  },
];

const DEFAULT_AGENT_MODEL = 'gpt-5.4';
const DASHBOARD_POLL_MS = 30_000;
const LIVE_RECONNECT_MS = 1_500;

function formatUptime(totalSeconds: number | undefined): string {
  const safeSeconds = Math.max(0, Math.floor(totalSeconds ?? 0));
  const hours = String(Math.floor(safeSeconds / 3600)).padStart(2, '0');
  const minutes = String(Math.floor((safeSeconds % 3600) / 60)).padStart(
    2,
    '0'
  );
  const seconds = String(safeSeconds % 60).padStart(2, '0');
  return `${hours}:${minutes}:${seconds}`;
}

function formatTimestamp(timestamp: number): string {
  return new Date(timestamp).toLocaleTimeString([], {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  });
}

function getErrorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function serializePayload(payload: unknown): string {
  if (typeof payload === 'string') {
    return payload;
  }

  if (payload === null || typeof payload === 'undefined') {
    return 'No payload returned.';
  }

  try {
    return JSON.stringify(payload, null, 2);
  } catch {
    return String(payload);
  }
}

function createActivityId(scope: string): string {
  return `${scope}-${Date.now().toString(36)}-${Math.random()
    .toString(36)
    .slice(2, 8)}`;
}

function parseWorkerNames(workerNames: string, model: string) {
  return workerNames
    .split(',')
    .map((value) => value.trim())
    .filter(Boolean)
    .map((name) => ({ name, model }));
}

async function requestJson<T>(
  path: string,
  init?: Omit<RequestInit, 'body'> & { body?: unknown }
): Promise<T> {
  const headers = new Headers(init?.headers);
  let body: BodyInit | undefined;

  if (typeof init?.body !== 'undefined') {
    headers.set('content-type', 'application/json');
    body = JSON.stringify(init.body);
  }

  const response = await fetch(path, {
    ...init,
    headers,
    body,
  });
  const text = await response.text();

  let parsed: unknown = undefined;
  if (text) {
    try {
      parsed = JSON.parse(text);
    } catch {
      parsed = text;
    }
  }

  const contentType = response.headers.get('content-type') ?? '';
  if (!contentType.toLowerCase().includes('application/json')) {
    throw new Error(
      `Expected JSON from ${path}, received ${
        contentType || 'unknown response type'
      }.`
    );
  }

  if (!response.ok) {
    if (
      parsed &&
      typeof parsed === 'object' &&
      'error' in parsed &&
      typeof parsed.error === 'string'
    ) {
      throw new Error(parsed.error);
    }

    throw new Error(
      typeof parsed === 'string'
        ? parsed
        : `Request failed with status ${String(response.status)}`
    );
  }

  return parsed as T;
}

async function fetchDashboardSnapshot(): Promise<DashboardSnapshot> {
  const [health, agents, swarms] = await Promise.all([
    requestJson<HealthSnapshot>('/api/health'),
    requestJson<AgentListResponse>('/api/agents'),
    requestJson<SwarmListResponse>('/api/swarms'),
  ]);

  return {
    health,
    agents: agents.agents,
    swarms: swarms.swarms,
  };
}

function Panel({
  eyebrow,
  title,
  actions,
  children,
}: {
  eyebrow: string;
  title: string;
  actions?: ReactNode;
  children: ReactNode;
}): ReactNode {
  return (
    <section className={styles.panel}>
      <div className={styles.panelHeader}>
        <div>
          <div className={styles.panelEyebrow}>{eyebrow}</div>
          <h2 className={styles.panelTitle}>{title}</h2>
        </div>
        {actions ? <div className={styles.panelActions}>{actions}</div> : null}
      </div>
      <div className={styles.panelBody}>{children}</div>
    </section>
  );
}

function ActivityConsole({
  entries,
  emptyLabel,
}: {
  entries: ActivityEntry[];
  emptyLabel: string;
}): ReactNode {
  if (entries.length === 0) {
    return <div className={styles.emptyState}>{emptyLabel}</div>;
  }

  return (
    <div className={styles.activityFeed}>
      {entries.map((entry) => (
        <article
          key={entry.id}
          className={`${styles.activityEntry} ${
            styles[`tone${entry.tone[0].toUpperCase()}${entry.tone.slice(1)}`]
          }`}
        >
          <div className={styles.activityMeta}>
            <span>{entry.title}</span>
            <span>{formatTimestamp(entry.timestamp)}</span>
          </div>
          <pre className={styles.activityBody}>{entry.body}</pre>
        </article>
      ))}
    </div>
  );
}

export function App() {
  const [section, setSection] = useState<SectionId>('agents');
  const [connectionState, setConnectionState] =
    useState<ConnectionState>('connecting');
  const [liveEventsConnected, setLiveEventsConnected] = useState(false);
  const [connectionMessage, setConnectionMessage] = useState<string | null>(
    null
  );
  const [lastSyncedAt, setLastSyncedAt] = useState<number | null>(null);
  const [health, setHealth] = useState<HealthSnapshot | null>(null);
  const [agents, setAgents] = useState<AgentEntry[]>([]);
  const [swarms, setSwarms] = useState<SwarmEntry[]>([]);
  const [selectedAgentId, setSelectedAgentId] = useState<string | null>(null);
  const [selectedSwarmId, setSelectedSwarmId] = useState<string | null>(null);
  const [busyAction, setBusyAction] = useState<string | null>(null);
  const [activity, setActivity] = useState<ActivityEntry[]>([]);
  const [agentDraft, setAgentDraft] = useState({
    name: '',
    model: DEFAULT_AGENT_MODEL,
  });
  const [swarmDraft, setSwarmDraft] = useState({
    strategy: 'supervisor',
    managerName: 'manager',
    model: DEFAULT_AGENT_MODEL,
    workers: 'researcher, operator',
  });
  const [agentTask, setAgentTask] = useState('');
  const [swarmTask, setSwarmTask] = useState('');
  const [agentOutputs, setAgentOutputs] = useState<Record<string, string>>({});
  const [swarmOutputs, setSwarmOutputs] = useState<Record<string, string>>({});
  const [historyQuery, setHistoryQuery] = useState('launch');
  const [historyResults, setHistoryResults] = useState<SearchResult[]>([]);
  const [documentDraft, setDocumentDraft] = useState({
    id: 'ops-playbook',
    text: '',
  });
  const [documentQuery, setDocumentQuery] = useState('operator');
  const [documentResults, setDocumentResults] = useState<SearchResult[]>([]);
  const healthRef = useRef<HealthSnapshot | null>(null);
  const agentsRef = useRef<AgentEntry[]>([]);
  const swarmsRef = useRef<SwarmEntry[]>([]);

  const selectedAgent = agents.find((entry) => entry.id === selectedAgentId);
  const selectedSwarm = swarms.find((entry) => entry.id === selectedSwarmId);
  const relevantActivity = activity.filter(
    (entry) => entry.scope === 'system' || entry.scope === section
  );
  const canStreamLiveEvents = connectionState !== 'offline' && health !== null;

  function addActivity(
    scope: SectionId | 'system',
    title: string,
    body: string,
    tone: Tone = 'neutral',
    timestamp = Date.now()
  ) {
    setActivity((current) =>
      [
        {
          id: createActivityId(scope),
          scope,
          title,
          body,
          tone,
          timestamp,
        },
        ...current,
      ].slice(0, 24)
    );
  }

  async function refreshDashboard(source: 'auto' | 'manual' = 'manual') {
    setConnectionState((current) =>
      current === 'ready' || source === 'manual' ? 'refreshing' : 'connecting'
    );

    try {
      const snapshot = await fetchDashboardSnapshot();
      healthRef.current = snapshot.health;
      agentsRef.current = snapshot.agents;
      swarmsRef.current = snapshot.swarms;

      startTransition(() => {
        setHealth(snapshot.health);
        setAgents(snapshot.agents);
        setSwarms(snapshot.swarms);
        setSelectedAgentId((current) =>
          snapshot.agents.some((entry) => entry.id === current)
            ? current
            : snapshot.agents[0]?.id ?? null
        );
        setSelectedSwarmId((current) =>
          snapshot.swarms.some((entry) => entry.id === current)
            ? current
            : snapshot.swarms[0]?.id ?? null
        );
        setConnectionState('ready');
        setConnectionMessage(null);
        setLastSyncedAt(Date.now());
      });
    } catch (error) {
      const message = getErrorMessage(error);

      startTransition(() => {
        setConnectionState('offline');
        setConnectionMessage(message);
        setLastSyncedAt(Date.now());
      });

      if (source === 'manual') {
        addActivity('system', 'Dashboard refresh failed', message, 'error');
      }
    }
  }

  useEffect(() => {
    healthRef.current = health;
  }, [health]);

  useEffect(() => {
    agentsRef.current = agents;
  }, [agents]);

  useEffect(() => {
    swarmsRef.current = swarms;
  }, [swarms]);

  useEffect(() => {
    setSelectedAgentId((current) =>
      agents.some((entry) => entry.id === current)
        ? current
        : agents[0]?.id ?? null
    );
  }, [agents]);

  useEffect(() => {
    setSelectedSwarmId((current) =>
      swarms.some((entry) => entry.id === current)
        ? current
        : swarms[0]?.id ?? null
    );
  }, [swarms]);

  useEffect(() => {
    void refreshDashboard('auto');

    const interval = window.setInterval(() => {
      void refreshDashboard('auto');
    }, DASHBOARD_POLL_MS);

    return () => {
      window.clearInterval(interval);
    };
  }, []);

  useEffect(() => {
    if (!canStreamLiveEvents || typeof window === 'undefined') {
      setLiveEventsConnected(false);
      return;
    }

    let socket: WebSocket | null = null;
    let reconnectTimer: number | null = null;
    let disposed = false;

    const clearReconnectTimer = () => {
      if (reconnectTimer !== null) {
        window.clearTimeout(reconnectTimer);
        reconnectTimer = null;
      }
    };

    const scheduleReconnect = () => {
      if (disposed || reconnectTimer !== null) {
        return;
      }

      reconnectTimer = window.setTimeout(() => {
        reconnectTimer = null;
        connect();
      }, LIVE_RECONNECT_MS);
    };

    const handleMessage = (message: MessageEvent<string>) => {
      let liveEvent: LiveEvent;

      try {
        liveEvent = JSON.parse(message.data) as LiveEvent;
      } catch {
        return;
      }

      const currentHealth = healthRef.current;
      if (!currentHealth) {
        return;
      }

      const nextSnapshot = applyLiveEvent(
        {
          health: currentHealth,
          agents: agentsRef.current,
          swarms: swarmsRef.current,
        },
        liveEvent
      );
      const liveActivity = buildLiveActivity(liveEvent);
      const outputDelta = buildLiveOutputDelta(liveEvent);

      healthRef.current = nextSnapshot.health;
      agentsRef.current = nextSnapshot.agents;
      swarmsRef.current = nextSnapshot.swarms;

      startTransition(() => {
        setHealth(nextSnapshot.health);
        setAgents(nextSnapshot.agents);
        setSwarms(nextSnapshot.swarms);
        setLastSyncedAt(liveEvent.timestamp);

        const agentOutput = outputDelta?.agentOutput;
        if (agentOutput) {
          setAgentOutputs((current) => ({
            ...current,
            [agentOutput.id]: agentOutput.body,
          }));
        }

        const swarmOutput = outputDelta?.swarmOutput;
        if (swarmOutput) {
          setSwarmOutputs((current) => ({
            ...current,
            [swarmOutput.id]: swarmOutput.body,
          }));
        }
      });

      if (liveActivity) {
        addActivity(
          liveActivity.scope,
          liveActivity.title,
          liveActivity.body,
          liveActivity.tone,
          liveEvent.timestamp
        );
      }
    };

    const connect = () => {
      if (disposed) {
        return;
      }

      const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
      socket = new WebSocket(`${protocol}//${window.location.host}/ws`);

      socket.addEventListener('open', () => {
        clearReconnectTimer();
        setLiveEventsConnected(true);
      });

      socket.addEventListener('message', handleMessage);

      socket.addEventListener('close', () => {
        setLiveEventsConnected(false);
        if (!disposed) {
          scheduleReconnect();
        }
      });

      socket.addEventListener('error', () => {
        socket?.close();
      });
    };

    connect();

    return () => {
      disposed = true;
      clearReconnectTimer();
      setLiveEventsConnected(false);

      if (socket) {
        socket.close();
      }
    };
  }, [canStreamLiveEvents]);

  async function handleCreateAgent() {
    if (!agentDraft.name.trim() || !agentDraft.model.trim()) {
      setConnectionMessage('Agent name and model are required.');
      return;
    }

    setBusyAction('create-agent');

    try {
      const created = await requestJson<{ id: string; name: string }>(
        '/api/agents',
        {
          method: 'POST',
          body: {
            name: agentDraft.name.trim(),
            model: agentDraft.model.trim(),
          },
        }
      );

      startTransition(() => {
        setSelectedAgentId(created.id);
        setAgentDraft((current) => ({
          ...current,
          name: '',
        }));
      });

      if (!liveEventsConnected) {
        addActivity(
          'agents',
          `Agent created: ${created.name}`,
          `Agent ${created.id} is staged and ready for tasks.`,
          'success'
        );
        await refreshDashboard('manual');
      }
    } catch (error) {
      const message = getErrorMessage(error);
      setConnectionMessage(message);
      addActivity('agents', 'Agent create failed', message, 'error');
    } finally {
      setBusyAction(null);
    }
  }

  async function handleRunAgent() {
    if (!selectedAgentId || !agentTask.trim()) {
      return;
    }

    setBusyAction('run-agent');

    try {
      const result = await requestJson<TaskOutcome>(
        `/api/agents/${selectedAgentId}/run`,
        {
          method: 'POST',
          body: { task: agentTask.trim() },
        }
      );
      const body =
        result.status === 'success'
          ? serializePayload(result.data)
          : result.error ?? 'Agent task failed.';

      startTransition(() => {
        setAgentOutputs((current) => ({
          ...current,
          [selectedAgentId]: body,
        }));
        setAgentTask('');
      });

      if (!liveEventsConnected) {
        addActivity(
          'agents',
          `${selectedAgent?.name ?? 'Agent'} ${result.status}`,
          body,
          result.status === 'success' ? 'success' : 'error'
        );
        await refreshDashboard('manual');
      }
    } catch (error) {
      const message = getErrorMessage(error);
      setConnectionMessage(message);
      addActivity('agents', 'Agent run failed', message, 'error');
    } finally {
      setBusyAction(null);
    }
  }

  async function handleDeleteAgent() {
    if (!selectedAgentId) {
      return;
    }

    setBusyAction('delete-agent');

    try {
      await requestJson<{ deleted: boolean }>(
        `/api/agents/${selectedAgentId}`,
        {
          method: 'DELETE',
        }
      );

      startTransition(() => {
        setAgentOutputs((current) => {
          const next = { ...current };
          delete next[selectedAgentId];
          return next;
        });
      });

      if (!liveEventsConnected) {
        addActivity(
          'agents',
          `Agent removed: ${selectedAgent?.name ?? selectedAgentId}`,
          'The selected operator has been removed from the local server surface.',
          'success'
        );
        await refreshDashboard('manual');
      }
    } catch (error) {
      const message = getErrorMessage(error);
      setConnectionMessage(message);
      addActivity('agents', 'Agent delete failed', message, 'error');
    } finally {
      setBusyAction(null);
    }
  }

  async function handleCreateSwarm() {
    const workers = parseWorkerNames(
      swarmDraft.workers,
      swarmDraft.model.trim() || DEFAULT_AGENT_MODEL
    );

    if (!swarmDraft.managerName.trim() || workers.length === 0) {
      setConnectionMessage(
        'Manager name and at least one worker are required.'
      );
      return;
    }

    setBusyAction('create-swarm');

    try {
      const created = await requestJson<{ id: string; strategy: string }>(
        '/api/swarms',
        {
          method: 'POST',
          body: {
            strategy: swarmDraft.strategy,
            manager: {
              name: swarmDraft.managerName.trim(),
              model: swarmDraft.model.trim() || DEFAULT_AGENT_MODEL,
            },
            workers,
          },
        }
      );

      startTransition(() => {
        setSelectedSwarmId(created.id);
      });

      if (!liveEventsConnected) {
        addActivity(
          'swarms',
          `Swarm created: ${created.strategy}`,
          `Coordinator ${created.id} is ready with ${String(
            workers.length
          )} workers.`,
          'success'
        );
        await refreshDashboard('manual');
      }
    } catch (error) {
      const message = getErrorMessage(error);
      setConnectionMessage(message);
      addActivity('swarms', 'Swarm create failed', message, 'error');
    } finally {
      setBusyAction(null);
    }
  }

  async function handleRunSwarm() {
    if (!selectedSwarmId || !swarmTask.trim()) {
      return;
    }

    setBusyAction('run-swarm');

    try {
      const result = await requestJson<TaskOutcome>(
        `/api/swarms/${selectedSwarmId}/run`,
        {
          method: 'POST',
          body: { task: swarmTask.trim() },
        }
      );
      const body =
        result.status === 'success'
          ? serializePayload(result.data)
          : result.error ?? 'Swarm run failed.';

      startTransition(() => {
        setSwarmOutputs((current) => ({
          ...current,
          [selectedSwarmId]: body,
        }));
        setSwarmTask('');
      });

      if (!liveEventsConnected) {
        addActivity(
          'swarms',
          `Swarm ${result.status}`,
          body,
          result.status === 'success' ? 'success' : 'error'
        );
        await refreshDashboard('manual');
      }
    } catch (error) {
      const message = getErrorMessage(error);
      setConnectionMessage(message);
      addActivity('swarms', 'Swarm run failed', message, 'error');
    } finally {
      setBusyAction(null);
    }
  }

  async function handleSearchHistory() {
    if (!historyQuery.trim()) {
      return;
    }

    setBusyAction('search-history');

    try {
      const response = await requestJson<SearchResponse>(
        `/api/search?q=${encodeURIComponent(historyQuery.trim())}&limit=8`
      );
      startTransition(() => {
        setHistoryResults(response.results);
      });
      addActivity(
        'search',
        'Task history queried',
        `Found ${String(
          response.results.length
        )} result entries for "${historyQuery.trim()}".`,
        'success'
      );
    } catch (error) {
      const message = getErrorMessage(error);
      setConnectionMessage(message);
      addActivity('search', 'Task history query failed', message, 'error');
    } finally {
      setBusyAction(null);
    }
  }

  async function handleIngestDocument() {
    if (!documentDraft.id.trim() || !documentDraft.text.trim()) {
      setConnectionMessage('Document id and text are required.');
      return;
    }

    setBusyAction('ingest-document');

    try {
      const response = await requestJson<{ id: string; chunks: unknown[] }>(
        '/api/documents',
        {
          method: 'POST',
          body: {
            id: documentDraft.id.trim(),
            text: documentDraft.text.trim(),
          },
        }
      );

      startTransition(() => {
        setDocumentDraft((current) => ({
          ...current,
          text: '',
        }));
      });

      addActivity(
        'search',
        `Document ingested: ${response.id}`,
        `Indexed ${String(
          response.chunks.length
        )} searchable chunks into local memory.`,
        'success'
      );
    } catch (error) {
      const message = getErrorMessage(error);
      setConnectionMessage(message);
      addActivity('search', 'Document ingest failed', message, 'error');
    } finally {
      setBusyAction(null);
    }
  }

  async function handleSearchDocuments() {
    if (!documentQuery.trim()) {
      return;
    }

    setBusyAction('search-documents');

    try {
      const response = await requestJson<SearchResponse>(
        `/api/documents/search?q=${encodeURIComponent(
          documentQuery.trim()
        )}&limit=8`
      );

      startTransition(() => {
        setDocumentResults(response.results);
      });

      addActivity(
        'search',
        'Document search complete',
        `Found ${String(
          response.results.length
        )} matching documents for "${documentQuery.trim()}".`,
        'success'
      );
    } catch (error) {
      const message = getErrorMessage(error);
      setConnectionMessage(message);
      addActivity('search', 'Document search failed', message, 'error');
    } finally {
      setBusyAction(null);
    }
  }

  return (
    <div className={styles.shell}>
      <aside className={styles.sidebar}>
        <div className={styles.brandBlock}>
          <div className={styles.brandKicker}>animaOS // operator surface</div>
          <h1 className={styles.brandTitle}>ANIMAOS CONTROL GRID</h1>
          <p className={styles.brandCopy}>
            Browser cockpit for agents, swarms, knowledge search, and system
            health.
          </p>
        </div>

        <nav className={styles.nav} aria-label="Primary navigation">
          {NAV_ITEMS.map((item) => (
            <button
              key={item.id}
              type="button"
              className={`${styles.navButton} ${
                section === item.id ? styles.navButtonActive : ''
              }`}
              onClick={() => setSection(item.id)}
            >
              <span className={styles.navEyebrow}>{item.eyebrow}</span>
              <span className={styles.navLabel}>{item.label}</span>
              <span className={styles.navDescription}>{item.description}</span>
            </button>
          ))}
        </nav>

        <div className={styles.sidebarFooter}>
          <div className={styles.statusLine}>
            <span className={styles.statusLabel}>SYS</span>
            <span
              className={`${styles.statusPill} ${
                liveEventsConnected ? styles.statusPillReady : ''
              } ${
                connectionState === 'offline' ? styles.statusPillOffline : ''
              }`}
            >
              {connectionState === 'offline'
                ? 'PREVIEW'
                : liveEventsConnected
                ? 'LIVE'
                : connectionState === 'ready'
                ? 'READY'
                : 'SYNC'}
            </span>
          </div>
          <div className={styles.sidebarMetrics}>
            <span>agents {health?.agents ?? agents.length}</span>
            <span>swarms {health?.swarms ?? swarms.length}</span>
            <span>uptime {formatUptime(health?.uptime)}</span>
          </div>
        </div>
      </aside>

      <main className={styles.main}>
        <section className={styles.hero}>
          <div>
            <div className={styles.heroEyebrow}>Realtime command lattice</div>
            <h2 className={styles.heroTitle}>
              Thin-browser operations deck for animaOS Kit
            </h2>
            <p className={styles.heroCopy}>
              Stage agents, compose swarms, inspect memory, and check runtime
              status from one surface. When the server is offline, the browser
              shell stays usable in preview mode.
            </p>
          </div>

          <div className={styles.heroActions}>
            <button
              type="button"
              className={styles.primaryButton}
              onClick={() => void refreshDashboard('manual')}
              disabled={busyAction === 'refresh'}
            >
              refresh all systems
            </button>
            <div className={styles.syncMeta}>
              <span>
                connection{' '}
                {liveEventsConnected ? 'live-streaming' : connectionState}
              </span>
              <span>
                last sync{' '}
                {lastSyncedAt ? formatTimestamp(lastSyncedAt) : 'pending'}
              </span>
            </div>
          </div>
        </section>

        <section className={styles.metricGrid}>
          <article className={styles.metricCard}>
            <span className={styles.metricLabel}>Service status</span>
            <strong className={styles.metricValue}>
              {connectionState === 'ready' ? health?.status ?? 'ok' : 'offline'}
            </strong>
            <span className={styles.metricMeta}>
              {connectionState === 'offline'
                ? 'start the Node server to enable live controls'
                : liveEventsConnected
                ? 'health endpoint and live event stream reachable'
                : 'health endpoint reachable'}
            </span>
          </article>

          <article className={styles.metricCard}>
            <span className={styles.metricLabel}>Agents observed</span>
            <strong className={styles.metricValue}>{agents.length}</strong>
            <span className={styles.metricMeta}>local operator roster</span>
          </article>

          <article className={styles.metricCard}>
            <span className={styles.metricLabel}>Swarms observed</span>
            <strong className={styles.metricValue}>{swarms.length}</strong>
            <span className={styles.metricMeta}>coordinated task surfaces</span>
          </article>

          <article className={styles.metricCard}>
            <span className={styles.metricLabel}>Uptime</span>
            <strong className={styles.metricValue}>
              {formatUptime(health?.uptime)}
            </strong>
            <span className={styles.metricMeta}>reported by /api/health</span>
          </article>
        </section>

        {connectionState === 'offline' ? (
          <section className={styles.ribbon}>
            <strong>Preview mode</strong>
            <span>
              {connectionMessage ??
                'Server endpoints are unavailable. Run bun x nx serve @animaOS-SWARM/server to activate live data.'}
            </span>
          </section>
        ) : null}

        {section === 'agents' ? (
          <section className={styles.workspaceGrid}>
            {Panel({
              eyebrow: 'Agent roster',
              title: 'Create and select operators',
              actions: (
                <span className={styles.inlineMeta}>{agents.length} total</span>
              ),
              children: (
                <>
                  <div className={styles.formGrid}>
                    <label className={styles.field}>
                      <span>Name</span>
                      <input
                        value={agentDraft.name}
                        onChange={(event) =>
                          setAgentDraft((current) => ({
                            ...current,
                            name: event.target.value,
                          }))
                        }
                        placeholder="observer-1"
                      />
                    </label>
                    <label className={styles.field}>
                      <span>Model</span>
                      <input
                        value={agentDraft.model}
                        onChange={(event) =>
                          setAgentDraft((current) => ({
                            ...current,
                            model: event.target.value,
                          }))
                        }
                        placeholder={DEFAULT_AGENT_MODEL}
                      />
                    </label>
                    <button
                      type="button"
                      className={styles.primaryButton}
                      disabled={busyAction === 'create-agent'}
                      onClick={() => void handleCreateAgent()}
                    >
                      create agent
                    </button>
                  </div>

                  <div className={styles.entityList}>
                    {agents.length === 0 ? (
                      <div className={styles.emptyState}>
                        No agents yet. Create one to open a task console.
                      </div>
                    ) : (
                      agents.map((agent) => (
                        <button
                          key={agent.id}
                          type="button"
                          className={`${styles.entityCard} ${
                            selectedAgentId === agent.id
                              ? styles.entityCardActive
                              : ''
                          }`}
                          onClick={() => setSelectedAgentId(agent.id)}
                        >
                          <div className={styles.entityCardHeader}>
                            <strong>{agent.name}</strong>
                            <span className={styles.statusCaps}>
                              {agent.status}
                            </span>
                          </div>
                          <div className={styles.entityCardMeta}>
                            <span>{agent.id}</span>
                            <span>
                              tokens {agent.tokenUsage?.totalTokens ?? 0}
                            </span>
                          </div>
                        </button>
                      ))
                    )}
                  </div>
                </>
              ),
            })}

            {Panel({
              eyebrow: 'Execution console',
              title: selectedAgent
                ? `Task console // ${selectedAgent.name}`
                : 'Task console // select an agent',
              actions: selectedAgent ? (
                <button
                  type="button"
                  className={styles.secondaryButton}
                  disabled={busyAction === 'delete-agent'}
                  onClick={() => void handleDeleteAgent()}
                >
                  delete agent
                </button>
              ) : null,
              children: selectedAgent ? (
                <>
                  <div className={styles.detailHeader}>
                    <div>
                      <span className={styles.detailLabel}>status</span>
                      <strong>{selectedAgent.status}</strong>
                    </div>
                    <div>
                      <span className={styles.detailLabel}>tokens</span>
                      <strong>
                        {selectedAgent.tokenUsage?.totalTokens ?? 0}
                      </strong>
                    </div>
                    <div>
                      <span className={styles.detailLabel}>agent id</span>
                      <strong>{selectedAgent.id}</strong>
                    </div>
                  </div>

                  <label className={styles.field}>
                    <span>Task brief</span>
                    <textarea
                      value={agentTask}
                      onChange={(event) => setAgentTask(event.target.value)}
                      placeholder="Summarize the current launch posture and return the next two operator actions."
                    />
                  </label>
                  <button
                    type="button"
                    className={styles.primaryButton}
                    disabled={
                      busyAction === 'run-agent' ||
                      agentTask.trim().length === 0
                    }
                    onClick={() => void handleRunAgent()}
                  >
                    run selected agent
                  </button>

                  <div className={styles.outputBlock}>
                    <div className={styles.outputHeader}>last output</div>
                    <pre className={styles.outputBody}>
                      {agentOutputs[selectedAgent.id] ??
                        'No task has been run for this agent in the current browser session.'}
                    </pre>
                  </div>

                  <ActivityConsole
                    entries={relevantActivity.slice(0, 6)}
                    emptyLabel="Agent actions will appear here once you create, run, or remove operators."
                  />
                </>
              ) : (
                <div className={styles.emptyState}>
                  Select an agent from the roster to stage work and inspect the
                  latest result.
                </div>
              ),
            })}
          </section>
        ) : null}

        {section === 'swarms' ? (
          <section className={styles.workspaceGrid}>
            {Panel({
              eyebrow: 'Swarm staging',
              title: 'Compose coordination surfaces',
              actions: (
                <span className={styles.inlineMeta}>{swarms.length} total</span>
              ),
              children: (
                <>
                  <div className={styles.formGrid}>
                    <label className={styles.field}>
                      <span>Strategy</span>
                      <select
                        value={swarmDraft.strategy}
                        onChange={(event) =>
                          setSwarmDraft((current) => ({
                            ...current,
                            strategy: event.target.value,
                          }))
                        }
                      >
                        <option value="supervisor">supervisor</option>
                        <option value="dynamic">dynamic</option>
                        <option value="round-robin">round-robin</option>
                      </select>
                    </label>
                    <label className={styles.field}>
                      <span>Manager</span>
                      <input
                        value={swarmDraft.managerName}
                        onChange={(event) =>
                          setSwarmDraft((current) => ({
                            ...current,
                            managerName: event.target.value,
                          }))
                        }
                        placeholder="manager"
                      />
                    </label>
                    <label className={styles.field}>
                      <span>Model</span>
                      <input
                        value={swarmDraft.model}
                        onChange={(event) =>
                          setSwarmDraft((current) => ({
                            ...current,
                            model: event.target.value,
                          }))
                        }
                        placeholder={DEFAULT_AGENT_MODEL}
                      />
                    </label>
                  </div>

                  <label className={styles.field}>
                    <span>Workers</span>
                    <input
                      value={swarmDraft.workers}
                      onChange={(event) =>
                        setSwarmDraft((current) => ({
                          ...current,
                          workers: event.target.value,
                        }))
                      }
                      placeholder="researcher, operator"
                    />
                  </label>
                  <button
                    type="button"
                    className={styles.primaryButton}
                    disabled={busyAction === 'create-swarm'}
                    onClick={() => void handleCreateSwarm()}
                  >
                    create swarm
                  </button>

                  <div className={styles.entityList}>
                    {swarms.length === 0 ? (
                      <div className={styles.emptyState}>
                        No swarms created yet. Stage one to test a coordination
                        strategy.
                      </div>
                    ) : (
                      swarms.map((swarm) => (
                        <button
                          key={swarm.id}
                          type="button"
                          className={`${styles.entityCard} ${
                            selectedSwarmId === swarm.id
                              ? styles.entityCardActive
                              : ''
                          }`}
                          onClick={() => setSelectedSwarmId(swarm.id)}
                        >
                          <div className={styles.entityCardHeader}>
                            <strong>{swarm.id}</strong>
                            <span className={styles.statusCaps}>
                              {swarm.status}
                            </span>
                          </div>
                          <div className={styles.entityCardMeta}>
                            <span>agents {swarm.agentIds?.length ?? 0}</span>
                            <span>results {swarm.results?.length ?? 0}</span>
                          </div>
                        </button>
                      ))
                    )}
                  </div>
                </>
              ),
            })}

            {Panel({
              eyebrow: 'Swarm console',
              title: selectedSwarm
                ? `Mission console // ${selectedSwarm.id}`
                : 'Mission console // select a swarm',
              children: selectedSwarm ? (
                <>
                  <div className={styles.detailHeader}>
                    <div>
                      <span className={styles.detailLabel}>status</span>
                      <strong>{selectedSwarm.status}</strong>
                    </div>
                    <div>
                      <span className={styles.detailLabel}>agents</span>
                      <strong>{selectedSwarm.agentIds?.length ?? 0}</strong>
                    </div>
                    <div>
                      <span className={styles.detailLabel}>results</span>
                      <strong>{selectedSwarm.results?.length ?? 0}</strong>
                    </div>
                  </div>

                  <label className={styles.field}>
                    <span>Swarm task</span>
                    <textarea
                      value={swarmTask}
                      onChange={(event) => setSwarmTask(event.target.value)}
                      placeholder="Break down the next milestone into delegated work and return a rollout plan."
                    />
                  </label>
                  <button
                    type="button"
                    className={styles.primaryButton}
                    disabled={
                      busyAction === 'run-swarm' ||
                      swarmTask.trim().length === 0
                    }
                    onClick={() => void handleRunSwarm()}
                  >
                    run selected swarm
                  </button>

                  <div className={styles.outputBlock}>
                    <div className={styles.outputHeader}>last swarm output</div>
                    <pre className={styles.outputBody}>
                      {swarmOutputs[selectedSwarm.id] ??
                        'No swarm execution has been triggered in the current browser session.'}
                    </pre>
                  </div>

                  <ActivityConsole
                    entries={relevantActivity.slice(0, 6)}
                    emptyLabel="Swarm actions will appear here when you stage or run coordinated work."
                  />
                </>
              ) : (
                <div className={styles.emptyState}>
                  Select a swarm to run a coordinated task and inspect the most
                  recent output.
                </div>
              ),
            })}
          </section>
        ) : null}

        {section === 'search' ? (
          <section className={styles.workspaceGrid}>
            {Panel({
              eyebrow: 'Task history',
              title: 'Search recent execution memory',
              children: (
                <>
                  <label className={styles.field}>
                    <span>History query</span>
                    <input
                      value={historyQuery}
                      onChange={(event) => setHistoryQuery(event.target.value)}
                      placeholder="launch"
                    />
                  </label>
                  <button
                    type="button"
                    className={styles.primaryButton}
                    disabled={busyAction === 'search-history'}
                    onClick={() => void handleSearchHistory()}
                  >
                    search task history
                  </button>

                  <div className={styles.resultList}>
                    {historyResults.length === 0 ? (
                      <div className={styles.emptyState}>
                        Query task history to inspect recent operator output.
                      </div>
                    ) : (
                      historyResults.map((result, index) => {
                        const card = formatTaskHistoryResult(result, index);

                        return (
                          <article
                            key={`${card.key}-${String(index)}`}
                            className={styles.resultCard}
                          >
                            <div className={styles.resultMeta}>
                              <strong>{card.label}</strong>
                              {typeof result.score === 'number' ? (
                                <span>score {result.score.toFixed(2)}</span>
                              ) : null}
                            </div>
                            <div className={styles.resultTitle}>
                              {card.title}
                            </div>
                            <p className={styles.resultCopy}>{card.preview}</p>
                          </article>
                        );
                      })
                    )}
                  </div>
                </>
              ),
            })}

            {Panel({
              eyebrow: 'Documents',
              title: 'Ingest and query local knowledge',
              children: (
                <>
                  <div className={styles.formGrid}>
                    <label className={styles.field}>
                      <span>Document id</span>
                      <input
                        value={documentDraft.id}
                        onChange={(event) =>
                          setDocumentDraft((current) => ({
                            ...current,
                            id: event.target.value,
                          }))
                        }
                        placeholder="ops-playbook"
                      />
                    </label>
                    <label className={styles.field}>
                      <span>Document query</span>
                      <input
                        value={documentQuery}
                        onChange={(event) =>
                          setDocumentQuery(event.target.value)
                        }
                        placeholder="operator"
                      />
                    </label>
                  </div>

                  <label className={styles.field}>
                    <span>Document text</span>
                    <textarea
                      value={documentDraft.text}
                      onChange={(event) =>
                        setDocumentDraft((current) => ({
                          ...current,
                          text: event.target.value,
                        }))
                      }
                      placeholder="Paste operating guidance, runbooks, or policy notes here."
                    />
                  </label>

                  <div className={styles.buttonRow}>
                    <button
                      type="button"
                      className={styles.primaryButton}
                      disabled={busyAction === 'ingest-document'}
                      onClick={() => void handleIngestDocument()}
                    >
                      ingest document
                    </button>
                    <button
                      type="button"
                      className={styles.secondaryButton}
                      disabled={busyAction === 'search-documents'}
                      onClick={() => void handleSearchDocuments()}
                    >
                      search documents
                    </button>
                  </div>

                  <div className={styles.resultList}>
                    {documentResults.length === 0 ? (
                      <div className={styles.emptyState}>
                        Search indexed documents to inspect local knowledge.
                      </div>
                    ) : (
                      documentResults.map((result, index) => {
                        const card = formatDocumentResult(result, index);

                        return (
                          <article
                            key={`${card.key}-${String(index)}`}
                            className={styles.resultCard}
                          >
                            <div className={styles.resultMeta}>
                              <strong>{card.label}</strong>
                              {typeof result.score === 'number' ? (
                                <span>score {result.score.toFixed(2)}</span>
                              ) : null}
                            </div>
                            <div className={styles.resultTitle}>
                              {card.title}
                            </div>
                            <p className={styles.resultCopy}>{card.preview}</p>
                          </article>
                        );
                      })
                    )}
                  </div>
                </>
              ),
            })}
          </section>
        ) : null}

        {section === 'health' ? (
          <section className={styles.workspaceGrid}>
            {Panel({
              eyebrow: 'Service telemetry',
              title: 'Runtime health snapshot',
              children: (
                <div className={styles.healthStack}>
                  <div className={styles.detailHeader}>
                    <div>
                      <span className={styles.detailLabel}>service</span>
                      <strong>{health?.status ?? 'offline'}</strong>
                    </div>
                    <div>
                      <span className={styles.detailLabel}>agents</span>
                      <strong>{health?.agents ?? agents.length}</strong>
                    </div>
                    <div>
                      <span className={styles.detailLabel}>swarms</span>
                      <strong>{health?.swarms ?? swarms.length}</strong>
                    </div>
                    <div>
                      <span className={styles.detailLabel}>uptime</span>
                      <strong>{formatUptime(health?.uptime)}</strong>
                    </div>
                  </div>

                  <div className={styles.endpointList}>
                    <article className={styles.endpointCard}>
                      <strong>/api/health</strong>
                      <span>service pulse and aggregate counts</span>
                    </article>
                    <article className={styles.endpointCard}>
                      <strong>/api/agents</strong>
                      <span>operator roster, create, run, delete</span>
                    </article>
                    <article className={styles.endpointCard}>
                      <strong>/api/swarms</strong>
                      <span>coordination roster, create, execute</span>
                    </article>
                    <article className={styles.endpointCard}>
                      <strong>/api/search + /api/documents</strong>
                      <span>task-memory and document knowledge search</span>
                    </article>
                    <article className={styles.endpointCard}>
                      <strong>/ws</strong>
                      <span>
                        live runtime event stream for browser operators
                      </span>
                    </article>
                  </div>
                </div>
              ),
            })}

            {Panel({
              eyebrow: 'System log',
              title: 'Operator activity feed',
              children: (
                <ActivityConsole
                  entries={relevantActivity.slice(0, 8)}
                  emptyLabel="Health and operator events will accumulate here as you work in the dashboard."
                />
              ),
            })}
          </section>
        ) : null}
      </main>
    </div>
  );
}

export default App;
