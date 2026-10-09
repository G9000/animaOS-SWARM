import { StatusCard } from '../components/system/StatusCard';
import { useStatus } from '../hooks/useStatus';
import { COMPANION_UNREACHABLE } from '../lib/approvals';
import { HEALTH_LOADING, HEALTH_TOO_OLD, healthCards } from '../lib/status';

export interface HealthPageProps {
  online: boolean;
  /** `LiveState.epoch`: bumped by every snapshot and resync. */
  epoch: number;
}

/** Spec §15.4: whether the daemon is healthy. Daemon text renders as text. */
export function HealthPage({ online, epoch }: HealthPageProps) {
  const view = useStatus({ enabled: online, epoch });

  if (!online) {
    return (
      <div className="system-page">
        <p className="system-note" role="status">
          {COMPANION_UNREACHABLE}
        </p>
      </div>
    );
  }

  const gone = view.errorStatus === 404;
  if (gone) {
    return (
      <div className="system-page">
        <p className="system-note" role="status">
          {HEALTH_TOO_OLD}
        </p>
      </div>
    );
  }

  return (
    <div className="system-page">
      <div className="system-header">
        <h2>How the daemon is doing</h2>
        <button
          type="button"
          className="studio-tool-button"
          onClick={view.refresh}
        >
          Refresh
        </button>
      </div>
      {view.error && (
        <p className="system-error" role="alert">
          {view.error}
        </p>
      )}
      {!view.loaded && !view.error && (
        <p className="system-note" role="status">
          {HEALTH_LOADING}
        </p>
      )}
      {view.status && (
        <div className="system-cards">
          {healthCards(view.status).map((card) => (
            <StatusCard key={card.id} card={card} />
          ))}
        </div>
      )}
    </div>
  );
}
