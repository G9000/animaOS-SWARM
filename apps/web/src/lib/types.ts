// UI view-model types shared by the daemon client and the components.
// The daemon wire format (snake_case here, camelCase on the wire) is adapted
// into these shapes by daemon-api.ts `toAgentDetail`.

export interface TokenUsage {
  prompt_tokens: number;
  completion_tokens: number;
  total_tokens: number;
}

export interface ChatMessage {
  id: string;
  role: 'User' | 'Assistant' | 'System' | 'Tool';
  content: {
    text: string;
    metadata?: Record<string, unknown> | null;
  };
  created_at_ms: number;
}

export interface AgentDetail {
  id: string;
  name: string;
  provider: string;
  model: string;
  created_at_ms: number;
  status: 'Idle' | 'Running' | 'Completed' | 'Failed' | 'Terminated';
  token_usage: TokenUsage;
  system?: string | null;
  messages: ChatMessage[];
}

/** Curated model suggestions per provider. Users can still pick "custom…". */
export const PROVIDER_MODELS: Record<string, string[]> = {
  openai: ['gpt-4o', 'gpt-4o-mini', 'gpt-4.1', 'gpt-4.1-mini', 'o4-mini'],
  anthropic: [
    'claude-opus-4-6',
    'claude-sonnet-4-6',
    'claude-sonnet-4-5',
    'claude-haiku-4-5',
  ],
  google: ['gemini-2.5-pro', 'gemini-2.5-flash', 'gemini-2.5-flash-lite'],
  ollama: ['llama3.1', 'qwen2.5', 'mistral'],
  groq: ['llama-3.3-70b-versatile', 'llama-3.1-8b-instant', 'openai/gpt-oss-120b'],
  xai: ['grok-4', 'grok-3', 'grok-3-mini'],
  openrouter: ['openai/gpt-4o', 'anthropic/claude-sonnet-4.5', 'google/gemini-2.5-pro'],
  mistral: ['mistral-large-latest', 'mistral-medium-latest', 'mistral-small-latest'],
  together: ['meta-llama/Llama-3.3-70B-Instruct-Turbo', 'Qwen/Qwen2.5-72B-Instruct-Turbo'],
  deepseek: ['deepseek-chat', 'deepseek-reasoner'],
  fireworks: ['accounts/fireworks/models/llama-v3p3-70b-instruct'],
  perplexity: ['sonar-pro', 'sonar'],
  moonshot: ['kimi-k2-0905-preview', 'moonshot-v1-128k', 'moonshot-v1-32k'],
};
