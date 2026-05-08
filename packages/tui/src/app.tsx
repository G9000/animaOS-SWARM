import React, { useState, useCallback, useRef } from 'react';
import { Box, Text, useApp, useInput } from 'ink';
import { type IEventBus, type TaskResult } from '@animaOS-SWARM/core';
import { useDaemonState } from './hooks/use-daemon-state.js';
import { useEventLog } from './hooks/use-event-log.js';
import { Header } from './components/header.js';
import { AgentPanel } from './components/agent-panel.js';
import { MessageStream } from './components/message-stream.js';
import { ToolPanel } from './components/tool-panel.js';
import { StatusBar } from './components/status-bar.js';
import { InputBar } from './components/input-bar.js';
import { ResultLog } from './components/result-log.js';
import { ResultView } from './components/result-view.js';
import { AgentsPanel } from './components/agents-panel.js';
import { TraceView } from './components/trace-view.js';
import { HistoryView } from './components/history-view.js';
import { buildDisplayedAgents } from './displayed-agents.js';
import { buildAppInputCommands } from './input-commands.js';
import type { ResultEntry } from './components/result-log.js';
import type { InputSuggestion } from './components/input-bar.js';
import type { AgentProfile } from './types.js';

export interface AppProps {
  eventBus: IEventBus;
  strategy: string;
  /** One-shot mode task. Omit to use interactive mode. */
  task?: string;
  /** Enable interactive input bar */
  interactive?: boolean;
  /** Called when user submits a task in interactive mode */
  onTask?: (task: string) => Promise<TaskResult>;
  /** Agent profiles for the /agents panel */
  agentProfiles?: AgentProfile[];
  /** Called when user edits and saves an agent */
  onSaveAgent?: (profile: AgentProfile) => void;
  /** Preloaded run history from previous sessions */
  initialResults?: ResultEntry[];
  /** Resume the last persisted result when the app opens */
  resumeLastResult?: boolean;
  /** Called whenever a new run is recorded */
  onResultRecorded?: (entry: ResultEntry) => void;
  /** Called whenever saved run metadata changes */
  onHistoryUpdated?: (entries: ResultEntry[]) => void;
  /** Called when history is cleared */
  onClearHistory?: () => void;
  /** Persistent daemon warning shown in the swarm view before tasks run */
  preflightWarning?: string;
  /** Poll for daemon health warnings while the TUI is open */
  pollDaemonWarning?: () => Promise<string | undefined>;
  /** Maximum number of prompt-history entries retained for the input bar's
   * up/down recall and Ctrl+R search. Defaults to 100. */
  maxPromptHistory?: number;
  /** Maximum number of deleted saved-runs that can be queued for `/undo`.
   * Defaults to 5. The oldest entry is dropped when this is exceeded. */
  maxDeletedSavedRunUndos?: number;
  /** Called for non-fatal diagnostic events that would otherwise be
   * swallowed (malformed event payloads, hung daemon polls, etc.). */
  onWarning?: (where: string, detail: unknown) => void;
}

type AppView = 'swarm' | 'agents' | 'history' | 'trace' | 'result';
type HistoryMode = 'history' | 'resume';
type ResumeAssistState = {
  kind: 'matches' | 'suggestions';
  query: string;
  entries: ResultEntry[];
  selectedIdx: number;
};
type PendingDeleteCommandState = {
  targetId: string;
  label: string;
  confirmationCommand: string;
};
type PendingDropOldestUndoCommandState = {
  label: string;
};
type DeletedSavedRunState = {
  entry: ResultEntry;
  index: number;
};
const DEFAULT_MAX_PROMPT_HISTORY = 100;
const DEFAULT_MAX_DELETED_SAVED_RUN_UNDOS = 5;
const TASK_SUBMISSION_BLOCKED_MESSAGE =
  'Task submission is blocked while the daemon is down. Use /health to recheck.';
const TASK_INPUT_COMMAND_ONLY_HINT =
  'daemon down - tasks paused; use /health or /help';
const TASK_INPUT_COMMAND_ONLY_HELPER =
  'commands only while daemon is down · /health recheck · /help commands';

function formatDaemonCheckTime(timestamp: number): string {
  return `${new Date(timestamp).toISOString().slice(11, 19)}Z`;
}

function hasSavedRunLabel(
  entry: ResultEntry
): entry is ResultEntry & { label: string } {
  return typeof entry.label === 'string' && entry.label.trim().length > 0;
}

function sortResumeEntries(entries: ResultEntry[]): ResultEntry[] {
  return [...entries].sort((left, right) => {
    const leftNamed = hasSavedRunLabel(left);
    const rightNamed = hasSavedRunLabel(right);

    if (leftNamed !== rightNamed) {
      return leftNamed ? -1 : 1;
    }

    return right.timestamp - left.timestamp;
  });
}

function findSavedRunByLabel(
  entries: ResultEntry[],
  query: string
): ResultEntry[] {
  const normalized = query.trim().toLowerCase();
  if (!normalized) {
    return [];
  }

  const labeledEntries = sortResumeEntries(entries).filter(hasSavedRunLabel);

  const exactMatches = labeledEntries.filter(
    (entry) => entry.label.toLowerCase() === normalized
  );
  if (exactMatches.length > 0) {
    return exactMatches;
  }

  const prefixMatches = labeledEntries.filter((entry) =>
    entry.label.toLowerCase().startsWith(normalized)
  );
  if (prefixMatches.length > 0) {
    return prefixMatches;
  }

  return labeledEntries.filter((entry) =>
    entry.label.toLowerCase().includes(normalized)
  );
}

function findSavedRunByExactLabel(
  entries: ResultEntry[],
  query: string
): ResultEntry | undefined {
  const normalized = query.trim().toLowerCase();
  if (!normalized) {
    return undefined;
  }

  return sortResumeEntries(entries)
    .filter(hasSavedRunLabel)
    .find((entry) => entry.label.toLowerCase() === normalized);
}

