import { useState, useEffect, useRef, useCallback } from 'react';
import type { IEventBus, Event } from '@animaOS-SWARM/core';
import type {
  AgentEntry,
  AgentDisplayStatus,
  MessageEntry,
  ToolEntry,
  SwarmStats,
} from '../types.js';

/** How often to advance the elapsed-seconds ticker while a run is live. */
const ELAPSED_TICK_MS = 1000;

export interface UseEventLogOptions {
  eventBus: IEventBus;
  strategy: string;
  /** Whether a task is actively running. Drives the elapsed-seconds ticker:
   * when true, a 1s interval re-renders the hook so the status bar doesn't
   * freeze between event arrivals. Optional for backwards compatibility —
   * defaults to "always tick" when undefined.
   */
  isRunning?: boolean;
  /** Forwarded from `AppProps.onWarning`. Called with a category string and
   * an opaque payload when the hook drops or skips data it cannot interpret
   * (e.g. malformed event payloads). Production wires this to nothing;
   * debug builds wire it to console.error.
   */
  onWarning?: (where: string, detail: unknown) => void;
}

export interface UseEventLogResult {
  agents: AgentEntry[];
  messages: MessageEntry[];
  tools: ToolEntry[];
  stats: SwarmStats;
  done: boolean;
  result: string | null;
  error: string | null;
  reset: () => void;
}

