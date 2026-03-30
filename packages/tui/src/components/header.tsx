import React from "react"
import { Box, Text } from "ink"

export interface HeaderProps {
  strategy: string
  agentCount: number
  task: string
}

export function Header({ strategy, agentCount, task }: HeaderProps): React.ReactElement {
  const truncatedTask = task.length > 60 ? task.slice(0, 57) + "..." : task

  return (
    <Box borderStyle="single" paddingX={1}>
      <Text bold>
        SWARM — {strategy} — {agentCount} agents — {truncatedTask}
      </Text>
    </Box>
  )
}