function buildDeleteConfirmationCommand(label?: string): string {
  const normalizedLabel = label?.trim().toLowerCase();
  return normalizedLabel ? `/delete ${normalizedLabel}` : '/delete';
}

function formatUndoQueueStatus(
  deletedSavedRunStack: DeletedSavedRunState[],
  undoLimit: number
): string {
  if (deletedSavedRunStack.length === 0) {
    return 'Undo queue empty. Delete a saved run first.';
  }

  const nextEntry =
    deletedSavedRunStack[deletedSavedRunStack.length - 1]?.entry;
  const oldestEntry = deletedSavedRunStack[0]?.entry;
  const nextLabel = nextEntry?.label?.trim() || nextEntry?.task || 'unknown';
  const oldestLabel =
    oldestEntry?.label?.trim() || oldestEntry?.task || 'unknown';
  const queueCount = deletedSavedRunStack.length;

  return [
    `Undo queue: ${String(queueCount)} deleted saved run${
      queueCount === 1 ? '' : 's'
    }.`,
    `Next restore ${nextLabel}.`,
    queueCount > 1 ? `Oldest queued ${oldestLabel}.` : null,
    `Limit ${String(undoLimit)}.`,
    'Open /resume and press u to restore.',
  ]
    .filter(Boolean)
    .join(' ');
}

function formatSavedRunMatchSummary(entries: ResultEntry[]): string {
  const preview = entries
    .slice(0, 3)
    .map((entry) => `"${entry.label}"`)
    .join(', ');

  if (entries.length <= 3) {
    return preview;
  }

  return `${preview}, +${String(entries.length - 3)} more`;
}

function subsequenceScore(source: string, query: string): number {
  let score = 0;
  let sourceIndex = 0;
  let lastMatchIndex = -2;

  for (const char of query) {
    const matchIndex = source.indexOf(char, sourceIndex);
    if (matchIndex === -1) {
      return 0;
    }

    score += matchIndex === lastMatchIndex + 1 ? 3 : 1;
    sourceIndex = matchIndex + 1;
    lastMatchIndex = matchIndex;
  }

  return score;
}

function savedRunSuggestionScore(entry: ResultEntry, query: string): number {
  if (!hasSavedRunLabel(entry)) {
    return 0;
  }

  const normalizedLabel = entry.label.toLowerCase();
  const normalizedQuery = query.trim().toLowerCase();
  if (!normalizedQuery) {
    return 0;
  }

  if (normalizedLabel === normalizedQuery) {
    return 1000;
  }

  if (normalizedLabel.startsWith(normalizedQuery)) {
    return 800 - normalizedLabel.length;
  }

  const includeIndex = normalizedLabel.indexOf(normalizedQuery);
  if (includeIndex >= 0) {
    return 600 - includeIndex;
  }

  return subsequenceScore(normalizedLabel, normalizedQuery);
}

function suggestSavedRunsByLabel(
  entries: ResultEntry[],
  query: string,
  limit = 3
): ResultEntry[] {
  return sortResumeEntries(entries)
    .map((entry) => ({
      entry,
      score: savedRunSuggestionScore(entry, query),
    }))
    .filter((candidate) => candidate.score > 0)
    .sort((left, right) => right.score - left.score)
    .slice(0, limit)
    .map((candidate) => candidate.entry);
}

function uniqueSavedRunLabels(entries: ResultEntry[]): ResultEntry[] {
  const seen = new Set<string>();

  return entries.filter((entry) => {
    if (!hasSavedRunLabel(entry)) {
      return false;
    }

    const normalizedLabel = entry.label.toLowerCase();
    if (seen.has(normalizedLabel)) {
      return false;
    }

    seen.add(normalizedLabel);
    return true;
  });
}

function normalizeSavedRunLabel(label: string): string {
  return label.trim().toLowerCase();
}

function suggestNextSavedRunLabel(
  entries: ResultEntry[],
  requestedLabel: string,
  targetId?: string
): string {
  const normalizedRequested = requestedLabel.trim();
  if (!normalizedRequested) {
    return normalizedRequested;
  }

  const usedLabels = new Set(
    entries
      .filter((entry) => entry.id !== targetId)
      .filter(hasSavedRunLabel)
      .map((entry) => normalizeSavedRunLabel(entry.label))
  );

  if (!usedLabels.has(normalizeSavedRunLabel(normalizedRequested))) {
    return normalizedRequested;
  }

  const suffixMatch = /^(.*?)(?:\s+(\d+))$/.exec(normalizedRequested);
  const baseLabel = suffixMatch?.[1]?.trim() || normalizedRequested;
  const hasExistingBaseLabel = usedLabels.has(
    normalizeSavedRunLabel(baseLabel)
  );
  const candidateBase = hasExistingBaseLabel ? baseLabel : normalizedRequested;
  let nextSuffix =
    hasExistingBaseLabel && suffixMatch?.[2] ? Number(suffixMatch[2]) + 1 : 2;

  let candidate = `${candidateBase} ${String(nextSuffix)}`;
  while (usedLabels.has(normalizeSavedRunLabel(candidate))) {
    nextSuffix += 1;
    candidate = `${candidateBase} ${String(nextSuffix)}`;
  }

  return candidate;
}

function findSavedRunLabelSuggestionEntries(
  entries: ResultEntry[],
  query: string,
  limit = 5
): ResultEntry[] {
  const normalizedQuery = query.trim();

  if (!normalizedQuery) {
    return uniqueSavedRunLabels(sortResumeEntries(entries)).slice(0, limit);
  }

  const directMatches = uniqueSavedRunLabels(
    findSavedRunByLabel(entries, normalizedQuery)
  );
  if (directMatches.length > 0) {
    return directMatches.slice(0, limit);
  }

  return uniqueSavedRunLabels(
    suggestSavedRunsByLabel(entries, normalizedQuery, limit * 2)
  ).slice(0, limit);
}