export function useEventLog({
  eventBus,
  strategy,
  isRunning,
  onWarning,
}: UseEventLogOptions): UseEventLogResult {
  const [agents, setAgents] = useState<AgentEntry[]>([]);
  const [messages, setMessages] = useState<MessageEntry[]>([]);
  const [tools, setTools] = useState<ToolEntry[]>([]);
  const [done, setDone] = useState(false);
  const [result, setResult] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [laggedEventCount, setLaggedEventCount] = useState(0);
  // Increments once per second while running; included in `stats` so the
  // returned object is structurally fresh on each tick and consumers see a
  // live elapsed counter without us re-deriving wall-clock time at render.
  const [, setTick] = useState(0);

  // Use a ref for startTime so it doesn't cause re-renders
  const startTimeRef = useRef(Date.now());
  // Per-hook id counters — module-level lets would leak across App instances
  // (the test harness or any future multi-pane setup would re-use ids).
  const nextMsgIdRef = useRef(0);
  const nextToolIdRef = useRef(0);

  const allocMsgId = useCallback(() => {
    const next = nextMsgIdRef.current;
    nextMsgIdRef.current = next + 1;
    return `msg-${String(next)}`;
  }, []);
  const allocToolId = useCallback(() => {
    const next = nextToolIdRef.current;
    nextToolIdRef.current = next + 1;
    return `tool-${String(next)}`;
  }, []);

  const reset = useCallback(() => {
    setAgents([]);
    setMessages([]);
    setTools([]);
    setDone(false);
    setResult(null);
    setError(null);
    setLaggedEventCount(0);
    startTimeRef.current = Date.now();
    nextMsgIdRef.current = 0;
    nextToolIdRef.current = 0;
  }, []);

  // Live elapsed ticker — only runs while a task is running so we don't burn
  // a needless setInterval on a quiet TUI. Defaults to "always tick" so older
  // callers that don't pass `isRunning` keep the old (always-on) behaviour.
  useEffect(() => {
    if (isRunning === false) {
      return;
    }
    const interval = setInterval(() => {
      setTick((value) => (value + 1) | 0);
    }, ELAPSED_TICK_MS);
    return () => clearInterval(interval);
  }, [isRunning]);

  const updateAgent = useCallback(
    (
      agentId: string,
      updater: (existing: AgentEntry | undefined) => Partial<AgentEntry>
    ) => {
      setAgents((prev) => {
        const idx = prev.findIndex((a) => a.id === agentId);
        if (idx >= 0) {
          const copy = [...prev];
          copy[idx] = { ...copy[idx], ...updater(copy[idx]) };
          return copy;
        }
        // If agent doesn't exist yet, create it from the partial
        const partial = updater(undefined);
        const newAgent: AgentEntry = {
          id: agentId,
          name: partial.name ?? agentId,
          status: partial.status ?? 'idle',
          tokens: partial.tokens ?? 0,
          currentTool: partial.currentTool,
        };
        return [...prev, newAgent];
      });
    },
    []
  );

  useEffect(() => {
    const unsubs: Array<() => void> = [];

    // agent:spawned
    unsubs.push(
      eventBus.on<{ agentId: string; name: string }>(
        'agent:spawned',
        (evt: Event<{ agentId: string; name: string }>) => {
          const { agentId, name } = evt.data;
          updateAgent(agentId, () => ({
            name,
            status: 'idle' as AgentDisplayStatus,
          }));
        }
      )
    );

    // task:started
    unsubs.push(
      eventBus.on<{ agentId: string }>(
        'task:started',
        (evt: Event<{ agentId: string }>) => {
          updateAgent(evt.data.agentId, () => ({
            status: 'thinking' as AgentDisplayStatus,
          }));
        }
      )
    );

    // tool:before
    unsubs.push(
      eventBus.on<{
        agentId: string;
        toolName: string;
        args: Record<string, unknown>;
      }>(
        'tool:before',
        (
          evt: Event<{
            agentId: string;
            toolName: string;
            args: Record<string, unknown>;
          }>
        ) => {
          const { agentId, toolName, args } = evt.data;
          updateAgent(agentId, () => ({
            status: 'running_tool' as AgentDisplayStatus,
            currentTool: toolName,
          }));

          const toolId = allocToolId();
          setAgents((prev) => {
            const agent = prev.find((a) => a.id === agentId);
            setTools((prevTools) => [
              ...prevTools,
              {
                id: toolId,
                agentId,
                agentName: agent?.name ?? agentId,
                toolName,
                args,
                status: 'running',
                timestamp: Date.now(),
              },
            ]);
            return prev;
          });
        }
      )
    );

    // tool:after
    unsubs.push(
      eventBus.on<{
        agentId: string;
        toolName: string;
        status: string;
        durationMs: number;
        result?: string;
      }>(
        'tool:after',
        (
          evt: Event<{
            agentId: string;
            toolName: string;
            status: string;
            durationMs: number;
            result?: string;
          }>
        ) => {
          const { agentId, toolName, status, durationMs, result } = evt.data;
          updateAgent(agentId, () => ({
            status: 'thinking' as AgentDisplayStatus,
            currentTool: undefined,
          }));

          setTools((prev) => {
            // Find the most recent running tool for this agent+tool
            let idx = -1;
            for (let i = prev.length - 1; i >= 0; i--) {
              const t = prev[i];
              if (
                t.agentId === agentId &&
                t.toolName === toolName &&
                t.status === 'running'
              ) {
                idx = i;
                break;
              }
            }
            if (idx >= 0) {
              const copy = [...prev];
              copy[idx] = {
                ...copy[idx],
                status: status === 'success' ? 'success' : 'error',
                durationMs,
                result,
              };
              return copy;
            }
            return prev;
          });
        }
      )
    );

    // agent:message
    unsubs.push(
      eventBus.on<{ from: string; to: string; message: { text: string } }>(
        'agent:message',
        (
          evt: Event<{
            from: string;
            to: string;
            message: { text: string };
          }>
        ) => {
          const { from, to, message } = evt.data;
          const msgId = allocMsgId();
          setMessages((prev) => [
            ...prev,
            {
              id: msgId,
              from,
              to,
              content: message.text,
              timestamp: Date.now(),
            },
          ]);
        }
      )
    );

    // task:completed
    unsubs.push(
      eventBus.on<{
        agentId: string;
        result: { data?: { text?: string } };
      }>(
        'task:completed',
        (
          evt: Event<{
            agentId: string;
            result: { data?: { text?: string } };
          }>
        ) => {
          updateAgent(evt.data.agentId, () => ({
            status: 'done' as AgentDisplayStatus,
            currentTool: undefined,
          }));
        }
      )
    );

    // task:failed
    unsubs.push(
      eventBus.on<{ agentId: string; error: string }>(
        'task:failed',
        (evt: Event<{ agentId: string; error: string }>) => {
          updateAgent(evt.data.agentId, () => ({
            status: 'error' as AgentDisplayStatus,
            currentTool: undefined,
          }));
        }
      )
    );

    // agent:tokens
    unsubs.push(
      eventBus.on<{ agentId: string; usage: { totalTokens: number } }>(
        'agent:tokens',
        (evt: Event<{ agentId: string; usage: { totalTokens: number } }>) => {
          const { agentId, usage } = evt.data;
          setAgents((prev) =>
            prev.map((a) =>
              a.id === agentId ? { ...a, tokens: usage.totalTokens } : a
            )
          );
        }
      )
    );

    // agent:terminated
    unsubs.push(
      eventBus.on<{ agentId: string }>(
        'agent:terminated',
        (evt: Event<{ agentId: string }>) => {
          updateAgent(evt.data.agentId, () => ({
            status: 'done' as AgentDisplayStatus,
            currentTool: undefined,
          }));
        }
      )
    );

    // swarm:completed
    unsubs.push(
      eventBus.on<{
        result: { status: string; data?: { text?: string }; error?: string };
      }>(
        'swarm:completed',
        (
          evt: Event<{
            result: {
              status: string;
              data?: { text?: string };
              error?: string;
            };
          }>
        ) => {
          setDone(true);
          const swarmResult = evt.data.result;
          if (swarmResult.error) {
            setError(swarmResult.error);
          } else if (swarmResult.data?.text) {
            setResult(swarmResult.data.text);
          } else {
            setResult(swarmResult.status);
          }
        }
      )
    );

    // swarm:lagged — synthetic event the daemon emits when a SSE consumer
    // falls behind the broadcast buffer. Without surfacing it, the trace
    // would silently miss events.
    unsubs.push(
      eventBus.on<{ missed: number }>(
        'swarm:lagged',
        (evt: Event<{ missed: number }>) => {
          const missed =
            typeof evt.data?.missed === 'number' && evt.data.missed >= 0
              ? Math.floor(evt.data.missed)
              : 0;
          if (missed === 0) {
            onWarning?.('useEventLog.swarm:lagged.payload', evt.data);
            return;
          }
          setLaggedEventCount((count) => count + missed);
          const msgId = allocMsgId();
          setMessages((prev) => [
            ...prev,
            {
              id: msgId,
              from: 'system',
              to: 'system',
              content: `missed ${missed} event${
                missed === 1 ? '' : 's'
              } — trace may have gaps; refresh for the canonical state`,
              timestamp: Date.now(),
              kind: 'gap',
            },
          ]);
        }
      )
    );

    return () => {
      for (const unsub of unsubs) {
        unsub();
      }
    };
  }, [allocMsgId, allocToolId, eventBus, onWarning, updateAgent]);

  const stats: SwarmStats = {
    totalTokens: agents.reduce((sum, a) => sum + a.tokens, 0),
    elapsed: Math.floor((Date.now() - startTimeRef.current) / 1000),
    agentCount: agents.length,
    strategy,
    laggedEventCount,
  };

  return { agents, messages, tools, stats, done, result, error, reset };
}
