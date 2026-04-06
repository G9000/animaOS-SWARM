import React, { useState, useCallback, useRef } from 'react';
import { Box, Text, useApp, useInput } from 'ink';
import type { IEventBus, TaskResult } from '@animaOS-SWARM/core';
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
import type { ResultEntry } from './components/result-log.js';
import type { SlashCommand } from './components/input-bar.js';
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
}

const SLASH_COMMANDS: SlashCommand[] = [
  { name: 'agents', description: 'browse and edit agents' },
  { name: 'history', description: 'browse past runs' },
  { name: 'resume', description: 'browse saved runs or resume by label' },
  { name: 'rename', description: 'name the current saved run' },
  { name: 'trace', description: 'inspect messages and tool activity' },
  { name: 'result', description: 'view the full last result' },
  { name: 'status', description: 'show current session state' },
  { name: 'retry', description: 'rerun the last task' },
  { name: 'help', description: 'show available commands' },
  { name: 'clear', description: 'clear session history' },
  { name: 'exit', description: 'exit the session' },
];

type AppView = 'swarm' | 'agents' | 'history' | 'trace' | 'result';
type HistoryMode = 'history' | 'resume';
const MAX_PROMPT_HISTORY = 100;

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
}: AppProps): React.ReactElement {
  const { exit } = useApp();
  const { agents, messages, tools, stats, done, result, error, reset } =
    useEventLog({
      eventBus,
      strategy,
    });
  const resumeEntry = resumeLastResult
    ? initialResults[initialResults.length - 1]
    : undefined;

  const [view, setView] = useState<AppView>(resumeEntry ? 'result' : 'swarm');
  const [phase, setPhase] = useState<'waiting' | 'running'>('waiting');
  const [currentTask, setCurrentTask] = useState(
    task ?? resumeEntry?.task ?? ''
  );
  const [resultLog, setResultLog] = useState<ResultEntry[]>(initialResults);
  const [historyMode, setHistoryMode] = useState<HistoryMode>('history');
  const [promptHistory, setPromptHistory] = useState<string[]>(
    initialResults.map((entry) => entry.task).slice(-MAX_PROMPT_HISTORY)
  );
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
  const displayedHistoryResults =
    historyMode === 'resume' ? sortResumeEntries(resultLog) : resultLog;
  const canExitOneShot =
    !interactive && view === 'swarm' && (done || Boolean(error));

  const showMsg = useCallback((msg: string) => {
    setSystemMsg(msg);
    setTimeout(() => setSystemMsg(null), 3000);
  }, []);

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
        `agents ${stats.agentCount}`,
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
    stats.agentCount,
    tools.length,
    showMsg,
  ]);

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

  const openResultEntry = useCallback((entry: ResultEntry, resume = false) => {
    setCurrentTask(entry.task);
    setActiveResultId(entry.id);
    setResumeEntryId(resume ? entry.id : null);
    setView('result');
  }, []);

  const rememberPrompt = useCallback((input: string) => {
    setPromptHistory((prev) => [...prev, input].slice(-MAX_PROMPT_HISTORY));
  }, []);

  const runTask = useCallback(
    async (input: string) => {
      if (!onTask) {
        return;
      }

      rememberPrompt(input);
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
    [onTask, rememberPrompt, reset, onResultRecorded]
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

      setHistoryMode('resume');
      setView('history');
    },
    { isActive: interactive && phase === 'waiting' && view === 'swarm' }
  );

  const handleSlashCommand = useCallback(
    async (cmd: string) => {
      const [name, ...rest] = cmd.slice(1).trim().split(/\s+/);
      const args = rest.join(' ').trim();
      switch (name.toLowerCase()) {
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
              showMsg(
                `No saved run named "${args}". Type /resume to browse saved runs.`
              );
              break;
            }

            if (matchedEntries.length > 1) {
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
          reset();
          setCurrentTask('');
          setActiveResultId(null);
          setResumeEntryId(null);
          onClearHistory?.();
          showMsg('Session cleared.');
          break;
        case 'help':
          showMsg(
            SLASH_COMMANDS.map((c) => `/${c.name}  ${c.description}`).join(
              '   '
            )
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
      resultLog,
      activeResult,
      view,
      hasTrace,
      renameSavedRun,
      openResultEntry,
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
      await runTask(input);
    },
    [handleSlashCommand, rememberPrompt, runTask]
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
            interactive
              ? async (entry: ResultEntry) => {
                  await runTask(entry.task);
                }
              : undefined
          }
        />
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
          ? 'Resumed last run. Type /retry to run it again or /back to return.'
          : 'Resumed saved run. Type /retry to run it again or /back to return.'
        : undefined;
    const note =
      interactive && !activeResult.label
        ? 'Type /rename <label> to name this saved run.'
        : undefined;
    return (
      <Box flexDirection="column">
        <ResultView
          entry={activeResult}
          onBack={() => setView('swarm')}
          hint={hint}
          note={note}
        />
        {systemMsg ? (
          <Box paddingX={2}>
            <Text color="cyan">{systemMsg}</Text>
          </Box>
        ) : null}
        <InputBar
          onSubmit={handleTaskSubmit}
          disabled={false}
          history={promptHistory}
          commands={[
            { name: 'back', description: 'return to swarm view' },
            ...(resultLog.length > 0
              ? [
                  {
                    name: 'resume',
                    description: 'browse saved runs to resume',
                  },
                  {
                    name: 'rename',
                    description: 'name the current saved run',
                    args: '<label>',
                  },
                ]
              : []),
            ...(interactive
              ? [
                  {
                    name: 'retry',
                    description: 'rerun the last task',
                  },
                ]
              : []),
          ]}
        />
      </Box>
    );
  }

  // ── Swarm view ──
  const headerTask = currentTask || (interactive ? 'interactive' : '');

  return (
    <Box flexDirection="column">
      <Header
        strategy={strategy}
        agentCount={stats.agentCount}
        task={headerTask}
      />
      <AgentPanel agents={agents} />
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

      {interactive ? <ResultLog results={resultLog} /> : null}

      <StatusBar
        stats={stats}
        done={interactive ? phase === 'waiting' && resultLog.length > 0 : done}
      />

      {interactive && phase === 'waiting' && resultLog.length > 0 ? (
        <Box paddingX={1}>
          <Text color="gray">Press Ctrl+O to open saved runs.</Text>
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

      {interactive ? (
        <InputBar
          onSubmit={handleTaskSubmit}
          disabled={phase === 'running'}
          history={promptHistory}
          commands={SLASH_COMMANDS}
        />
      ) : null}
    </Box>
  );
}
