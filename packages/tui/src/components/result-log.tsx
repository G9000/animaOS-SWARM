import React from 'react';
import { Box, Text } from 'ink';

export interface ResultEntry {
  id: string;
  timestamp: number;
  task: string;
  result: string;
  isError: boolean;
  label?: string;
}

export interface ResultLogProps {
  results: ResultEntry[];
}

export function ResultLog({
  results,
}: ResultLogProps): React.ReactElement | null {
  if (results.length === 0) return null;

  const recent = results.slice(-3);

  function formatTime(timestamp: number): string {
    return new Date(timestamp).toLocaleTimeString(undefined, {
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
    });
  }

  return (
    <Box flexDirection="column" borderStyle="single" paddingX={1}>
      <Text bold color="white">
        Past runs
      </Text>
      <Text dimColor>/history browse all /retry rerun last</Text>
      {recent.map((r) => {
        const label = r.label?.trim();
        const task = r.task.length > 55 ? r.task.slice(0, 52) + '...' : r.task;
        const result =
          r.result.length > 200 ? r.result.slice(0, 197) + '...' : r.result;
        return (
          <Box key={r.id} flexDirection="column" marginTop={1}>
            <Text color={r.isError ? 'red' : 'green'}>
              {r.isError ? '✗' : '✓'} {label ?? task}
            </Text>
            <Text color="gray"> {formatTime(r.timestamp)}</Text>
            {label ? (
              <Text color="gray" wrap="wrap">
                {'  '}task: {task}
              </Text>
            ) : null}
            <Text color="gray" wrap="wrap">
              {'  '}
              {result}
            </Text>
          </Box>
        );
      })}
    </Box>
  );
}
