/** Agent status as tracked by the TUI */
export type AgentDisplayStatus = "idle" | "thinking" | "running_tool" | "done" | "error"

/** An agent row in the agent panel */
export interface AgentEntry {
  id: string
  name: string
  status: AgentDisplayStatus
  tokens: number
  currentTool?: string
}

/** A message row in the message stream */
export interface MessageEntry {
  id: string
  from: string
  to: string
  content: string
  timestamp: number
  /** Optional kind tag — `gap` is reserved for synthetic event-bus gap markers
   * (e.g. when the daemon emits a `swarm:lagged` event because an SSE consumer
   * fell behind the broadcast buffer). `system` and `agent` are the defaults.
   */
  kind?: "agent" | "system" | "gap"
}

/** A tool call row in the tool panel */
export interface ToolEntry {
  id: string
  agentId: string
  agentName: string
  toolName: string
  args: Record<string, unknown>
  status: "running" | "success" | "error"
  result?: string
  durationMs?: number
  timestamp: number
}

/** Aggregated swarm stats for the status bar */
export interface SwarmStats {
  totalTokens: number
  /** Wall-clock seconds since the run started. Drives a live ticker while a
   * task is running so the status bar doesn't freeze between events. */
  elapsed: number
  agentCount: number
  strategy: string
  /** Number of `swarm:lagged` events observed during this run. Non-zero means
   * the trace has gaps — surface in the status bar so users know to refresh. */
  laggedEventCount: number
}

/** An agent's full profile — used for the agents panel and editing */
export interface AgentProfile {
  name: string
  role?: "orchestrator" | "worker"
  bio?: string
  lore?: string
  adjectives?: string[]
  topics?: string[]
  knowledge?: string[]
  style?: string
  system?: string
}
