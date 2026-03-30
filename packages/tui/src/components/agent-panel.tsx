import React from "react"
import { Box, Text } from "ink"
import type { AgentEntry, AgentDisplayStatus } from "../types.js"

export interface AgentPanelProps {
  agents: AgentEntry[]
}

const STATUS_COLORS: Record<AgentDisplayStatus, string> = {
  idle: "gray",
  thinking: "yellow",
  running_tool: "blue",
  done: "green",
  error: "red",
}

export function AgentPanel({ agents }: AgentPanelProps): React.ReactElement {
  if (agents.length === 0) {
    return (
      <Box paddingX={1}>
        <Text dimColor>Waiting for agents to spawn...</Text>
      </Box>
    )
  }

  return (
    <Box flexDirection="column" paddingX={1}>
      {agents.map((agent) => (
        <Box key={agent.id}>
          <Text>
            <Text bold>[{agent.name}]</Text>{" "}
            <Text color={STATUS_COLORS[agent.status]}>{agent.status}</Text>
            {agent.status === "running_tool" && agent.currentTool ? (
              <Text color="blue"> ({agent.currentTool})</Text>
            ) : null}
            <Text dimColor> tokens: {agent.tokens}</Text>
          </Text>
        </Box>
      ))}
    </Box>
  )
}