function buildSavedRunLabelInputSuggestions(
  entries: ResultEntry[],
  value: string,
  commandName: 'resume' | 'rename' | 'delete',
  limit = 5
): InputSuggestion[] {
  const match = new RegExp(`^/${commandName}(?:\\s+(.*))?$`, 'i').exec(value);
  if (!match) {
    return [];
  }

  const query = match[1]?.trim() ?? '';
  const candidates = findSavedRunLabelSuggestionEntries(entries, query, limit);

  const suggestions = candidates.map((entry) => ({
    label: entry.label ?? entry.task,
    value: `/${commandName} ${entry.label ?? entry.task}`,
    description: entry.task,
  }));

  if (commandName === 'rename' && query) {
    const suggestedLabel = suggestNextSavedRunLabel(entries, query);
    if (
      normalizeSavedRunLabel(suggestedLabel) !== normalizeSavedRunLabel(query)
    ) {
      suggestions.unshift({
        label: suggestedLabel,
        value: `/rename ${suggestedLabel}`,
        description: 'next available label',
      });
    }
  }

  return suggestions.slice(0, limit);
}

export function App({
  eventBus,
  strategy,
  task,
  interactive = false,
  onTask,
  agentProfiles = [],
  onSaveAgent,
  initialResults = [],
  resumeLastResult = false,
  onResultRecorded,
  onHistoryUpdated,
  onClearHistory,
  preflightWarning,
  pollDaemonWarning,
  maxPromptHistory = DEFAULT_MAX_PROMPT_HISTORY,
  maxDeletedSavedRunUndos = DEFAULT_MAX_DELETED_SAVED_RUN_UNDOS,
  onWarning,
}: AppProps): React.ReactElement {
  const { exit } = useApp();
  const [phase, setPhase] = useState<'waiting' | 'running'>('waiting');
  const { agents, messages, tools, stats, done, result, error, reset } =
    useEventLog({
      eventBus,
      strategy,
      isRunning: phase === 'running',
      onWarning,
    });
  const resumeEntry = resumeLastResult
    ? initialResults[initialResults.length - 1]
    : undefined;

  const [view, setView] = useState<AppView>(resumeEntry ? 'result' : 'swarm');
  const [currentTask, setCurrentTask] = useState(
    task ?? resumeEntry?.task ?? ''
  );
  const [resultLog, setResultLog] = useState<ResultEntry[]>(initialResults);
  const [historyMode, setHistoryMode] = useState<HistoryMode>('history');
  const [promptHistory, setPromptHistory] = useState<string[]>(
    initialResults.map((entry) => entry.task).slice(-maxPromptHistory)
  );
  const [resumeAssist, setResumeAssist] = useState<ResumeAssistState | null>(
    null
  );
  const [pendingDeleteCommand, setPendingDeleteCommand] =
    useState<PendingDeleteCommandState | null>(null);
  const [pendingDropOldestUndoCommand, setPendingDropOldestUndoCommand] =
    useState<PendingDropOldestUndoCommandState | null>(null);
  const [deletedSavedRunStack, setDeletedSavedRunStack] = useState<
    DeletedSavedRunState[]
  >([]);
  const [systemMsg, setSystemMsg] = useState<string | null>(null);
  const [profiles, setProfiles] = useState<AgentProfile[]>(agentProfiles);
  const [activeResultId, setActiveResultId] = useState<string | null>(
    resumeEntry?.id ?? null
  );
  const [resumeEntryId, setResumeEntryId] = useState<string | null>(
    resumeEntry?.id ?? null
  );
  const nextResultId = useRef(0);
  const hasTrace = messages.length > 0 || tools.length > 0;
  const activeResult =
    (activeResultId
      ? resultLog.find((entry) => entry.id === activeResultId)
      : undefined) ?? resultLog[resultLog.length - 1];
  const nextUndoEntry =
    deletedSavedRunStack[deletedSavedRunStack.length - 1]?.entry;
  const nextUndoLabel =
    nextUndoEntry?.label?.trim() || nextUndoEntry?.task || undefined;
  const oldestUndoEntry = deletedSavedRunStack[0]?.entry;
  const oldestUndoLabel =
    oldestUndoEntry?.label?.trim() || oldestUndoEntry?.task || undefined;
  const pendingDropOldestUndoLabel =
    pendingDropOldestUndoCommand?.label === oldestUndoLabel
      ? oldestUndoLabel
      : undefined;
  const undoQueueIsFull =
    deletedSavedRunStack.length === maxDeletedSavedRunUndos;
  const displayedHistoryResults =
    historyMode === 'resume' ? sortResumeEntries(resultLog) : resultLog;
  const canExitOneShot =
    !interactive && view === 'swarm' && (done || Boolean(error));
  const displayedAgents = buildDisplayedAgents(profiles, agents);
  const configuredAgentCount =
    profiles.length > 0 ? profiles.length : undefined;
  const visibleAgentCount = configuredAgentCount ?? stats.agentCount;
  const showMsg = useCallback((msg: string) => {
    setSystemMsg(msg);
    setTimeout(() => setSystemMsg(null), 3000);
  }, []);

  const {
    daemonWarning,
    daemonRecoveryNotice,
    daemonStatus,
    lastDaemonCheckAt,
    taskEntryBlockedByDaemon,
    syncDaemonWarning,
  } = useDaemonState({
    interactive,
    phase,
    preflightWarning,
    pollDaemonWarning,
    showMessage: showMsg,
    onWarning,
  });
  const { swarmInputCommands, resultInputCommands, helpCommands } =
    buildAppInputCommands({
      interactive,
      hasSavedRuns: resultLog.length > 0,
      taskEntryBlockedByDaemon,
      showResultCommands: view === 'result' && Boolean(activeResult),
    });

  const blockInteractiveTaskSubmission = useCallback(() => {
    if (!interactive || !daemonWarning) {
      return false;
    }

    showMsg(TASK_SUBMISSION_BLOCKED_MESSAGE);
    return true;
  }, [daemonWarning, interactive, showMsg]);

  const showStatus = useCallback(() => {
    const taskLabel = currentTask
      ? currentTask.length > 40
        ? currentTask.slice(0, 37) + '...'
        : currentTask
      : interactive
      ? 'interactive'
      : 'idle';

    showMsg(
      [
        `Status: ${phase}`,
        `task ${taskLabel}`,
        typeof configuredAgentCount === 'number'
          ? `agents ${configuredAgentCount} configured / ${stats.agentCount} active`
          : `agents ${stats.agentCount}`,
        daemonStatus
          ? `daemon ${daemonStatus}${
              lastDaemonCheckAt
                ? ` checked ${formatDaemonCheckTime(lastDaemonCheckAt)}`
                : ''
            }`
          : null,
        `messages ${messages.length}`,
        `tools ${tools.length}`,
        `history ${resultLog.length}`,
        hasTrace ? 'trace ready' : 'trace empty',
      ].join('  ·  ')
    );
  }, [
    currentTask,
    hasTrace,
    messages.length,
    phase,
    resultLog.length,
    configuredAgentCount,
    daemonStatus,
    lastDaemonCheckAt,
    stats.agentCount,
    tools.length,
    showMsg,
  ]);

  const clearResumeAssist = useCallback(() => {
    setResumeAssist(null);
  }, []);

  const moveResumeAssistSelection = useCallback((direction: 1 | -1) => {
    setResumeAssist((current) => {
      if (!current || current.entries.length === 0) {
        return current;
      }

      const nextIdx =
        (current.selectedIdx + direction + current.entries.length) %
        current.entries.length;

      return {
        ...current,
        selectedIdx: nextIdx,
      };
    });
  }, []);

  const renameSavedRun = useCallback(
    (targetId: string, nextLabel: string) => {
      const normalizedLabel = nextLabel.trim();
      if (!normalizedLabel) {
        showMsg('Provide a name, for example /rename launch hotfix.');
        return;
      }

      const targetExists = resultLog.some((entry) => entry.id === targetId);
      if (!targetExists) {
        showMsg('Saved run no longer exists. Open /resume and try again.');
        return;
      }

      const duplicateLabelEntry = resultLog.find(
        (entry) =>
          entry.id !== targetId &&
          hasSavedRunLabel(entry) &&
          normalizeSavedRunLabel(entry.label) ===
            normalizeSavedRunLabel(normalizedLabel)
      );
      if (duplicateLabelEntry) {
        const suggestedLabel = suggestNextSavedRunLabel(
          resultLog,
          normalizedLabel,
          targetId
        );
        showMsg(
          `Saved run label "${normalizedLabel}" is already used. Try /rename ${suggestedLabel} to keep /resume unambiguous.`
        );
        return;
      }

      const next = resultLog.map((entry) =>
        entry.id === targetId
          ? {
              ...entry,
              label: normalizedLabel,
            }
          : entry
      );

      setResultLog(next);
      onHistoryUpdated?.(next);
      showMsg(`Saved run named: ${normalizedLabel}`);
    },
    [onHistoryUpdated, resultLog, showMsg]
  );

  const deleteSavedRun = useCallback(
    (targetId: string) => {
      const targetEntry = resultLog.find((entry) => entry.id === targetId);
      if (!targetEntry) {
        showMsg('Saved run no longer exists. Open /resume and try again.');
        return;
      }

      const targetIndex = resultLog.findIndex((entry) => entry.id === targetId);

      const next = resultLog.filter((entry) => entry.id !== targetId);
      const nextDeletedSavedRunStack = [
        ...deletedSavedRunStack,
        {
          entry: targetEntry,
          index: targetIndex,
        },
      ];
      const droppedUndoEntry =
        nextDeletedSavedRunStack.length > maxDeletedSavedRunUndos
          ? nextDeletedSavedRunStack[0]
          : undefined;

      setResultLog(next);
      setPendingDeleteCommand(null);
      setPendingDropOldestUndoCommand(null);
      setDeletedSavedRunStack(
        nextDeletedSavedRunStack.slice(-maxDeletedSavedRunUndos)
      );
      onHistoryUpdated?.(next);

      if (activeResultId === targetId) {
        setActiveResultId(next[next.length - 1]?.id ?? null);
      }

      if (resumeEntryId === targetId) {
        setResumeEntryId(null);
      }

      showMsg(
        `Deleted saved run: ${
          targetEntry.label?.trim() || targetEntry.task
        }. Press u to undo from /resume.${
          droppedUndoEntry
            ? ` Oldest undo dropped: ${
                droppedUndoEntry.entry.label?.trim() ||
                droppedUndoEntry.entry.task
              }.`
            : ''
        }`
      );
    },
    [
      activeResultId,
      deletedSavedRunStack,
      onHistoryUpdated,
      resultLog,
      resumeEntryId,
      showMsg,
    ]
  );

  const undoDeleteSavedRun = useCallback(() => {
    const lastDeletedSavedRun =
      deletedSavedRunStack[deletedSavedRunStack.length - 1];
    if (!lastDeletedSavedRun) {
      showMsg('No deleted saved run to restore.');
      return;
    }

    const insertIndex = Math.min(
      Math.max(lastDeletedSavedRun.index, 0),
      resultLog.length
    );
    const next = [
      ...resultLog.slice(0, insertIndex),
      lastDeletedSavedRun.entry,
      ...resultLog.slice(insertIndex),
    ];

    setResultLog(next);
    setPendingDropOldestUndoCommand(null);
    setDeletedSavedRunStack((prev) => prev.slice(0, -1));
    onHistoryUpdated?.(next);
    const remainingUndos = deletedSavedRunStack.length - 1;
    showMsg(
      `Restored saved run: ${
        lastDeletedSavedRun.entry.label?.trim() ||
        lastDeletedSavedRun.entry.task
      }${
        remainingUndos > 0
          ? `. ${String(remainingUndos)} more deleted run${
              remainingUndos === 1 ? '' : 's'
            } queued for undo.`
          : ''
      }`
    );
  }, [deletedSavedRunStack, onHistoryUpdated, resultLog, showMsg]);

  const dropOldestDeletedSavedRun = useCallback(() => {
    const oldestDeletedSavedRun = deletedSavedRunStack[0];
    if (!oldestDeletedSavedRun) {
      showMsg('No queued undo to discard.');
      return;
    }

    setPendingDropOldestUndoCommand(null);
    setDeletedSavedRunStack((prev) => prev.slice(1));
    const remainingUndos = deletedSavedRunStack.length - 1;
    showMsg(
      `Dropped oldest queued undo: ${
        oldestDeletedSavedRun.entry.label?.trim() ||
        oldestDeletedSavedRun.entry.task
      }${
        remainingUndos > 0
          ? `. ${String(remainingUndos)} deleted run${
              remainingUndos === 1 ? '' : 's'
            } still queued.`
          : ''
      }`
    );
  }, [deletedSavedRunStack, showMsg]);

  const openResultEntry = useCallback(
    (entry: ResultEntry, resume = false) => {
      clearResumeAssist();
      setCurrentTask(entry.task);
      setActiveResultId(entry.id);
      setResumeEntryId(resume ? entry.id : null);
      setView('result');
    },
    [clearResumeAssist]
  );

  const rememberPrompt = useCallback((input: string) => {
    setPromptHistory((prev) => [...prev, input].slice(-maxPromptHistory));
  }, []);

  const runTask = useCallback(
    async (input: string) => {
      if (!onTask) {
        return;
      }

      rememberPrompt(input);
      clearResumeAssist();
      setPendingDeleteCommand(null);
      setPendingDropOldestUndoCommand(null);
      setCurrentTask(input);
      setActiveResultId(null);
      setResumeEntryId(null);
      setView('swarm');
      setPhase('running');
      reset();

      const taskResult = await onTask(input);
      const text =
        taskResult.status === 'success'
          ? (taskResult.data as { text?: string })?.text ?? 'completed'
          : taskResult.error ?? 'error';

      const entry: ResultEntry = {
        id: `run-${Date.now()}-${String(nextResultId.current++)}`,
        timestamp: Date.now(),
        task: input,
        result: text,
        isError: taskResult.status !== 'success',
      };

      setResultLog((prev) => [...prev, entry]);
      onResultRecorded?.(entry);
      setPhase('waiting');
    },
    [clearResumeAssist, onTask, rememberPrompt, reset, onResultRecorded]
  );

  useInput(
    (input, key) => {
      if (!canExitOneShot) {
        return;
      }

      if (input.toLowerCase() === 'h' && resultLog.length > 0) {
        setHistoryMode('history');
        setView('history');
        return;
      }

      if (input.toLowerCase() === 't' && hasTrace) {
        setView('trace');
        return;
      }

      if (input.toLowerCase() === 'r' && onTask && currentTask) {
        void runTask(currentTask);
        return;
      }

      if (key.return || key.escape || input.toLowerCase() === 'q') {
        exit();
      }
    },
    { isActive: canExitOneShot }
  );

  useInput(
    (input) => {
      if (input.toLowerCase() === 's') {
        showStatus();
      }
    },
    { isActive: interactive && phase === 'running' && view === 'swarm' }
  );

  useInput(
    (input, key) => {
      if (!(key.ctrl && input.toLowerCase() === 'o')) {
        return;
      }

      if (resultLog.length === 0) {
        showMsg('No saved runs yet. Run a task first.');
        return;
      }

      clearResumeAssist();
      setHistoryMode('resume');
      setView('history');
    },
    { isActive: interactive && phase === 'waiting' && view === 'swarm' }
  );

  useInput(
    (input, key) => {
      if (!resumeAssist) {
        return;
      }

      if (key.escape) {
        clearResumeAssist();
        return;
      }

      if (key.ctrl && input.toLowerCase() === 'n') {
        moveResumeAssistSelection(1);
        return;
      }

      if (key.ctrl && input.toLowerCase() === 'p') {
        moveResumeAssistSelection(-1);
        return;
      }

      if (key.ctrl && input.toLowerCase() === 'y') {
        const entry = resumeAssist.entries[resumeAssist.selectedIdx];
        if (entry) {
          openResultEntry(entry, true);
        }
      }
    },
    { isActive: interactive && phase === 'waiting' && view === 'swarm' }
  );

  const handleSlashCommand = useCallback(
    async (cmd: string) => {
      clearResumeAssist();
      const [name, ...rest] = cmd.slice(1).trim().split(/\s+/);
      const args = rest.join(' ').trim();
      const commandName = name.toLowerCase();

      if (commandName !== 'delete' && pendingDeleteCommand) {
        setPendingDeleteCommand(null);
      }
      if (commandName !== 'undo-drop' && pendingDropOldestUndoCommand) {
        setPendingDropOldestUndoCommand(null);
      }
      switch (commandName) {
        case 'agents':
          if (profiles.length === 0) {
            showMsg('No agent profiles loaded. Create an agency first.');
          } else {
            setView('agents');
          }
          break;
        case 'history':
          if (resultLog.length === 0) {
            showMsg('No run history yet. Run a task first.');
          } else {
            setHistoryMode('history');
            setView('history');
          }
          break;
        case 'resume':
          if (resultLog.length === 0) {
            showMsg('No saved runs yet. Run a task first.');
          } else if (args) {
            const matchedEntries = findSavedRunByLabel(resultLog, args);
            if (matchedEntries.length === 0) {
              const suggestions = suggestSavedRunsByLabel(resultLog, args);
              if (suggestions.length > 0) {
                setResumeAssist({
                  kind: 'suggestions',
                  query: args,
                  entries: suggestions,
                  selectedIdx: 0,
                });
              }
              showMsg(
                `No saved run named "${args}". Type /resume to browse saved runs.`
              );
              break;
            }

            if (matchedEntries.length > 1) {
              setResumeAssist({
                kind: 'matches',
                query: args,
                entries: matchedEntries.slice(0, 3),
                selectedIdx: 0,
              });
              showMsg(
                `Multiple saved runs match "${args}": ${formatSavedRunMatchSummary(
                  matchedEntries
                )}. Type /resume to browse saved runs.`
              );
              break;
            }

            const matchedEntry = matchedEntries[0];
            if (!matchedEntry) {
              break;
            }
            setHistoryMode('resume');
            openResultEntry(matchedEntry, true);
          } else {
            setHistoryMode('resume');
            setView('history');
          }
          break;
        case 'rename': {
          const renameEntry =
            view === 'result' && activeResult
              ? activeResult
              : resultLog[resultLog.length - 1];
          if (!renameEntry) {
            showMsg('No saved runs yet. Run a task first.');
            break;
          }

          renameSavedRun(renameEntry.id, args || renameEntry.task);
          break;
        }
        case 'delete': {
          if (resultLog.length === 0) {
            showMsg('No saved runs yet. Run a task first.');
            break;
          }

          const targetEntry = args
            ? findSavedRunByExactLabel(resultLog, args)
            : view === 'result' && activeResult
            ? activeResult
            : undefined;

          if (!args && !targetEntry) {
            showMsg(
              'Open a result or provide a saved run label, for example /delete release prep.'
            );
            break;
          }

          if (args && !targetEntry) {
            showMsg(
              `No saved run named "${args}". Use Tab completion or /resume to inspect saved runs.`
            );
            break;
          }

          if (!targetEntry) {
            break;
          }

          const targetLabel = targetEntry.label?.trim() || args;
          const confirmationCommand = args
            ? buildDeleteConfirmationCommand(targetLabel)
            : buildDeleteConfirmationCommand();

          if (
            pendingDeleteCommand?.targetId === targetEntry.id &&
            pendingDeleteCommand.confirmationCommand === confirmationCommand
          ) {
            deleteSavedRun(targetEntry.id);
            break;
          }

          setPendingDeleteCommand({
            targetId: targetEntry.id,
            label: targetEntry.label?.trim() || targetEntry.task,
            confirmationCommand,
          });
          showMsg(
            args
              ? `Confirm delete: repeat /delete ${targetLabel} to remove this saved run.`
              : `Confirm delete: repeat /delete to remove ${
                  targetEntry.label?.trim() || targetEntry.task
                }.`
          );
          break;
        }
        case 'result':
          if (resultLog.length === 0) {
            showMsg('No results yet. Run a task first.');
          } else {
            const lastEntry = resultLog[resultLog.length - 1];
            if (lastEntry) {
              openResultEntry(lastEntry, false);
            }
          }
          break;
        case 'status':
          showStatus();
          break;
        case 'health':
          await syncDaemonWarning('manual');
          break;
        case 'undo':
          undoDeleteSavedRun();
          break;
        case 'undo-drop':
          if (deletedSavedRunStack.length === 0) {
            dropOldestDeletedSavedRun();
            break;
          }

          if (
            pendingDropOldestUndoCommand?.label === oldestUndoLabel &&
            oldestUndoLabel
          ) {
            dropOldestDeletedSavedRun();
            break;
          }

          if (oldestUndoLabel) {
            setPendingDropOldestUndoCommand({
              label: oldestUndoLabel,
            });
            showMsg(
              `Confirm oldest undo discard: repeat /undo-drop to discard ${oldestUndoLabel}.`
            );
          }
          break;
        case 'undo-status':
          showMsg(
            formatUndoQueueStatus(deletedSavedRunStack, maxDeletedSavedRunUndos)
          );
          break;
        case 'trace':
          if (!hasTrace) {
            showMsg('No trace yet. Run a task first.');
          } else {
            setView('trace');
          }
          break;
        case 'back':
          setView('swarm');
          break;
        case 'retry': {
          const retryEntry =
            view === 'result' && activeResult
              ? activeResult
              : resultLog[resultLog.length - 1];
          if (!retryEntry) {
            showMsg('No previous task to retry.');
            break;
          }
          if (blockInteractiveTaskSubmission()) {
            break;
          }
          await runTask(retryEntry.task);
          break;
        }
        case 'exit':
        case 'quit':
          exit();
          break;
        case 'clear':
          setResultLog([]);
          setHistoryMode('history');
          setPromptHistory([]);
          setPendingDeleteCommand(null);
          setPendingDropOldestUndoCommand(null);
          setDeletedSavedRunStack([]);
          reset();
          setCurrentTask('');
          setActiveResultId(null);
          setResumeEntryId(null);
          onClearHistory?.();
          showMsg('Session cleared.');
          break;
        case 'help':
          showMsg(
            helpCommands.map((c) => `/${c.name}  ${c.description}`).join('   ')
          );
          break;
        default:
          showMsg(`Unknown command: /${name}. Type /help for commands.`);
      }
    },
    [
      exit,
      reset,
      profiles,
      showMsg,
      showStatus,
      syncDaemonWarning,
      resultLog,
      activeResult,
      view,
      hasTrace,
      clearResumeAssist,
      deletedSavedRunStack,
      moveResumeAssistSelection,
      pendingDeleteCommand,
      pendingDropOldestUndoCommand,
      renameSavedRun,
      deleteSavedRun,
      undoDeleteSavedRun,
      dropOldestDeletedSavedRun,
      helpCommands,
      oldestUndoLabel,
      openResultEntry,
      blockInteractiveTaskSubmission,
      runTask,
      onClearHistory,
    ]
  );

  const handleTaskSubmit = useCallback(
    async (input: string) => {
      if (input.startsWith('/')) {
        rememberPrompt(input);
        await handleSlashCommand(input);
        return;
      }
      if (blockInteractiveTaskSubmission()) {
        return;
      }
      await runTask(input);
    },
    [
      blockInteractiveTaskSubmission,
      handleSlashCommand,
      rememberPrompt,
      runTask,
    ]
  );

  const handleSaveAgent = useCallback(
    (profile: AgentProfile) => {
      setProfiles((prev) =>
        prev.map((p) => (p.name === profile.name ? profile : p))
      );
      onSaveAgent?.(profile);
    },
    [onSaveAgent]
  );

  const inputSuggestions = useCallback(
    (value: string) => [
      ...buildSavedRunLabelInputSuggestions(resultLog, value, 'resume'),
      ...buildSavedRunLabelInputSuggestions(resultLog, value, 'rename'),
      ...buildSavedRunLabelInputSuggestions(resultLog, value, 'delete'),
    ],
    [resultLog]
  );

  // ── Agents view ──
  if (view === 'agents') {
    return (
      <Box flexDirection="column">
        <AgentsPanel
          profiles={profiles}
          onBack={() => setView('swarm')}
          onSave={handleSaveAgent}
        />
      </Box>
    );
  }

  // ── History view ──
  if (view === 'history') {
    return (
      <Box flexDirection="column">
        <HistoryView
          results={displayedHistoryResults}
          title={historyMode === 'resume' ? 'Resume' : 'History'}
          selectActionLabel={historyMode === 'resume' ? 'resume' : 'open'}
          initialSelection={historyMode === 'resume' ? 'first' : 'last'}
          onBack={() => setView('swarm')}
          onSelect={
            interactive
              ? (entry: ResultEntry) => {
                  openResultEntry(entry, historyMode === 'resume');
                }
              : undefined
          }
          onRetry={
            interactive && !taskEntryBlockedByDaemon
              ? async (entry: ResultEntry) => {
                  if (blockInteractiveTaskSubmission()) {
                    return;
                  }
                  await runTask(entry.task);
                }
              : undefined
          }
          onDelete={
            interactive && historyMode === 'resume'
              ? (entry: ResultEntry) => {
                  deleteSavedRun(entry.id);
                }
              : undefined
          }
          onUndoDelete={
            interactive &&
            historyMode === 'resume' &&
            deletedSavedRunStack.length > 0
              ? () => {
                  undoDeleteSavedRun();
                }
              : undefined
          }
          onDropOldestUndo={
            interactive &&
            historyMode === 'resume' &&
            deletedSavedRunStack.length > 0
              ? () => {
                  dropOldestDeletedSavedRun();
                }
              : undefined
          }
          undoDeleteLabel={
            historyMode === 'resume' && deletedSavedRunStack.length > 0
              ? deletedSavedRunStack[
                  deletedSavedRunStack.length - 1
                ]?.entry.label?.trim() ||
                deletedSavedRunStack[deletedSavedRunStack.length - 1]?.entry
                  .task
              : undefined
          }
          dropOldestUndoLabel={
            historyMode === 'resume' && deletedSavedRunStack.length > 0
              ? oldestUndoLabel
              : undefined
          }
          undoDeleteCount={
            historyMode === 'resume' ? deletedSavedRunStack.length : undefined
          }
        />
        {systemMsg ? (
          <Box paddingX={2}>
            <Text color="cyan">{systemMsg}</Text>
          </Box>
        ) : null}
      </Box>
    );
  }

  // ── Trace view ──
  if (view === 'trace') {
    return (
      <Box flexDirection="column">
        <TraceView
          messages={messages}
          tools={tools}
          onBack={() => setView('swarm')}
        />
      </Box>
    );
  }

  // ── Result view ──
  if (view === 'result' && activeResult) {
    const hint =
      interactive && resumeEntryId === activeResult.id
        ? resumeEntry?.id === activeResult.id
          ? taskEntryBlockedByDaemon
            ? 'Resumed last run. Retry unavailable while daemon is down. Use /health to recheck or /back to return.'
            : 'Resumed last run. Type /retry to run it again or /back to return.'
          : taskEntryBlockedByDaemon
          ? 'Resumed saved run. Retry unavailable while daemon is down. Use /health to recheck or /back to return.'
          : 'Resumed saved run. Type /retry to run it again or /back to return.'
        : undefined;
    const note =
      interactive && !activeResult.label
        ? 'Type /rename <label> to name this saved run.'
        : undefined;
    const pendingDeleteNotice =
      pendingDeleteCommand?.targetId === activeResult.id
        ? `Repeat ${pendingDeleteCommand.confirmationCommand} to remove ${pendingDeleteCommand.label}. Any other command cancels.`
        : undefined;
    return (
      <Box flexDirection="column">
        <ResultView
          entry={activeResult}
          onBack={() => setView('swarm')}
          hint={hint}
          note={note}
          pendingDeleteNotice={pendingDeleteNotice}
        />
        {systemMsg ? (
          <Box paddingX={2}>
            <Text color="cyan">{systemMsg}</Text>
          </Box>
        ) : null}
        <InputBar
          onSubmit={handleTaskSubmit}
          disabled={false}
          commandOnly={taskEntryBlockedByDaemon}
          commandOnlyHint={TASK_INPUT_COMMAND_ONLY_HINT}
          commandOnlyHelperText={TASK_INPUT_COMMAND_ONLY_HELPER}
          history={promptHistory}
          suggestions={inputSuggestions}
          commands={resultInputCommands}
        />
        {daemonRecoveryNotice ? (
          <Box paddingX={2}>
            <Text color="green" bold>
              {daemonRecoveryNotice}
            </Text>
          </Box>
        ) : null}
      </Box>
    );
  }

  // ── Swarm view ──
  const headerTask = currentTask || (interactive ? 'interactive' : '');

  return (
    <Box flexDirection="column">
      <Header
        strategy={strategy}
        agentCount={visibleAgentCount}
        activeAgentCount={stats.agentCount}
        task={headerTask}
      />
      {daemonWarning ? (
        <Box paddingX={1}>
          <Text color="yellow">{daemonWarning}</Text>
        </Box>
      ) : null}
      <AgentPanel agents={displayedAgents} />
      <MessageStream messages={messages} />
      <ToolPanel tools={tools} />

      {!interactive && done && result ? (
        <Box paddingX={1}>
          <Text color="green" bold>
            Result: {result}
          </Text>
        </Box>
      ) : null}
      {!interactive && done && error ? (
        <Box paddingX={1}>
          <Text color="red" bold>
            Error: {error}
          </Text>
        </Box>
      ) : null}

      {!interactive && (done || error) ? (
        <Box paddingX={1}>
          <Text color="gray">
            {[
              hasTrace ? 't trace' : null,
              resultLog.length > 0 ? 'h history' : null,
              onTask && currentTask ? 'r retry' : null,
              'Enter/Esc/q exit',
            ]
              .filter(Boolean)
              .join('  ·  ')}
          </Text>
        </Box>
      ) : null}

      {interactive ? (
        <ResultLog
          results={resultLog}
          showRetryHint={!taskEntryBlockedByDaemon}
        />
      ) : null}

      <StatusBar
        stats={stats}
        configuredAgentCount={configuredAgentCount}
        daemonStatus={daemonStatus}
        daemonCheckedAt={lastDaemonCheckAt}
        done={interactive ? phase === 'waiting' && resultLog.length > 0 : done}
      />

      {interactive && phase === 'waiting' && resultLog.length > 0 ? (
        <Box paddingX={1}>
          <Text color="gray">Press Ctrl+O to open saved runs.</Text>
        </Box>
      ) : null}

      {interactive && phase === 'waiting' && deletedSavedRunStack.length > 0 ? (
        <Box paddingX={1}>
          <Text color="gray">
            {`Undo queued: ${nextUndoLabel}${
              deletedSavedRunStack.length > 1
                ? ` (+${String(deletedSavedRunStack.length - 1)} more)`
                : ''
            }.${
              undoQueueIsFull && oldestUndoLabel
                ? ` Queue full. Next delete drops oldest: ${oldestUndoLabel}.`
                : ''
            } Use /undo or /undo-status.`}
          </Text>
        </Box>
      ) : null}

      {interactive && phase === 'waiting' && pendingDropOldestUndoLabel ? (
        <Box paddingX={1}>
          <Text color="yellow" bold>
            Undo discard armed.
          </Text>
          <Text color="yellow">
            {` Repeat /undo-drop to discard ${pendingDropOldestUndoLabel}. Any other command cancels.`}
          </Text>
        </Box>
      ) : null}

      {interactive && phase === 'running' ? (
        <Box paddingX={1}>
          <Text color="gray">
            Press s for status while the swarm is running.
          </Text>
        </Box>
      ) : null}

      {systemMsg ? (
        <Box paddingX={2}>
          <Text color="cyan">{systemMsg}</Text>
        </Box>
      ) : null}

      {resumeAssist && view === 'swarm' ? (
        <Box
          flexDirection="column"
          borderStyle="single"
          paddingX={1}
          marginX={1}
        >
          <Text bold color="yellow">
            {resumeAssist.kind === 'matches'
              ? `Saved run matches for "${resumeAssist.query}"`
              : `Closest saved runs for "${resumeAssist.query}"`}
          </Text>
          {resumeAssist.entries.map((entry, idx) => {
            const active = idx === resumeAssist.selectedIdx;
            return (
              <Box key={entry.id} flexDirection="column" marginTop={1}>
                <Text color={active ? 'magenta' : 'gray'} bold={active}>
                  {active ? '❯ ' : '  '}
                  {entry.label}
                </Text>
                <Text color="gray">
                  {'  '}
                  {entry.task}
                </Text>
              </Box>
            );
          })}
          <Box marginTop={1}>
            <Text color="gray" dimColor>
              Use /resume {'<label>'}, Ctrl+N/Ctrl+P to move, Ctrl+Y to open, or
              Ctrl+O to browse all saved runs.
            </Text>
          </Box>
        </Box>
      ) : null}

      {daemonRecoveryNotice ? (
        <Box paddingX={2}>
          <Text color="green" bold>
            {daemonRecoveryNotice}
          </Text>
        </Box>
      ) : null}

      {interactive ? (
        <InputBar
          onSubmit={handleTaskSubmit}
          disabled={phase === 'running'}
          commandOnly={taskEntryBlockedByDaemon}
          commandOnlyHint={TASK_INPUT_COMMAND_ONLY_HINT}
          commandOnlyHelperText={TASK_INPUT_COMMAND_ONLY_HELPER}
          history={promptHistory}
          suggestions={inputSuggestions}
          commands={swarmInputCommands}
        />
      ) : null}
    </Box>
  );
}
