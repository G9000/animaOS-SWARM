import React, { useState, useEffect, useRef } from "react"
import { Box, Text } from "ink"
import type { SwarmStats } from "../types.js"

export interface StatusBarProps {
  stats: SwarmStats
  done: boolean
}

export function StatusBar({ stats, done }: StatusBarProps): React.ReactElement {
  const [elapsed, setElapsed] = useState(0)
  const startRef = useRef(Date.now())

  useEffect(() => {
    if (done) return

    const interval = setInterval(() => {
      setElapsed(Math.floor((Date.now() - startRef.current) / 1000))
    }, 1000)

    return () => {
      clearInterval(interval)
    }
  }, [done])

  const displayElapsed = done ? elapsed : elapsed

  return (
    <Box borderStyle="single" paddingX={1} justifyContent="space-between">
      <Text>tokens: {stats.totalTokens}</Text>
      <Text>cost: ${stats.totalCost.toFixed(4)}</Text>
      <Text>elapsed: {displayElapsed}s</Text>
      <Text>agents: {stats.agentCount}</Text>
    </Box>
  )
}
