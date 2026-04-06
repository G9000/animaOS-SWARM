import React, { useState } from 'react';
import { Box, Text, useInput } from 'ink';
import type { ResultEntry } from './result-log.js';

function matchesHistoryEntry(entry: ResultEntry, query: string): boolean {
  return `${entry.label ?? ''}\n${entry.task}\n${entry.result}`
    .toLowerCase()
    .includes(query);
}

function nextMatchIndex(
  matchIndexes: number[],
  currentIdx: number,
  direction: 1 | -1
): number | null {
  if (matchIndexes.length === 0) {
    return null;
  }

  const currentMatch = matchIndexes.indexOf(currentIdx);
  if (currentMatch >= 0) {
    return (
      matchIndexes[
        (currentMatch + direction + matchIndexes.length) % matchIndexes.length
      ] ?? null
    );
  }

  if (direction > 0) {
    return (
      matchIndexes.find((idx) => idx > currentIdx) ?? matchIndexes[0] ?? null
    );
  }

  return (
    [...matchIndexes].reverse().find((idx) => idx < currentIdx) ??
    matchIndexes[matchIndexes.length - 1] ??
    null
  );
}

export interface HistoryViewProps {
  results: ResultEntry[];
  onBack: () => void;
  onRetry?: (entry: ResultEntry) => void;
  onSelect?: (entry: ResultEntry) => void;
  title?: string;
  selectActionLabel?: string;
  initialSelection?: 'first' | 'last';
}

function formatTime(timestamp: number): string {
  return new Date(timestamp).toLocaleTimeString(undefined, {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  });
}

function truncate(value: string, maxLength: number): string {
  return value.length > maxLength
    ? value.slice(0, maxLength - 3) + '...'
    : value;
}

