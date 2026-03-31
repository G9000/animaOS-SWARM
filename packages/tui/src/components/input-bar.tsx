import React, { useState } from "react"
import { Box, Text, useInput } from "ink"

export interface SlashCommand {
  name: string
  description: string
  args?: string
}

export interface InputBarProps {
  onSubmit: (value: string) => void
  disabled?: boolean
  placeholder?: string
  commands?: SlashCommand[]
}

export function InputBar({
  onSubmit,
  disabled = false,
  placeholder = "type your task... or /help for commands",
  commands = [],
}: InputBarProps): React.ReactElement {
  const [value, setValue] = useState("")
  const [selectedIdx, setSelectedIdx] = useState(0)

  const isSlash = value.startsWith("/")
  const matches = isSlash
    ? commands.filter((c) => `/${c.name}`.startsWith(value.toLowerCase()))
    : []

  // Keep selectedIdx in bounds whenever matches change
  const clampedIdx = matches.length > 0 ? Math.min(selectedIdx, matches.length - 1) : 0

  useInput(
    (input, key) => {
      if (matches.length > 0) {
        if (key.upArrow) {
          setSelectedIdx((i) => Math.max(0, i - 1))
          return
        }
        if (key.downArrow) {
          setSelectedIdx((i) => Math.min(matches.length - 1, i + 1))
          return
        }
        if (key.return) {
          const cmd = matches[clampedIdx]
          if (cmd.args) {
            // Has args — autocomplete name and let user fill args
            setValue(`/${cmd.name} `)
            setSelectedIdx(0)
          } else {
            // No args — submit directly
            onSubmit(`/${cmd.name}`)
            setValue("")
            setSelectedIdx(0)
          }
          return
        }
        if (key.tab) {
          const cmd = matches[clampedIdx]
          setValue(cmd.args ? `/${cmd.name} ` : `/${cmd.name}`)
          setSelectedIdx(0)
          return
        }
      }

      if (key.return) {
        if (value.trim()) {
          onSubmit(value.trim())
          setValue("")
          setSelectedIdx(0)
        }
      } else if (key.backspace || key.delete) {
        setValue((v) => v.slice(0, -1))
        setSelectedIdx(0)
      } else if (!key.ctrl && !key.meta && !key.escape && !key.tab) {
        setValue((v) => v + input)
        setSelectedIdx(0)
      }
    },
    { isActive: !disabled },
  )

  let body: React.ReactElement
  if (disabled) {
    body = <Text color="yellow">running swarm...</Text>
  } else if (value) {
    body = (
      <Text>
        {isSlash ? <Text color="magenta">{value}</Text> : value}
        <Text color="cyan">▌</Text>
      </Text>
    )
  } else {
    body = (
      <Text>
        <Text color="gray">{placeholder}</Text>
        <Text color="cyan">▌</Text>
      </Text>
    )
  }

  return (
    <Box flexDirection="column">
      {/* Command palette */}
      {matches.length > 0 && (
        <Box flexDirection="column" paddingX={2}>
          {matches.map((cmd, i) => {
            const active = i === clampedIdx
            return (
              <Box key={cmd.name}>
                <Text color={active ? "magenta" : "gray"} bold={active}>
                  {active ? "❯ " : "  "}
                </Text>
                <Text color={active ? "magenta" : "gray"} bold={active}>
                  {"/"}{cmd.name}
                  {cmd.args ? <Text color="gray"> {cmd.args}</Text> : null}
                </Text>
                <Text color={active ? "white" : "gray"}>{"  "}{cmd.description}</Text>
              </Box>
            )
          })}
          <Text color="gray" dimColor>
            {"  ↑↓ navigate · enter select · tab complete"}
          </Text>
        </Box>
      )}

      <Box borderStyle="round" paddingX={1}>
        <Text bold color="cyan">{">"}{" "}</Text>
        {body}
      </Box>
    </Box>
  )
}
