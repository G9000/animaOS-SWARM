import React, { useState, useEffect, useRef } from 'react';
import { Box, Text } from 'ink';
import { maybeColor, useColorEnabled } from '../colors.js';
import type { SwarmStats } from '../types.js';

export interface StatusBarProps {
  stats: SwarmStats;
  done: boolean;
  configuredAgentCount?: number;
  daemonStatus?: 'up' | 'down';
  daemonCheckedAt?: number | null;
  /** Override the ambient `ColorContext`. When omitted, the context value
   * (which defaults to `!process.env.NO_COLOR`) is used. */
  colorEnabled?: boolean;
}

function formatDaemonCheckTime(timestamp: number): string {
  return `${new Date(timestamp).toISOString().slice(11, 19)}Z`;
}

export function StatusBar({
  stats,
  done,
  configuredAgentCount,
  daemonStatus,
  daemonCheckedAt,
  colorEnabled: colorEnabledProp,
}: StatusBarProps): React.ReactElement {
  const ambient = useColorEnabled();
  const colorEnabled = colorEnabledProp ?? ambient;
  // Local ticker so the elapsed display advances between unrelated state
  // changes. The hook-level ticker in useEventLog covers consumers that read
  // `stats.elapsed` directly; this one keeps the bar live even when the hook
  // is mocked away (tests) or the consumer doesn't propagate stat changes.
  const [elapsed, setElapsed] = useState(stats.elapsed);
  const startRef = useRef(Date.now());
  // Reset the local clock when stats.elapsed jumps backwards (new task) so
  // the bar doesn't show time accumulated from an earlier run.
  const lastStatsElapsedRef = useRef(stats.elapsed);

  useEffect(() => {
    if (stats.elapsed < lastStatsElapsedRef.current) {
      startRef.current = Date.now();
      setElapsed(stats.elapsed);
    }
    lastStatsElapsedRef.current = stats.elapsed;
  }, [stats.elapsed]);

  useEffect(() => {
    if (done) return;

    const interval = setInterval(() => {
      setElapsed(Math.floor((Date.now() - startRef.current) / 1000));
    }, 1000);

    return () => {
      clearInterval(interval);
    };
  }, [done]);

  const agentLabel =
    typeof configuredAgentCount === 'number' &&
    configuredAgentCount !== stats.agentCount
      ? `${configuredAgentCount} cfg / ${stats.agentCount} active`
      : String(stats.agentCount);

  return (
    <Box borderStyle="single" paddingX={1} justifyContent="space-between">
      <Text>tokens: {stats.totalTokens}</Text>
      <Text>elapsed: {elapsed}s</Text>
      <Text>agents: {agentLabel}</Text>
      {stats.laggedEventCount > 0 ? (
        <Text color={maybeColor(colorEnabled, 'yellow')}>
          gaps: {stats.laggedEventCount}
        </Text>
      ) : null}
      {daemonStatus ? (
        <Text
          color={maybeColor(
            colorEnabled,
            daemonStatus === 'up' ? 'green' : 'red'
          )}
        >
          daemon: {daemonStatus}
          {daemonCheckedAt
            ? ` @ ${formatDaemonCheckTime(daemonCheckedAt)}`
            : ''}
        </Text>
      ) : null}
    </Box>
  );
}
