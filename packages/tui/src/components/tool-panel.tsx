import React from "react"
import { Box, Text } from "ink"
import type { ToolEntry } from "../types.js"

export interface ToolPanelProps {
  tools: ToolEntry[]
  maxVisible?: number
}

function argsPreview(args: Record<string, unknown>): string {
  const str = JSON.stringify(args)
  return str.length > 40 ? str.slice(0, 37) + "..." : str
}

function statusIcon(status: ToolEntry["status"]): string {
  switch (status) {
    case "running":
      return "..."
    case "success":
      return "ok"
    case "error":
      return "err"
  }
}

export function ToolPanel({
  tools,
  maxVisible = 5,
}: ToolPanelProps): React.ReactElement | null {
  if (tools.length === 0) {
    return null
  }

  const visible = tools.slice(-maxVisible)

  return (
    <Box flexDirection="column" paddingX={1}>
      <Text dimColor>-- tools --</Text>
      {visible.map((tool) => (
        <Box key={tool.id}>
          <Text>
            <Text>[{statusIcon(tool.status)}]</Text>{" "}
            <Text>{tool.agentId.slice(0, 8)}: </Text>
            <Text bold>
              {tool.toolName}({argsPreview(tool.args)})
            </Text>
            {tool.durationMs != null ? (
              <Text dimColor> {tool.durationMs}ms</Text>
            ) : null}
          </Text>
        </Box>
      ))}
    </Box>
  )
}
