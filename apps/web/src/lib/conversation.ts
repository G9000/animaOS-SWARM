import type { AgentDetail } from './types';

export function conversationMarkdown(agent: AgentDetail): string {
  return `# ${agent.name.replace(/[\r\n]/g, ' ')} — conversation\n\nExport of messages currently loaded in this workspace.\n\n${agent.messages
    .map((message) => {
      const time = new Date(message.created_at_ms);
      return `## ${message.role} · ${Number.isNaN(time.getTime()) ? 'Unknown time' : time.toISOString()}\n\n${message.content.text}\n`;
    })
    .join('\n---\n\n')}`;
}

export function conversationFilename(name: string): string {
  const stem = name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-|-$/g, '')
    .slice(0, 80);
  return `${stem || 'workspace'}-conversation.md`;
}
