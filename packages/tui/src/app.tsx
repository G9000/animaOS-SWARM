import React, { useState, useCallback } from "react"
import { Box, Text, useApp } from "ink"
import type { IEventBus, TaskResult } from "@animaOS-SWARM/core"
import { useEventLog } from "./hooks/use-event-log.js"
import { Header } from "./components/header.js"
import { AgentPanel } from "./components/agent-panel.js"
import { MessageStream } from "./components/message-stream.js"
import { ToolPanel } from "./components/tool-panel.js"
import { StatusBar } from "./components/status-bar.js"
import { InputBar } from "./components/input-bar.js"
import { ResultLog } from "./components/result-log.js"
import { AgentsPanel } from "./components/agents-panel.js"
import type { ResultEntry } from "./components/result-log.js"
import type { SlashCommand } from "./components/input-bar.js"
import type { AgentProfile } from "./types.js"

export interface AppProps {
  eventBus: IEventBus
  strategy: string
  /** One-shot mode task. Omit to use interactive mode. */
  task?: string
  /** Enable interactive input bar */
  interactive?: boolean
  /** Called when user submits a task in interactive mode */
  onTask?: (task: string) => Promise<TaskResult>
  /** Agent profiles for the /agents panel */
  agentProfiles?: AgentProfile[]
  /** Called when user edits and saves an agent */
  onSaveAgent?: (profile: AgentProfile) => void
}

const SLASH_COMMANDS: SlashCommand[] = [
  { name: "agents",  description: "browse and edit agents" },
  { name: "help",    description: "show available commands" },
  { name: "clear",   description: "clear session history" },
  { name: "exit",    description: "exit the session" },
]

type AppView = "swarm" | "agents"

export function App({
  eventBus,
  strategy,
  task,
  interactive = false,
  onTask,
  agentProfiles = [],
  onSaveAgent,
}: AppProps): React.ReactElement {
  const { exit } = useApp()
  const { agents, messages, tools, stats, done, result, error, reset } = useEventLog({
    eventBus,
    strategy,
  })

  const [view, setView] = useState<AppView>("swarm")
  const [phase, setPhase] = useState<"waiting" | "running">("waiting")
  const [currentTask, setCurrentTask] = useState(task ?? "")
  const [resultLog, setResultLog] = useState<ResultEntry[]>([])
  const [systemMsg, setSystemMsg] = useState<string | null>(null)
  const [profiles, setProfiles] = useState<AgentProfile[]>(agentProfiles)

  const showMsg = useCallback((msg: string) => {
    setSystemMsg(msg)
    setTimeout(() => setSystemMsg(null), 3000)
  }, [])

  const handleSlashCommand = useCallback(
    (cmd: string) => {
      const [name] = cmd.slice(1).trim().split(/\s+/)
      switch (name.toLowerCase()) {
        case "agents":
          if (profiles.length === 0) {
            showMsg("No agent profiles loaded. Create an agency first.")
          } else {
            setView("agents")
          }
          break
        case "exit":
        case "quit":
          exit()
          break
        case "clear":
          setResultLog([])
          reset()
          setCurrentTask("")
          showMsg("Session cleared.")
          break
        case "help":
          showMsg(
            SLASH_COMMANDS.map((c) => `/${c.name}  ${c.description}`).join("   "),
          )
          break
        default:
          showMsg(`Unknown command: /${name}. Type /help for commands.`)
      }
    },
    [exit, reset, profiles, showMsg],
  )

  const handleTaskSubmit = useCallback(
    async (input: string) => {
      if (input.startsWith("/")) {
        handleSlashCommand(input)
        return
      }
      if (!onTask) return
      setCurrentTask(input)
      setPhase("running")
      reset()

      const taskResult = await onTask(input)

      const text =
        taskResult.status === "success"
          ? ((taskResult.data as { text?: string })?.text ?? "completed")
          : (taskResult.error ?? "error")

      setResultLog((prev) => [
        ...prev,
        { task: input, result: text, isError: taskResult.status !== "success" },
      ])
      setPhase("waiting")
    },
    [onTask, reset, handleSlashCommand],
  )

  const handleSaveAgent = useCallback(
    (profile: AgentProfile) => {
      setProfiles((prev) => prev.map((p) => (p.name === profile.name ? profile : p)))
      onSaveAgent?.(profile)
    },
    [onSaveAgent],
  )

  // ── Agents view ──
  if (view === "agents") {
    return (
      <Box flexDirection="column">
        <AgentsPanel
          profiles={profiles}
          onBack={() => setView("swarm")}
          onSave={handleSaveAgent}
        />
      </Box>
    )
  }

  // ── Swarm view ──
  const headerTask = currentTask || (interactive ? "interactive" : "")

  return (
    <Box flexDirection="column">
      <Header strategy={strategy} agentCount={stats.agentCount} task={headerTask} />
      <AgentPanel agents={agents} />
      <MessageStream messages={messages} />
      <ToolPanel tools={tools} />

      {!interactive && done && result ? (
        <Box paddingX={1}>
          <Text color="green" bold>Result: {result}</Text>
        </Box>
      ) : null}
      {!interactive && done && error ? (
        <Box paddingX={1}>
          <Text color="red" bold>Error: {error}</Text>
        </Box>
      ) : null}

      {interactive ? <ResultLog results={resultLog} /> : null}

      <StatusBar
        stats={stats}
        done={interactive ? phase === "waiting" && resultLog.length > 0 : done}
      />

      {systemMsg ? (
        <Box paddingX={2}>
          <Text color="cyan">{systemMsg}</Text>
        </Box>
      ) : null}

      {interactive ? (
        <InputBar
          onSubmit={handleTaskSubmit}
          disabled={phase === "running"}
          commands={SLASH_COMMANDS}
        />
      ) : null}
    </Box>
  )
}
