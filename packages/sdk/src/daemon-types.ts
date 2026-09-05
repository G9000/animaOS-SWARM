import type { AgentStatus, Attachment, TokenUsage } from '@animaOS-SWARM/core';

/** JSON returned by the daemon; distinct from in-process runtime objects. */
export interface DaemonContent {
  text: string;
  attachments: Attachment[] | null;
  metadata: Record<string, unknown> | null;
}

export interface DaemonTaskResult<T = DaemonContent> {
  status: 'success' | 'error';
  data: T | null;
  error: string | null;
  durationMs: number;
}

export interface DaemonToolDescriptor {
  name: string;
  description: string;
  parameters: Record<string, unknown>;
  examples: Array<{
    input: string;
    args: Record<string, unknown>;
    output: string;
  }> | null;
}

export interface DaemonPluginDescriptor {
  name: string;
  description: string;
}

export interface DaemonAgentSettings {
  temperature: number | null;
  maxTokens: number | null;
  timeoutMs: number | null;
  maxRetries: number | null;
  maxToolIterations: number | null;
  additional: Record<string, unknown>;
}

export interface DaemonAgentConfig {
  name: string;
  model: string;
  bio: string | null;
  lore: string | null;
  knowledge: string[] | null;
  topics: string[] | null;
  adjectives: string[] | null;
  style: string | null;
  provider: string | null;
  system: string | null;
  tools: DaemonToolDescriptor[] | null;
  plugins: DaemonPluginDescriptor[] | null;
  settings: DaemonAgentSettings | null;
}

export interface DaemonAgentState {
  id: string;
  name: string;
  status: AgentStatus;
  config: DaemonAgentConfig;
  createdAtMs: number;
  tokenUsage: TokenUsage;
}

export interface DaemonAgentMessage {
  id: string;
  agentId: string;
  roomId: string;
  content: DaemonContent;
  role: 'user' | 'assistant' | 'system' | 'tool';
  createdAtMs: number;
}
