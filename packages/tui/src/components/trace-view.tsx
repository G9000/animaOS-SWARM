import React, { useState } from 'react';
import { Box, Text, useInput } from 'ink';
import type { MessageEntry, ToolEntry } from '../types.js';
import {
  buildTraceEntries,
  traceEntryDetail,
  traceEntrySummary,
  type TraceEntry,
} from '../trace.js';

function matchesTraceEntry(entry: TraceEntry, query: string): boolean {
  return `${traceEntrySummary(entry)}\n${traceEntryDetail(entry)}`
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

export interface TraceViewProps {
  messages: MessageEntry[];
  tools: ToolEntry[];
  onBack: () => void;
}

function entryColor(entry: TraceEntry): string {
  if (entry.kind === 'message') {
    return 'cyan';
  }

  switch (entry.tool.status) {
    case 'running':
      return 'yellow';
    case 'success':
      return 'green';
    case 'error':
      return 'red';
  }
}

export function TraceView({
  messages,
  tools,
  onBack,
}: TraceViewProps): React.ReactElement {
  const entries = buildTraceEntries(messages, tools);
  const [selectedIdx, setSelectedIdx] = useState(
    entries.length > 0 ? entries.length - 1 : 0
  );
  const [searchMode, setSearchMode] = useState(false);
  const [searchQuery, setSearchQuery] = useState('');

  const clampedIdx =
    entries.length === 0 ? 0 : Math.min(selectedIdx, entries.length - 1);
  const selected = entries[clampedIdx];
  const normalizedQuery = searchQuery.trim().toLowerCase();
  const matchIndexes = normalizedQuery
    ? entries.flatMap((entry, idx) =>
        matchesTraceEntry(entry, normalizedQuery) ? [idx] : []
      )
    : [];
  const currentMatch = matchIndexes.indexOf(clampedIdx);
  const start = Math.max(0, clampedIdx - 4);
  const visible = entries.slice(start, start + 8);

  function updateSearch(nextQuery: string) {
    setSearchQuery(nextQuery);

    const normalized = nextQuery.trim().toLowerCase();
    if (!normalized) {
      return;
    }

    const nextMatches = entries.flatMap((entry, idx) =>
      matchesTraceEntry(entry, normalized) ? [idx] : []
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
      setSelectedIdx((current) => Math.min(entries.length - 1, current + 1));
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

  if (entries.length === 0) {
    return (
      <Box flexDirection="column" paddingX={1}>
        <Box borderStyle="single" paddingX={1}>
          <Text bold color="cyan">
            Trace
          </Text>
          <Text color="gray"> q or esc to return</Text>
        </Box>
        <Box marginTop={1}>
          <Text dimColor>No messages or tool activity yet.</Text>
        </Box>
      </Box>
    );
  }

  return (
    <Box flexDirection="column" paddingX={1}>
      <Box borderStyle="single" paddingX={1}>
        <Text bold color="cyan">
          Trace
        </Text>
        <Text color="gray"> ↑↓ inspect / search n next N prev q back</Text>
      </Box>

      {searchMode || normalizedQuery ? (
        <Box marginTop={1} paddingX={1}>
          <Text color="magenta">/ </Text>
          <Text color={searchQuery.length > 0 ? 'white' : 'gray'}>
            {searchQuery.length > 0
              ? searchQuery
              : 'type to search messages, tools, and results'}
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
        <Box flexDirection="column" width={44} marginRight={2}>
          <Text dimColor>Activity ({entries.length})</Text>
          {visible.map((entry, idx) => {
            const entryIdx = start + idx;
            const active = entryIdx === clampedIdx;
            const matched = matchIndexes.includes(entryIdx);
            return (
              <Box key={entry.id} marginTop={1}>
                <Text color={active ? 'magenta' : 'gray'} bold={active}>
                  {active ? '❯ ' : '  '}
                </Text>
                <Text color={entryColor(entry)} bold={active}>
                  {entry.kind === 'message' ? 'msg' : 'tool'}
                </Text>
                <Text color={active ? 'white' : matched ? 'yellow' : 'gray'}>
                  {' '}
                  {traceEntrySummary(entry)}
                </Text>
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
          <Text bold color={entryColor(selected)}>
            {selected.kind === 'message' ? 'Message' : 'Tool call'}
          </Text>
          <Box marginTop={1} flexDirection="column">
            {traceEntryDetail(selected)
              .split('\n')
              .map((line, idx) => (
                <Text key={`${selected.id}-${String(idx)}`} wrap="wrap">
                  {line.length > 0 ? line : ' '}
                </Text>
              ))}
          </Box>
        </Box>
      </Box>
    </Box>
  );
}
