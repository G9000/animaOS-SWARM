import { useEffect, useState } from 'react';
import {
  health,
  agents,
  swarms,
  type AgentSnapshot,
  type SwarmState,
  type HealthResponse,
} from '../lib/api';
import styles from './app.module.css';

function usePolled<T>(
  fetcher: () => Promise<T>,
  intervalMs = 4000
): { data: T | null; error: string | null; loading: boolean } {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    const run = async () => {
      try {
        const result = await fetcher();
        if (!cancelled) { setData(result); setError(null); }
      } catch (e) {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      } finally {
        if (!cancelled) setLoading(false);
      }
    };
    run();
    const id = setInterval(run, intervalMs);
    return () => { cancelled = true; clearInterval(id); };
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return { data, error, loading };
}

function StatusDot({ status }: { status: string }) {
  const cls =
    status === 'ok' || status === 'idle' || status === 'completed'
      ? styles.dotGreen
      : status === 'running'
      ? styles.dotYellow
      : styles.dotRed;
  return <span className={`${styles.dot} ${cls}`} />;
}

function HealthBadge({ h, error }: { h: HealthResponse | null; error: string | null }) {
  if (error) return <span className={styles.badge} data-status="err">daemon offline</span>;
  if (!h) return <span className={styles.badge} data-status="pending">—</span>;
  const parts = [h.status];
  if (h.version) parts.push(`v${h.version}`);
  if (h.uptime_secs !== undefined) parts.push(`up ${Math.round(h.uptime_secs / 60)}m`);
  return (
    <span className={styles.badge} data-status="ok">
      <StatusDot status="ok" />
      {parts.join(' · ')}
    </span>
  );
}

function AgentsPanel() {
  const { data, error, loading } = usePolled(() => agents.list());
  return (
    <section className={styles.panel}>
      <h2 className={styles.panelTitle}>agents</h2>
      {loading && <p className={styles.muted}>loading…</p>}
      {error && <p className={styles.err}>{error}</p>}
      {data && data.length === 0 && <p className={styles.muted}>no agents</p>}
      {data && data.length > 0 && (
        <table className={styles.table}>
          <thead>
            <tr>
              <th>name</th>
              <th>status</th>
              <th>tokens</th>
              <th>id</th>
            </tr>
          </thead>
          <tbody>
            {data.map((a: AgentSnapshot) => (
              <tr key={a.state.id}>
                <td>{a.state.name}</td>
                <td>
                  <StatusDot status={a.state.status} />
                  {a.state.status}
                </td>
                <td>{a.state.tokenUsage.totalTokens.toLocaleString()}</td>
                <td className={styles.mono}>{a.state.id.slice(0, 8)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}

function SwarmsPanel() {
  const { data, error, loading } = usePolled(() => swarms.list());
  return (
    <section className={styles.panel}>
      <h2 className={styles.panelTitle}>swarms</h2>
      {loading && <p className={styles.muted}>loading…</p>}
      {error && <p className={styles.err}>{error}</p>}
      {data && data.length === 0 && <p className={styles.muted}>no swarms</p>}
      {data && data.length > 0 && (
        <table className={styles.table}>
          <thead>
            <tr>
              <th>id</th>
              <th>status</th>
              <th>agents</th>
              <th>tokens</th>
            </tr>
          </thead>
          <tbody>
            {data.map((s: SwarmState) => (
              <tr key={s.id}>
                <td className={styles.mono}>{s.id.slice(0, 8)}</td>
                <td>
                  <StatusDot status={s.status} />
                  {s.status}
                </td>
                <td>{s.agentIds.length}</td>
                <td>{s.tokenUsage.totalTokens.toLocaleString()}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}

export function App() {
  const { data: h, error: hErr } = usePolled(() => health.get(), 8000);

  return (
    <div className={styles.shell}>
      <header className={styles.header}>
        <span className={styles.kicker}>animaOS</span>
        <h1 className={styles.title}>Control Grid</h1>
        <div className={styles.headerRight}>
          <HealthBadge h={h} error={hErr} />
        </div>
      </header>

      <main className={styles.main}>
        <AgentsPanel />
        <SwarmsPanel />
      </main>
    </div>
  );
}

export default App;
