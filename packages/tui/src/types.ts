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
  totalCost: number
  elapsed: number
  agentCount: number
  strategy: string
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
