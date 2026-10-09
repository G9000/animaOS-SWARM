import { useState, type ReactNode } from 'react';

import { UsageChart } from '../components/usage/UsageChart';
import { UsageTable } from '../components/usage/UsageTable';
import { useSessionUsage } from '../hooks/useSessionUsage';
import { useUsage } from '../hooks/useUsage';
import { COMPANION_UNREACHABLE } from '../lib/approvals';
import {
  DEFAULT_USAGE_RANGE_DAYS,
  USAGE_EMPTY,
  USAGE_RANGES_DAYS,
  USAGE_TRUNCATED_NOTE,
  costNote,
  formatCost,
  formatTokens,
  sourceLabel,
  subscriptionNote,
  type UsageRangeDays,
} from '../lib/usage';

export interface UsagePageProps {
  agentId: string | null;
  online: boolean;
  /** `LiveState.epoch`: bumped by every snapshot and resync. */
  epoch: number;
  /** The session last viewed, for the This chat card. */
  sessionId: string | null;
}

const RANGE_STORAGE_KEY = 'anima.usage.range';
const LOADING_TEXT = 'Loading usage…';
const NO_SESSION_LABEL = 'No chat';

function readRange(): UsageRangeDays {
  try {
    const stored = Number(sessionStorage.getItem(RANGE_STORAGE_KEY));
    const found = USAGE_RANGES_DAYS.find((days) => days === stored);
    if (found) return found;
  } catch {
    // Storage can be blocked; the page works without it.
  }
  return DEFAULT_USAGE_RANGE_DAYS;
}

function rememberRange(days: UsageRangeDays) {
  try {
    sessionStorage.setItem(RANGE_STORAGE_KEY, String(days));
  } catch {
    // Not remembered; nothing else depends on it.
  }
}

function Card({
  label,
  value,
  children,
}: {
  label: string;
  value: string;
  children?: ReactNode;
}) {
  return (
    <div className="usage-card" role="group" aria-label={label}>
      <span className="usage-card-label">{label}</span>
      <strong className="usage-card-value">{value}</strong>
      {children}
    </div>
  );
}

/** Spec §15.4: what the companion costs. Model, provider, and session names
 *  render as text. */
export function UsagePage({
  agentId,
  online,
  epoch,
  sessionId,
}: UsagePageProps) {
  const [days, setDays] = useState<UsageRangeDays>(readRange);
  const [exporting, setExporting] = useState(false);
  const view = useUsage({ enabled: online, days, agentId, epoch });
  const chat = useSessionUsage(
    online && agentId && sessionId ? { agentId, sessionId } : null,
    epoch,
  );

  if (!online) {
    return (
      <div className="usage-page">
        <p className="usage-note" role="status">
          {COMPANION_UNREACHABLE}
        </p>
      </div>
    );
  }

  const chooseRange = (next: UsageRangeDays) => {
    setDays(next);
    rememberRange(next);
  };
  const exportCsv = async () => {
    setExporting(true);
    try {
      await view.exportCsv();
    } finally {
      setExporting(false);
    }
  };

  const { totals, today } = view;
  const gone = view.errorStatus === 404;
  const empty = totals !== null && totals.calls === 0;

  return (
    <div className="usage-page">
      <div className="usage-header">
        <h2>What your companion costs</h2>
        <div className="usage-ranges" role="group" aria-label="Range">
          {USAGE_RANGES_DAYS.map((option) => (
            <button
              key={option}
              type="button"
              className="studio-tool-button"
              aria-pressed={days === option}
              onClick={() => chooseRange(option)}
            >
              {option} days
            </button>
          ))}
        </div>
        {!gone && (
          <button
            type="button"
            className="studio-tool-button"
            disabled={exporting || !view.loaded}
            onClick={() => void exportCsv()}
          >
            Export CSV
          </button>
        )}
      </div>
      {view.error && (
        <div className="usage-header">
          <p className="usage-error" role="alert">
            {view.error}
          </p>
          <button
            type="button"
            className="studio-tool-button"
            onClick={view.refresh}
          >
            Refresh
          </button>
        </div>
      )}
      {!view.loaded && !view.error && (
        <p className="usage-note" role="status">
          {LOADING_TEXT}
        </p>
      )}
      {!gone && totals && (
        <>
          <div className="usage-cards">
            <Card label="Total tokens" value={formatTokens(totals.totalTokens)}>
              <small>
                {formatTokens(totals.promptTokens)} prompt ·{' '}
                {formatTokens(totals.completionTokens)} completion
              </small>
            </Card>
            <Card label="Cost" value={formatCost(totals.costMicros)}>
              {costNote(totals) && <small>{costNote(totals)}</small>}
              {subscriptionNote(totals) && (
                <small>{subscriptionNote(totals)}</small>
              )}
            </Card>
            <Card label="Calls" value={String(totals.calls)} />
            {today && (
              <Card label="Today" value={formatTokens(today.totalTokens)}>
                <small>{formatCost(today.costMicros)}</small>
              </Card>
            )}
            {sessionId && chat && (
              <Card label="This chat" value={formatTokens(chat.totalTokens)}>
                <small>{formatCost(chat.costMicros)}</small>
              </Card>
            )}
          </div>
          {view.truncated && (
            <p className="usage-note" role="status">
              {USAGE_TRUNCATED_NOTE}
            </p>
          )}
          {empty ? (
            <p className="usage-note">{USAGE_EMPTY}</p>
          ) : (
            <>
              <section className="usage-chart-section" aria-label="Per day">
                <h3>Per day</h3>
                <UsageChart days={view.days} rangeDays={days} />
              </section>
              <UsageTable
                title="By model"
                keyHeading="Model"
                rows={view.models}
              />
              <UsageTable
                title="By source"
                keyHeading="Source"
                rows={view.sources}
                renderKey={sourceLabel}
              />
              <UsageTable
                title="Top chats"
                keyHeading="Chat"
                rows={view.sessions}
                renderKey={(key) =>
                  key === '' ? (
                    NO_SESSION_LABEL
                  ) : (
                    <a href={`#/s/${encodeURIComponent(key)}`}>{key}</a>
                  )
                }
              />
            </>
          )}
        </>
      )}
    </div>
  );
}
