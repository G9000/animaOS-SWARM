import React from 'react';
import { Box, Text } from 'ink';

export interface HeaderProps {
  strategy: string;
  agentCount: number;
  activeAgentCount?: number;
  task: string;
}

export function Header({
  strategy,
  agentCount,
  activeAgentCount,
  task,
}: HeaderProps): React.ReactElement {
  const truncatedTask = task.length > 60 ? task.slice(0, 57) + '...' : task;
  const agentLabel =
    typeof activeAgentCount === 'number' && activeAgentCount !== agentCount
      ? `${agentCount} agents (${activeAgentCount} active)`
      : `${agentCount} agents`;

  return (
    <Box borderStyle="single" paddingX={1}>
      <Text bold>
        SWARM — {strategy} — {agentLabel} — {truncatedTask}
      </Text>
    </Box>
  );
}
