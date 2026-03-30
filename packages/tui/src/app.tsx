import React from "react"
import { Box, Text } from "ink"
import type { IEventBus } from "@animaOS-SWARM/core"
import { useEventLog } from "./hooks/use-event-log.js"
import { Header } from "./components/header.js"
import { AgentPanel } from "./components/agent-panel.js"
import { MessageStream } from "./components/message-stream.js"
import { ToolPanel } from "./components/tool-panel.js"
import { StatusBar } from "./components/status-bar.js"

export interface AppProps {
  eventBus: IEventBus
  strategy: string
  task: string
}

export function App({ eventBus, strategy, task }: AppProps): React.ReactElement {
  const { agents, messages, tools, stats, done, result, error } = useEventLog({
    eventBus,
    strategy,
  })

  return (
    <Box flexDirection="column">
      <Header strategy={strategy} agentCount={stats.agentCount} task={task} />
      <AgentPanel agents={agents} />
      <MessageStream messages={messages} />
      <ToolPanel tools={tools} />
      {done && result ? (
        <Box paddingX={1}>
          <Text color="green" bold>
            Result: {result}
          </Text>
        </Box>
      ) : null}
      {done && error ? (
        <Box paddingX={1}>
          <Text color="red" bold>
            Error: {error}
          </Text>
        </Box>
      ) : null}
      <StatusBar stats={stats} done={done} />
    </Box>
  )
}
