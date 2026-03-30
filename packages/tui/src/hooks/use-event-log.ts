import { useState, useEffect, useRef, useCallback } from "react"
import type { IEventBus, Event } from "@animaOS-SWARM/core"
import type {
  AgentEntry,
  AgentDisplayStatus,
  MessageEntry,
  ToolEntry,
  SwarmStats,
} from "../types.js"

export interface UseEventLogOptions {
  eventBus: IEventBus
  strategy: string
}

export interface UseEventLogResult {
  agents: AgentEntry[]
  messages: MessageEntry[]
  tools: ToolEntry[]
  stats: SwarmStats
  done: boolean
  result: string | null
  error: string | null
}

let nextMsgId = 0
let nextToolId = 0

export function useEventLog({
  eventBus,
  strategy,
}: UseEventLogOptions): UseEventLogResult {
  const [agents, setAgents] = useState<AgentEntry[]>([])
  const [messages, setMessages] = useState<MessageEntry[]>([])
  const [tools, setTools] = useState<ToolEntry[]>([])
  const [done, setDone] = useState(false)
  const [result, setResult] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  // Use a ref for startTime so it doesn't cause re-renders
  const startTimeRef = useRef(Date.now())

  const updateAgent = useCallback(
    (
      agentId: string,
      updater: (existing: AgentEntry | undefined) => Partial<AgentEntry>,
    ) => {
      setAgents((prev) => {
        const idx = prev.findIndex((a) => a.id === agentId)
        if (idx >= 0) {
          const copy = [...prev]
          copy[idx] = { ...copy[idx], ...updater(copy[idx]) }
          return copy
        }
        // If agent doesn't exist yet, create it from the partial
        const partial = updater(undefined)
        const newAgent: AgentEntry = {
          id: agentId,
          name: partial.name ?? agentId,
          status: partial.status ?? "idle",
          tokens: partial.tokens ?? 0,
          currentTool: partial.currentTool,
        }
        return [...prev, newAgent]
      })
    },
    [],
  )

  useEffect(() => {
    const unsubs: Array<() => void> = []

    // agent:spawned
    unsubs.push(
      eventBus.on<{ agentId: string; name: string }>(
        "agent:spawned",
        (evt: Event<{ agentId: string; name: string }>) => {
          const { agentId, name } = evt.data
          updateAgent(agentId, () => ({
            name,
            status: "idle" as AgentDisplayStatus,
          }))
        },
      ),
    )

    // task:started
    unsubs.push(
      eventBus.on<{ agentId: string }>(
        "task:started",
        (evt: Event<{ agentId: string }>) => {
          updateAgent(evt.data.agentId, () => ({
            status: "thinking" as AgentDisplayStatus,
          }))
        },
      ),
    )

    // tool:before
    unsubs.push(
      eventBus.on<{
        agentId: string
        toolName: string
        args: Record<string, unknown>
      }>(
        "tool:before",
        (
          evt: Event<{
            agentId: string
            toolName: string
            args: Record<string, unknown>
          }>,
        ) => {
          const { agentId, toolName, args } = evt.data
          updateAgent(agentId, () => ({
            status: "running_tool" as AgentDisplayStatus,
            currentTool: toolName,
          }))

          const toolId = `tool-${String(nextToolId++)}`
          setAgents((prev) => {
            const agent = prev.find((a) => a.id === agentId)
            setTools((prevTools) => [
              ...prevTools,
              {
                id: toolId,
                agentId,
                agentName: agent?.name ?? agentId,
                toolName,
                args,
                status: "running",
                timestamp: Date.now(),
              },
            ])
            return prev
          })
        },
      ),
    )

    // tool:after
    unsubs.push(
      eventBus.on<{
        agentId: string
        toolName: string
        status: string
        durationMs: number
      }>(
        "tool:after",
        (
          evt: Event<{
            agentId: string
            toolName: string
            status: string
            durationMs: number
          }>,
        ) => {
          const { agentId, toolName, status, durationMs } = evt.data
          updateAgent(agentId, () => ({
            status: "thinking" as AgentDisplayStatus,
            currentTool: undefined,
          }))

          setTools((prev) => {
            // Find the most recent running tool for this agent+tool
            let idx = -1
            for (let i = prev.length - 1; i >= 0; i--) {
              const t = prev[i]
              if (
                t.agentId === agentId &&
                t.toolName === toolName &&
                t.status === "running"
              ) {
                idx = i
                break
              }
            }
            if (idx >= 0) {
              const copy = [...prev]
              copy[idx] = {
                ...copy[idx],
                status: status === "success" ? "success" : "error",
                durationMs,
              }
              return copy
            }
            return prev
          })
        },
      ),
    )

    // agent:message
    unsubs.push(
      eventBus.on<{ from: string; to: string; message: { text: string } }>(
        "agent:message",
        (
          evt: Event<{
            from: string
            to: string
            message: { text: string }
          }>,
        ) => {
          const { from, to, message } = evt.data
          const msgId = `msg-${String(nextMsgId++)}`
          setMessages((prev) => [
            ...prev,
            {
              id: msgId,
              from,
              to,
              content: message.text,
              timestamp: Date.now(),
            },
          ])
        },
      ),
    )

    // task:completed
    unsubs.push(
      eventBus.on<{
        agentId: string
        result: { data?: { text?: string } }
      }>(
        "task:completed",
        (
          evt: Event<{
            agentId: string
            result: { data?: { text?: string } }
          }>,
        ) => {
          updateAgent(evt.data.agentId, () => ({
            status: "done" as AgentDisplayStatus,
            currentTool: undefined,
          }))
        },
      ),
    )

    // task:failed
    unsubs.push(
      eventBus.on<{ agentId: string; error: string }>(
        "task:failed",
        (evt: Event<{ agentId: string; error: string }>) => {
          updateAgent(evt.data.agentId, () => ({
            status: "error" as AgentDisplayStatus,
            currentTool: undefined,
          }))
        },
      ),
    )

    // agent:terminated
    unsubs.push(
      eventBus.on<{ agentId: string }>(
        "agent:terminated",
        (evt: Event<{ agentId: string }>) => {
          updateAgent(evt.data.agentId, () => ({
            status: "done" as AgentDisplayStatus,
            currentTool: undefined,
          }))
        },
      ),
    )

    // swarm:completed
    unsubs.push(
      eventBus.on<{
        result: { status: string; data?: { text?: string }; error?: string }
      }>(
        "swarm:completed",
        (
          evt: Event<{
            result: {
              status: string
              data?: { text?: string }
              error?: string
            }
          }>,
        ) => {
          setDone(true)
          const swarmResult = evt.data.result
          if (swarmResult.error) {
            setError(swarmResult.error)
          } else if (swarmResult.data?.text) {
            setResult(swarmResult.data.text)
          } else {
            setResult(swarmResult.status)
          }
        },
      ),
    )

    return () => {
      for (const unsub of unsubs) {
        unsub()
      }
    }
  }, [eventBus, updateAgent])

  const stats: SwarmStats = {
    totalTokens: agents.reduce((sum, a) => sum + a.tokens, 0),
    totalCost: 0,
    elapsed: Math.floor((Date.now() - startTimeRef.current) / 1000),
    agentCount: agents.length,
    strategy,
  }

  return { agents, messages, tools, stats, done, result, error }
}
