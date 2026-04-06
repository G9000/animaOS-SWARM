import type { MessageEntry, ToolEntry } from './types.js';

export type TraceEntry =
  | {
      id: string;
      kind: 'message';
      timestamp: number;
      message: MessageEntry;
    }
  | {
      id: string;
      kind: 'tool';
      timestamp: number;
      tool: ToolEntry;
    };

function truncate(value: string, maxLength: number): string {
  return value.length > maxLength
    ? value.slice(0, maxLength - 3) + '...'
    : value;
}

function formatTime(timestamp: number): string {
  return new Date(timestamp).toLocaleTimeString(undefined, {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  });
}

function formatJson(value: unknown): string {
  return JSON.stringify(value, null, 2) ?? '{}';
}

function formatResult(value: string): string {
  const trimmed = value.trim();
  if (
    !(
      (trimmed.startsWith('{') && trimmed.endsWith('}')) ||
      (trimmed.startsWith('[') && trimmed.endsWith(']'))
    )
  ) {
    return value;
  }

  try {
    return formatJson(JSON.parse(trimmed));
  } catch {
    return value;
  }
}

export function buildTraceEntries(
  messages: MessageEntry[],
  tools: ToolEntry[]
): TraceEntry[] {
  return [
    ...messages.map(
      (message): TraceEntry => ({
        id: message.id,
        kind: 'message',
        timestamp: message.timestamp,
        message,
      })
    ),
    ...tools.map(
      (tool): TraceEntry => ({
        id: tool.id,
        kind: 'tool',
        timestamp: tool.timestamp,
        tool,
      })
    ),
  ].sort((left, right) => {
    if (left.timestamp !== right.timestamp) {
      return left.timestamp - right.timestamp;
    }

    return left.id.localeCompare(right.id);
  });
}

export function traceEntrySummary(entry: TraceEntry): string {
  if (entry.kind === 'message') {
    const { from, to, content } = entry.message;
    return `${from} -> ${to}  ${truncate(content, 60)}`;
  }

  const { agentName, toolName, args, status, durationMs } = entry.tool;
  const statusLabel =
    status === 'running' ? '...' : status === 'success' ? 'ok' : 'err';
  const duration = durationMs != null ? ` ${durationMs}ms` : '';
  return `${agentName} [${statusLabel}] ${toolName}(${truncate(
    JSON.stringify(args) ?? '{}',
    32
  )})${duration}`;
}

export function traceEntryDetail(entry: TraceEntry): string {
  if (entry.kind === 'message') {
    const { from, to, content, timestamp } = entry.message;
    return [
      `Type: message`,
      `Time: ${formatTime(timestamp)}`,
      `From: ${from}`,
      `To: ${to}`,
      '',
      content,
    ].join('\n');
  }

  const { agentName, toolName, args, status, durationMs, timestamp, result } =
    entry.tool;
  return [
    `Type: tool`,
    `Time: ${formatTime(timestamp)}`,
    `Agent: ${agentName}`,
    `Tool: ${toolName}`,
    `Status: ${status}`,
    `Duration: ${durationMs ?? 0}ms`,
    '',
    'Args:',
    formatJson(args),
    ...(result
      ? ['', 'Result:', formatResult(result)]
      : ['', 'Result:', '(not available in the streamed event payload)']),
  ].join('\n');
}
