import React from "react"
import { Box, Text } from "ink"
import type { MessageEntry } from "../types.js"

export interface MessageStreamProps {
  messages: MessageEntry[]
  maxVisible?: number
}

export function MessageStream({
  messages,
  maxVisible = 10,
}: MessageStreamProps): React.ReactElement {
  const visible = messages.slice(-maxVisible)

  return (
    <Box flexDirection="column" paddingX={1}>
      <Text dimColor>-- messages --</Text>
      {visible.length === 0 ? (
        <Text dimColor>No messages yet</Text>
      ) : (
        visible.map((msg) => (
          <Box key={msg.id}>
            <Text>
              <Text bold>{msg.from.slice(0, 8)}</Text>
              <Text dimColor> → </Text>
              <Text bold>{msg.to.slice(0, 8)}</Text>
              <Text>: {msg.content}</Text>
            </Text>
          </Box>
        ))
      )}
    </Box>
  )
}
