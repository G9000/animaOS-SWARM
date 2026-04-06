import React from 'react';
import { Box, Text } from 'ink';
import type { ResultEntry } from './result-log.js';

export interface ResultViewProps {
  entry: ResultEntry;
  onBack: () => void;
  hint?: string;
  note?: string;
  pendingDeleteNotice?: string;
}

export function ResultView({
  entry,
  onBack,
  hint,
  note,
  pendingDeleteNotice,
}: ResultViewProps): React.ReactElement {
  void onBack; // back is triggered via /back slash command in app

  return (
    <Box flexDirection="column" paddingX={1}>
      <Box borderStyle="single" paddingX={1}>
        <Text bold color="cyan">
          Result — type /back to return
        </Text>
      </Box>
      {hint ? (
        <Box marginTop={1}>
          <Text color="gray">{hint}</Text>
        </Box>
      ) : null}
      {note ? (
        <Box marginTop={1}>
          <Text color="gray">{note}</Text>
        </Box>
      ) : null}
      {pendingDeleteNotice ? (
        <Box marginTop={1} flexDirection="column">
          <Text bold color="yellow">
            Pending delete
          </Text>
          <Text color="gray" wrap="wrap">
            {pendingDeleteNotice}
          </Text>
        </Box>
      ) : null}
      {entry.label ? (
        <Box marginTop={1}>
          <Text bold color="white">
            Saved run:{' '}
          </Text>
          <Text color="magenta">{entry.label}</Text>
        </Box>
      ) : null}
      <Box marginTop={1}>
        <Text bold color="white">
          Task:{' '}
        </Text>
        <Text color="gray">{entry.task}</Text>
      </Box>
      <Box marginTop={1} flexDirection="column">
        <Text bold color={entry.isError ? 'red' : 'green'}>
          {entry.isError ? 'Error' : 'Result'}:
        </Text>
        <Box marginTop={1}>
          <Text wrap="wrap">{entry.result}</Text>
        </Box>
      </Box>
    </Box>
  );
}
