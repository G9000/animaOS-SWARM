import {
  useLayoutEffect,
  useRef,
  useState,
  type ChangeEvent,
  type FormEvent,
} from 'react';
import { LOG_LEVELS, type LogLevel } from '@animaOS-SWARM/sdk';

import { LogLine } from '../components/system/LogLine';
import { useLogs } from '../hooks/useLogs';
import { COMPANION_UNREACHABLE } from '../lib/approvals';
import {
  LOGS_CONNECTING,
  LOGS_COPIED,
  LOGS_COPY_FAILED,
  LOGS_EMPTY,
  LOGS_PAUSED_NOTE,
  LOGS_RECONNECTING,
  LOGS_TOO_OLD,
  LOG_LEVEL_LABELS,
  logsAsText,
} from '../lib/logs';

export interface LogsPageProps {
  online: boolean;
}

type LevelChoice = LogLevel | 'all';
type CopyState = 'copied' | 'failed' | null;

/** The log region counts as "at the bottom" within this many pixels. */
const STICK_SLACK_PX = 24;

const heldNote = (count: number) =>
  `${count} new ${count === 1 ? 'line' : 'lines'} held`;

/** Spec §15.4: what the daemon is saying. Every line renders as text. */
export function LogsPage({ online }: LogsPageProps) {
  const [level, setLevel] = useState<LevelChoice>('all');
  const [draft, setDraft] = useState('');
  const [query, setQuery] = useState('');
  const [paused, setPaused] = useState(false);
  const [copy, setCopy] = useState<CopyState>(null);
  const view = useLogs({
    enabled: online,
    level: level === 'all' ? null : level,
    query,
    paused,
  });
  const region = useRef<HTMLDivElement>(null);
  const stuck = useRef(true);

  // Follow the newest line until the owner scrolls up.
  useLayoutEffect(() => {
    const element = region.current;
    if (element && stuck.current && !paused) {
      element.scrollTop = element.scrollHeight;
    }
  }, [view.lines, paused]);

  if (!online) {
    return (
      <div className="system-page">
        <p className="system-note" role="status">
          {COMPANION_UNREACHABLE}
        </p>
      </div>
    );
  }

  if (view.errorStatus === 404) {
    return (
      <div className="system-page">
        <p className="system-note" role="status">
          {LOGS_TOO_OLD}
        </p>
      </div>
    );
  }

  const chooseLevel = (event: ChangeEvent<HTMLSelectElement>) => {
    setLevel(event.target.value as LevelChoice);
    setCopy(null);
  };
  const submitSearch = (event: FormEvent) => {
    event.preventDefault();
    setQuery(draft.trim());
    setCopy(null);
  };
  const copyLines = async () => {
    try {
      await navigator.clipboard.writeText(logsAsText(view.lines));
      setCopy('copied');
    } catch {
      setCopy('failed');
    }
  };
  const onScroll = () => {
    const element = region.current;
    if (!element) return;
    stuck.current =
      element.scrollHeight - element.scrollTop - element.clientHeight <=
      STICK_SLACK_PX;
  };

  return (
    <div className="system-page system-logs-page">
      <div className="system-header">
        <h2>What the daemon is saying</h2>
      </div>
      <div className="system-toolbar">
        <label className="system-field">
          <span>Show at least</span>
          <select value={level} onChange={chooseLevel}>
            <option value="all">All</option>
            {LOG_LEVELS.map((option) => (
              <option key={option} value={option}>
                {LOG_LEVEL_LABELS[option]}
              </option>
            ))}
          </select>
        </label>
        <form className="system-search" role="search" onSubmit={submitSearch}>
          <input
            type="search"
            aria-label="Search logs"
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
          />
          <button type="submit" className="studio-tool-button">
            Search
          </button>
        </form>
        <button
          type="button"
          className="studio-tool-button"
          aria-pressed={paused}
          onClick={() => setPaused((value) => !value)}
        >
          {paused ? 'Resume' : 'Pause'}
        </button>
        {paused && view.held > 0 && (
          <span className="system-note">{heldNote(view.held)}</span>
        )}
        <button
          type="button"
          className="studio-tool-button"
          disabled={view.lines.length === 0}
          onClick={() => void copyLines()}
        >
          Copy
        </button>
        <span className="system-note" role="status">
          {copy === 'copied'
            ? LOGS_COPIED
            : copy === 'failed'
              ? LOGS_COPY_FAILED
              : ''}
        </span>
      </div>
      {paused && <p className="system-note">{LOGS_PAUSED_NOTE}</p>}
      {view.error && (
        <p className="system-error" role="alert">
          {view.error}
        </p>
      )}
      {!view.connected && (
        <p className="system-note" role="status">
          {view.loaded ? LOGS_RECONNECTING : LOGS_CONNECTING}
        </p>
      )}
      {view.loaded && view.lines.length === 0 && (
        <p className="system-note">{LOGS_EMPTY}</p>
      )}
      <div
        ref={region}
        className="system-log"
        role="log"
        aria-label="Daemon logs"
        aria-live="off"
        tabIndex={0}
        onScroll={onScroll}
      >
        {view.lines.map((line) => (
          <LogLine key={line.seq} line={line} />
        ))}
      </div>
    </div>
  );
}