export function HistoryView({
  results,
  onBack,
  onRetry,
  onSelect,
  title = 'History',
  selectActionLabel = 'open',
  initialSelection = 'last',
}: HistoryViewProps): React.ReactElement {
  const [selectedIdx, setSelectedIdx] = useState(() => {
    if (results.length === 0) {
      return 0;
    }

    return initialSelection === 'first' ? 0 : results.length - 1;
  });
  const [searchMode, setSearchMode] = useState(false);
  const [searchQuery, setSearchQuery] = useState('');

  const clampedIdx =
    results.length === 0 ? 0 : Math.min(selectedIdx, results.length - 1);
  const selected = results[clampedIdx];
  const normalizedQuery = searchQuery.trim().toLowerCase();
  const matchIndexes = normalizedQuery
    ? results.flatMap((entry, idx) =>
        matchesHistoryEntry(entry, normalizedQuery) ? [idx] : []
      )
    : [];
  const currentMatch = matchIndexes.indexOf(clampedIdx);
  const start = Math.max(0, clampedIdx - 4);
  const visible = results.slice(start, start + 8);

  function updateSearch(nextQuery: string) {
    setSearchQuery(nextQuery);

    const normalized = nextQuery.trim().toLowerCase();
    if (!normalized) {
      return;
    }

    const nextMatches = results.flatMap((entry, idx) =>
      matchesHistoryEntry(entry, normalized) ? [idx] : []
    );
    const nextIdx =
      nextMatches.find((idx) => idx >= clampedIdx) ?? nextMatches[0];

    if (nextIdx !== undefined) {
      setSelectedIdx(nextIdx);
    }
  }

  function moveSearch(direction: 1 | -1) {
    const nextIdx = nextMatchIndex(matchIndexes, clampedIdx, direction);
    if (nextIdx !== null) {
      setSelectedIdx(nextIdx);
    }
  }

  useInput((input, key) => {
    if (key.escape && (searchMode || normalizedQuery)) {
      setSearchMode(false);
      setSearchQuery('');
      return;
    }

    if (searchMode) {
      if (key.return) {
        setSearchMode(false);
        return;
      }

      if (key.backspace || key.delete) {
        updateSearch(searchQuery.slice(0, -1));
        return;
      }

      if (!key.ctrl && !key.meta && !key.tab && input.length > 0) {
        updateSearch(searchQuery + input);
      }
      return;
    }

    if (!key.ctrl && !key.meta && input === '/') {
      setSearchMode(true);
      return;
    }

    if (normalizedQuery && input === 'n') {
      moveSearch(1);
      return;
    }

    if (normalizedQuery && input === 'N') {
      moveSearch(-1);
      return;
    }

    if (key.upArrow) {
      setSelectedIdx((current) => Math.max(0, current - 1));
      return;
    }

    if (key.downArrow) {
      setSelectedIdx((current) => Math.min(results.length - 1, current + 1));
      return;
    }

    if (input.toLowerCase() === 'r' && selected && onRetry) {
      onRetry(selected);
      return;
    }

    if (key.return && selected && onSelect) {
      onSelect(selected);
      return;
    }

    if (
      key.escape ||
      input.toLowerCase() === 'q' ||
      input.toLowerCase() === 'b'
    ) {
      onBack();
    }
  });

  if (results.length === 0) {
    return (
      <Box flexDirection="column" paddingX={1}>
        <Box borderStyle="single" paddingX={1}>
          <Text bold color="cyan">
            {title}
          </Text>
          <Text color="gray"> q or esc to return</Text>
        </Box>
        <Box marginTop={1}>
          <Text dimColor>No runs recorded yet.</Text>
        </Box>
      </Box>
    );
  }

  return (
    <Box flexDirection="column" paddingX={1}>
      <Box borderStyle="single" paddingX={1}>
        <Text bold color="cyan">
          {title}
        </Text>
        <Text color="gray">
          {' '}
          {[
            '↑↓ inspect',
            '/ search',
            'n next',
            'N prev',
            onSelect ? `enter ${selectActionLabel}` : null,
            onRetry ? 'r retry' : null,
            'q back',
          ]
            .filter(Boolean)
            .join(' ')}
        </Text>
      </Box>

      {searchMode || normalizedQuery ? (
        <Box marginTop={1} paddingX={1}>
          <Text color="magenta">/ </Text>
          <Text color={searchQuery.length > 0 ? 'white' : 'gray'}>
            {searchQuery.length > 0
              ? searchQuery
              : 'type to search task or result'}
          </Text>
          <Text color="gray">
            {normalizedQuery
              ? matchIndexes.length > 0
                ? `  ${String(currentMatch + 1)}/${String(matchIndexes.length)}`
                : '  no matches'
              : '  enter close · esc clear'}
          </Text>
        </Box>
      ) : null}

      <Box marginTop={1}>
        <Box flexDirection="column" width={46} marginRight={2}>
          <Text dimColor>
            {title === 'Resume' ? 'Saved runs' : 'Past runs'} ({results.length})
          </Text>
          {visible.map((entry, idx) => {
            const entryIdx = start + idx;
            const active = entryIdx === clampedIdx;
            const matched = matchIndexes.includes(entryIdx);
            const statusColor = entry.isError ? 'red' : 'green';
            const title = entry.label?.trim() || truncate(entry.task, 34);
            return (
              <Box key={entry.id} marginTop={1} flexDirection="column">
                <Box>
                  <Text color={active ? 'magenta' : 'gray'} bold={active}>
                    {active ? '❯ ' : '  '}
                  </Text>
                  <Text color={statusColor} bold={active}>
                    {entry.isError ? 'err' : 'ok'}
                  </Text>
                  <Text color={active ? 'white' : matched ? 'yellow' : 'gray'}>
                    {' '}
                    {title}
                  </Text>
                </Box>
                <Text color="gray">
                  {'    '}
                  {formatTime(entry.timestamp)}
                </Text>
                {entry.label ? (
                  <Text color="gray" wrap="wrap">
                    {'    '}
                    {truncate(entry.task, 34)}
                  </Text>
                ) : null}
              </Box>
            );
          })}
        </Box>

        <Box
          flexDirection="column"
          flexGrow={1}
          borderStyle="single"
          paddingX={1}
        >
          <Text bold color={selected.isError ? 'red' : 'green'}>
            {selected.isError ? 'Failed run' : 'Successful run'}
          </Text>
          <Box marginTop={1}>
            <Text bold color="white">
              Time:{' '}
            </Text>
            <Text color="gray">{formatTime(selected.timestamp)}</Text>
          </Box>
          {selected.label ? (
            <Box marginTop={1} flexDirection="column">
              <Text bold color="white">
                Saved run:
              </Text>
              <Text color="magenta" wrap="wrap">
                {selected.label}
              </Text>
            </Box>
          ) : null}
          <Box marginTop={1} flexDirection="column">
            <Text bold color="white">
              Task:
            </Text>
            <Text color="gray" wrap="wrap">
              {selected.task}
            </Text>
          </Box>
          <Box marginTop={1} flexDirection="column">
            <Text bold color={selected.isError ? 'red' : 'green'}>
              {selected.isError ? 'Error:' : 'Result:'}
            </Text>
            <Text wrap="wrap">{selected.result}</Text>
          </Box>
        </Box>
      </Box>
    </Box>
  );
}
