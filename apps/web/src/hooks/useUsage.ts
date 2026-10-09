import { useCallback, useEffect, useRef, useState } from 'react';
import type {
  UsageGroup,
  UsageGroupBy,
  UsageSummary,
  UsageTotals,
} from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';
import { downloadText } from '../lib/download';
import {
  USAGE_EXPORT_FAILED,
  fillDays,
  usageCsvFilename,
  usageErrorMessage,
  usageRange,
  type UsageRange,
  type UsageRangeDays,
} from '../lib/usage';

export interface UsageOptions {
  /** False while the daemon is offline: nothing is read. */
  enabled: boolean;
  days: UsageRangeDays;
  agentId: string | null;
  /** `LiveState.epoch`: bumped by every snapshot and resync. */
  epoch: number;
  /** The clock; tests fix it. */
  now?: () => Date;
}

export interface UsageView {
  range: UsageRange;
  /** Every day of the range, ascending. */
  days: UsageGroup[];
  models: UsageGroup[];
  sources: UsageGroup[];
  sessions: UsageGroup[];
  totals: UsageTotals | null;
  /** The range's last day. */
  today: UsageTotals | null;
  truncated: boolean;
  loaded: boolean;
  error: string | null;
  /** The HTTP status of the failed read, when the daemon refused it. */
  errorStatus: number | null;
  refresh: () => void;
  /** Reads the range's CSV and downloads it; true when it did. */
  exportCsv: () => Promise<boolean>;
}

const GROUPS: readonly UsageGroupBy[] = ['day', 'model', 'source', 'session'];

/** A list that did not change keeps its array, so a harness that goes quiet
 *  does not render again (the empty reload included). */
function keepSame<T>(previous: T, next: T): T {
  return JSON.stringify(previous) === JSON.stringify(next) ? previous : next;
}

function rangeFor(days: number, now: () => Date): UsageRange {
  return usageRange(days, now());
}

const NO_GROUPS: UsageGroup[] = [];

/** The Usage page's data (spec §15.4): four summaries over the range. */
export function useUsage({
  enabled,
  days,
  agentId,
  epoch,
  now = () => new Date(),
}: UsageOptions): UsageView {
  const clock = useRef(now);
  clock.current = now;
  const [range, setRange] = useState(() => rangeFor(days, now));
  const [dayList, setDayList] = useState<UsageGroup[]>(NO_GROUPS);
  const [models, setModels] = useState<UsageGroup[]>(NO_GROUPS);
  const [sources, setSources] = useState<UsageGroup[]>(NO_GROUPS);
  const [sessions, setSessions] = useState<UsageGroup[]>(NO_GROUPS);
  const [totals, setTotals] = useState<UsageTotals | null>(null);
  const [truncated, setTruncated] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [readError, setReadError] = useState<{
    message: string;
    status: number | null;
  } | null>(null);
  const [exportError, setExportError] = useState<string | null>(null);
  // The newest read wins; a late answer to an older one is dropped.
  const sequence = useRef(0);
  const unmounted = useRef(false);
  const rangeRef = useRef(range);
  const agentRef = useRef(agentId);
  agentRef.current = agentId;

  const load = useCallback(async () => {
    if (!agentId) return;
    const mine = ++sequence.current;
    const next = rangeFor(days, clock.current);
    const query = {
      from: next.from,
      to: next.to,
      agentId,
      tzOffsetMinutes: next.tzOffsetMinutes,
    };
    const settled = await Promise.allSettled(
      GROUPS.map((groupBy) => daemon.usageSummary({ ...query, groupBy })),
    );
    if (unmounted.current || mine !== sequence.current) return;

    const [day, model, source, session] = settled;
    const value = (result: PromiseSettledResult<UsageSummary>) =>
      result.status === 'fulfilled' ? result.value : null;
    const failure = settled.find((result) => result.status === 'rejected');

    rangeRef.current = next;
    setRange((previous) => keepSame(previous, next));
    const daySummary = value(day);
    if (daySummary) {
      setDayList((previous) =>
        keepSame(previous, fillDays(daySummary.groups, next)),
      );
      setTotals((previous) => keepSame(previous, daySummary.totals));
    }
    const modelSummary = value(model);
    if (modelSummary)
      setModels((previous) => keepSame(previous, modelSummary.groups));
    const sourceSummary = value(source);
    if (sourceSummary)
      setSources((previous) => keepSame(previous, sourceSummary.groups));
    const sessionSummary = value(session);
    if (sessionSummary)
      setSessions((previous) => keepSame(previous, sessionSummary.groups));
    const answered = [
      daySummary,
      modelSummary,
      sourceSummary,
      sessionSummary,
    ].filter((item): item is UsageSummary => item !== null);
    if (answered.length > 0)
      setTruncated(answered.some((item) => item.truncated));
    setReadError(
      failure && failure.status === 'rejected'
        ? usageErrorMessage(failure.reason)
        : null,
    );
    setLoaded(true);
  }, [agentId, days]);

  useEffect(() => {
    unmounted.current = false;
    return () => {
      unmounted.current = true;
    };
  }, []);

  useEffect(() => {
    if (!enabled || !agentId) return;
    setExportError(null);
    void load();
  }, [enabled, agentId, days, epoch, load]);

  const refresh = useCallback(() => {
    setExportError(null);
    void load();
  }, [load]);

  const exportCsv = useCallback(async () => {
    const owner = agentRef.current;
    if (!owner) return false;
    const { from, to } = rangeRef.current;
    setExportError(null);
    try {
      const text = await daemon.exportUsageCsv({ from, to, agentId: owner });
      downloadText(usageCsvFilename({ from, to }), text, 'text/csv');
      return true;
    } catch {
      if (!unmounted.current) setExportError(USAGE_EXPORT_FAILED);
      return false;
    }
  }, []);

  return {
    range,
    days: dayList,
    models,
    sources,
    sessions,
    totals,
    today: dayList.length > 0 ? dayList[dayList.length - 1].totals : null,
    truncated,
    loaded,
    error: exportError ?? readError?.message ?? null,
    errorStatus: exportError ? null : (readError?.status ?? null),
    refresh,
    exportCsv,
  };
}
