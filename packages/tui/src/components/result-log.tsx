import React from 'react';
import { Box, Text } from 'ink';

export interface ResultEntry {
  task: string;
  result: string;
  isError: boolean;
}

export interface ResultLogProps {
  results: ResultEntry[];
}

export function ResultLog({
  results,
}: ResultLogProps): React.ReactElement | null {
  if (results.length === 0) return null;

  const recent = results.slice(-3);

  return (
    <Box flexDirection="column" borderStyle="single" paddingX={1}>
      <Text bold color="white">
        Past runs
      </Text>
      {recent.map((r) => {
        const task = r.task.length > 55 ? r.task.slice(0, 52) + '...' : r.task;
        const result =
          r.result.length > 200 ? r.result.slice(0, 197) + '...' : r.result;
        return (
          <Box key={r.task} flexDirection="column" marginTop={1}>
            <Text color={r.isError ? 'red' : 'green'}>
              {r.isError ? '✗' : '✓'} {task}
            </Text>
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
