import React, { useState } from 'react';
import { Box, Text, useInput } from 'ink';

export interface SlashCommand {
  name: string;
  description: string;
  args?: string;
}

export interface InputSuggestion {
  label: string;
  value: string;
  description?: string;
}

export interface InputBarProps {
  onSubmit: (value: string) => void;
  disabled?: boolean;
  placeholder?: string;
  commands?: SlashCommand[];
  history?: string[];
  suggestions?: (value: string) => InputSuggestion[];
}

export function InputBar({
  onSubmit,
  disabled = false,
  placeholder = 'type your task... or /help for commands',
  commands = [],
  history = [],
  suggestions,
}: InputBarProps): React.ReactElement {
  const [value, setValue] = useState('');
  const [selectedIdx, setSelectedIdx] = useState(0);
  const [historyIdx, setHistoryIdx] = useState<number | null>(null);
  const [historyDraft, setHistoryDraft] = useState('');
  const [historySearchMode, setHistorySearchMode] = useState(false);
  const [historySearchQuery, setHistorySearchQuery] = useState('');
  const [historySearchResultIdx, setHistorySearchResultIdx] = useState<
    number | null
  >(null);

  const isSlash = value.startsWith('/');
  const commandMatches = isSlash
    ? commands.filter((c) => `/${c.name}`.startsWith(value.toLowerCase()))
    : [];
  const inputSuggestions = suggestions?.(value) ?? [];
  const showingCommandMatches = commandMatches.length > 0;
  const activeMatchCount = showingCommandMatches
    ? commandMatches.length
    : inputSuggestions.length;
  const canRecallHistory = !isSlash && history.length > 0;
  const normalizedHistorySearchQuery = historySearchQuery.trim().toLowerCase();
  const historySearchMatches = history
    .flatMap((entry, idx) =>
      normalizedHistorySearchQuery.length === 0 ||
      entry.toLowerCase().includes(normalizedHistorySearchQuery)
        ? [idx]
        : []
    )
    .reverse();

  // Keep selectedIdx in bounds whenever matches change
  const clampedIdx =
    activeMatchCount > 0 ? Math.min(selectedIdx, activeMatchCount - 1) : 0;

  function clearHistoryNavigation() {
    setHistoryIdx(null);
    setHistoryDraft('');
  }

  function exitHistorySearch() {
    setHistorySearchMode(false);
    setHistorySearchQuery('');
    setHistorySearchResultIdx(null);
    setHistoryDraft('');
  }

  function previewHistorySearch(query: string, resultIdx: number | null) {
    const normalized = query.trim().toLowerCase();
    const nextMatches = history
      .flatMap((entry, idx) =>
        normalized.length === 0 || entry.toLowerCase().includes(normalized)
          ? [idx]
          : []
      )
      .reverse();

    if (nextMatches.length === 0) {
      setHistorySearchResultIdx(null);
      setValue(historyDraft);
      return;
    }

    const nextResultIdx =
      resultIdx === null ? 0 : Math.min(resultIdx, nextMatches.length - 1);
    setHistorySearchResultIdx(nextResultIdx);
    setValue(history[nextMatches[nextResultIdx]] ?? historyDraft);
  }

  function beginHistorySearch() {
    clearHistoryNavigation();
    setHistorySearchMode(true);
    setHistorySearchQuery('');
    setHistoryDraft(value);
    previewHistorySearch('', 0);
  }

  function cycleHistorySearch() {
    if (!historySearchMode) {
      beginHistorySearch();
      return;
    }

    if (historySearchMatches.length === 0) {
      return;
    }

    const nextResultIdx =
      historySearchResultIdx === null
        ? 0
        : (historySearchResultIdx + 1) % historySearchMatches.length;

    previewHistorySearch(historySearchQuery, nextResultIdx);
  }

  useInput(
    (input, key) => {
      if (key.ctrl && input === 'r' && history.length > 0) {
        cycleHistorySearch();
        return;
      }

      if (historySearchMode) {
        if (key.return) {
          setHistorySearchMode(false);
          setHistorySearchQuery('');
          setHistorySearchResultIdx(null);
          setHistoryDraft('');
          return;
        }

        if (key.escape) {
          setValue(historyDraft);
          exitHistorySearch();
          return;
        }

        if (key.backspace || key.delete) {
          const nextQuery = historySearchQuery.slice(0, -1);
          setHistorySearchQuery(nextQuery);
          previewHistorySearch(nextQuery, 0);
          return;
        }

        if (!key.ctrl && !key.meta && !key.tab && input.length > 0) {
          const nextQuery = historySearchQuery + input;
          setHistorySearchQuery(nextQuery);
          previewHistorySearch(nextQuery, 0);
        }
        return;
      }

      if (activeMatchCount > 0) {
        if (key.upArrow) {
          setSelectedIdx((i) => Math.max(0, i - 1));
          return;
        }
        if (key.downArrow) {
          setSelectedIdx((i) => Math.min(activeMatchCount - 1, i + 1));
          return;
        }
        if (key.return) {
          if (showingCommandMatches) {
            const match = commandMatches[clampedIdx];
            if (!match) {
              return;
            }

            if (match.args) {
              // Has args — autocomplete name and let user fill args
              setValue(`/${match.name} `);
              setSelectedIdx(0);
              clearHistoryNavigation();
            } else {
              // No args — submit directly
              onSubmit(`/${match.name}`);
              setValue('');
              setSelectedIdx(0);
              clearHistoryNavigation();
            }
            return;
          }

          if (value.trim()) {
            onSubmit(value.trim());
            setValue('');
            setSelectedIdx(0);
            clearHistoryNavigation();
          }
          return;
        }
        if (key.tab) {
          if (showingCommandMatches) {
            const match = commandMatches[clampedIdx];
            if (!match) {
              return;
            }

            setValue(match.args ? `/${match.name} ` : `/${match.name}`);
          } else {
            const match = inputSuggestions[clampedIdx];
            if (!match) {
              return;
            }

            setValue(match.value);
          }
          setSelectedIdx(0);
          clearHistoryNavigation();
          return;
        }
      }

      if (canRecallHistory && key.upArrow) {
        setSelectedIdx(0);
        setHistoryIdx((current) => {
          const nextIdx =
            current === null ? history.length - 1 : Math.max(0, current - 1);
          if (current === null) {
            setHistoryDraft(value);
          }
          setValue(history[nextIdx] ?? '');
          return nextIdx;
        });
        return;
      }

      if (canRecallHistory && key.downArrow && historyIdx !== null) {
        setSelectedIdx(0);
        setHistoryIdx((current) => {
          if (current === null) {
            return null;
          }

          const nextIdx = current < history.length - 1 ? current + 1 : null;
          setValue(nextIdx === null ? historyDraft : history[nextIdx] ?? '');
          if (nextIdx === null) {
            setHistoryDraft('');
          }
          return nextIdx;
        });
        return;
      }

      if (key.return) {
        if (value.trim()) {
          onSubmit(value.trim());
          setValue('');
          setSelectedIdx(0);
          clearHistoryNavigation();
        }
      } else if (key.backspace || key.delete) {
        setValue((v) => v.slice(0, -1));
        setSelectedIdx(0);
        clearHistoryNavigation();
      } else if (!key.ctrl && !key.meta && !key.escape && !key.tab) {
        setValue((v) => v + input);
        setSelectedIdx(0);
        clearHistoryNavigation();
      }
    },
    { isActive: !disabled }
  );

  let body: React.ReactElement;
  if (disabled) {
    body = <Text color="yellow">running swarm...</Text>;
  } else if (value) {
    body = (
      <Text>
        {isSlash ? <Text color="magenta">{value}</Text> : value}
        <Text color="cyan">▌</Text>
      </Text>
    );
  } else {
    body = (
      <Text>
        <Text color="gray">{placeholder}</Text>
        <Text color="cyan">▌</Text>
      </Text>
    );
  }

  return (
    <Box flexDirection="column">
      {historySearchMode && (
        <Box flexDirection="column" paddingX={2}>
          <Text color="magenta">ctrl+r history search</Text>
          <Text>
            <Text color="magenta">query </Text>
            <Text color={historySearchQuery.length > 0 ? 'white' : 'gray'}>
              {historySearchQuery.length > 0
                ? historySearchQuery
                : 'search previous prompts'}
            </Text>
            <Text color="gray">
              {historySearchMatches.length > 0
                ? `  ${String((historySearchResultIdx ?? 0) + 1)}/${String(
                    historySearchMatches.length
                  )}`
                : '  no matches'}
            </Text>
          </Text>
          <Text color="gray" dimColor>
            {'ctrl+r older match · enter accept · esc cancel'}
          </Text>
        </Box>
      )}

      {/* Command palette */}
      {activeMatchCount > 0 && !historySearchMode && (
        <Box flexDirection="column" paddingX={2}>
          {showingCommandMatches
            ? commandMatches.map((match, i) => {
                const active = i === clampedIdx;
                return (
                  <Box key={match.name}>
                    <Text color={active ? 'magenta' : 'gray'} bold={active}>
                      {active ? '❯ ' : '  '}
                    </Text>
                    <Text color={active ? 'magenta' : 'gray'} bold={active}>
                      {'/'}
                      {match.name}
                      {match.args ? (
                        <Text color="gray"> {match.args}</Text>
                      ) : null}
                    </Text>
                    <Text color={active ? 'white' : 'gray'}>
                      {'  '}
                      {match.description}
                    </Text>
                  </Box>
                );
              })
            : inputSuggestions.map((match, i) => {
                const active = i === clampedIdx;
                return (
                  <Box key={`${match.value}-${String(i)}`}>
                    <Text color={active ? 'magenta' : 'gray'} bold={active}>
                      {active ? '❯ ' : '  '}
                    </Text>
                    <Text color={active ? 'magenta' : 'gray'} bold={active}>
                      {match.label}
                    </Text>
                    {match.description ? (
                      <Text color={active ? 'white' : 'gray'}>
                        {'  '}
                        {match.description}
                      </Text>
                    ) : null}
                  </Box>
                );
              })}
          <Text color="gray" dimColor>
            {showingCommandMatches
              ? '  ↑↓ navigate · enter select · tab complete'
              : '  ↑↓ navigate · tab complete · enter submit'}
          </Text>
        </Box>
      )}

      {activeMatchCount === 0 &&
        history.length > 0 &&
        !disabled &&
        !historySearchMode && (
          <Box paddingX={2}>
            <Text color="gray" dimColor>
              {'↑↓ recall previous prompts · ctrl+r search history'}
            </Text>
          </Box>
        )}

      <Box borderStyle="round" paddingX={1}>
        <Text bold color="cyan">
          {'>'}{' '}
        </Text>
        {body}
      </Box>
    </Box>
  );
}
