export type ProviderEnvConfig = {
  keyEnv: readonly string[];
  urlEnv: readonly string[];
  defaultBaseUrl: string;
};

export interface ResolvedProviderConfig {
  provider: string;
  apiKey?: string;
  baseUrl?: string;
  defaultBaseUrl: string;
}

const DEFAULT_OPENAI_BASE_URL = 'https://api.openai.com/v1';
const DEFAULT_ANTHROPIC_BASE_URL = 'https://api.anthropic.com';
const DEFAULT_GOOGLE_BASE_URL = 'https://generativelanguage.googleapis.com';
const DEFAULT_OLLAMA_BASE_URL = 'http://127.0.0.1:11434/v1';
const DEFAULT_GROQ_BASE_URL = 'https://api.groq.com/openai/v1';
const DEFAULT_XAI_BASE_URL = 'https://api.x.ai/v1';
const DEFAULT_OPENROUTER_BASE_URL = 'https://openrouter.ai/api/v1';
const DEFAULT_MISTRAL_BASE_URL = 'https://api.mistral.ai/v1';
const DEFAULT_TOGETHER_BASE_URL = 'https://api.together.xyz/v1';
const DEFAULT_DEEPSEEK_BASE_URL = 'https://api.deepseek.com/v1';
const DEFAULT_FIREWORKS_BASE_URL = 'https://api.fireworks.ai/inference/v1';
const DEFAULT_PERPLEXITY_BASE_URL = 'https://api.perplexity.ai';
const DEFAULT_MOONSHOT_BASE_URL = 'https://api.moonshot.ai/v1';

const PROVIDER_ENV: Record<string, ProviderEnvConfig> = {
  openai: {
    keyEnv: ['OPENAI_API_KEY', 'OPENAI_KEY', 'OPENAI_TOKEN'],
    urlEnv: ['OPENAI_BASE_URL'],
    defaultBaseUrl: DEFAULT_OPENAI_BASE_URL,
  },
  anthropic: {
    keyEnv: [
      'ANTHROPIC_API_KEY',
      'ANTHROPIC_KEY',
      'ANTHROPIC_TOKEN',
      'CLAUDE_API_KEY',
    ],
    urlEnv: ['ANTHROPIC_BASE_URL'],
    defaultBaseUrl: DEFAULT_ANTHROPIC_BASE_URL,
  },
  google: {
    keyEnv: [
      'GOOGLE_API_KEY',
      'GOOGLE_KEY',
      'GOOGLE_AI_KEY',
      'GEMINI_API_KEY',
      'GOOGLE_GENERATIVE_AI_API_KEY',
    ],
    urlEnv: ['GOOGLE_BASE_URL'],
    defaultBaseUrl: DEFAULT_GOOGLE_BASE_URL,
  },
  gemini: {
    keyEnv: [
      'GOOGLE_API_KEY',
      'GOOGLE_KEY',
      'GOOGLE_AI_KEY',
      'GEMINI_API_KEY',
      'GOOGLE_GENERATIVE_AI_API_KEY',
    ],
    urlEnv: ['GOOGLE_BASE_URL'],
    defaultBaseUrl: DEFAULT_GOOGLE_BASE_URL,
  },
  ollama: {
    keyEnv: ['OLLAMA_API_KEY'],
    urlEnv: ['OLLAMA_BASE_URL'],
    defaultBaseUrl: DEFAULT_OLLAMA_BASE_URL,
  },
  groq: {
    keyEnv: ['GROQ_API_KEY', 'GROQ_KEY', 'GROQ_TOKEN'],
    urlEnv: ['GROQ_BASE_URL'],
    defaultBaseUrl: DEFAULT_GROQ_BASE_URL,
  },
  xai: {
    keyEnv: ['XAI_API_KEY', 'XAI_KEY', 'GROK_API_KEY'],
    urlEnv: ['XAI_BASE_URL'],
    defaultBaseUrl: DEFAULT_XAI_BASE_URL,
  },
  grok: {
    keyEnv: ['XAI_API_KEY', 'XAI_KEY', 'GROK_API_KEY'],
    urlEnv: ['XAI_BASE_URL'],
    defaultBaseUrl: DEFAULT_XAI_BASE_URL,
  },
  openrouter: {
    keyEnv: ['OPENROUTER_API_KEY', 'OPENROUTER_KEY', 'OPENROUTER_TOKEN'],
    urlEnv: ['OPENROUTER_BASE_URL'],
    defaultBaseUrl: DEFAULT_OPENROUTER_BASE_URL,
  },
  mistral: {
    keyEnv: ['MISTRAL_API_KEY', 'MISTRAL_KEY', 'MISTRAL_TOKEN'],
    urlEnv: ['MISTRAL_BASE_URL'],
    defaultBaseUrl: DEFAULT_MISTRAL_BASE_URL,
  },
  together: {
    keyEnv: ['TOGETHER_API_KEY', 'TOGETHER_KEY', 'TOGETHER_TOKEN'],
    urlEnv: ['TOGETHER_BASE_URL'],
    defaultBaseUrl: DEFAULT_TOGETHER_BASE_URL,
  },
  deepseek: {
    keyEnv: ['DEEPSEEK_API_KEY'],
    urlEnv: ['DEEPSEEK_BASE_URL'],
    defaultBaseUrl: DEFAULT_DEEPSEEK_BASE_URL,
  },
  fireworks: {
    keyEnv: ['FIREWORKS_API_KEY'],
    urlEnv: ['FIREWORKS_BASE_URL'],
    defaultBaseUrl: DEFAULT_FIREWORKS_BASE_URL,
  },
  perplexity: {
    keyEnv: ['PERPLEXITY_API_KEY'],
    urlEnv: ['PERPLEXITY_BASE_URL'],
    defaultBaseUrl: DEFAULT_PERPLEXITY_BASE_URL,
  },
  moonshot: {
    keyEnv: ['MOONSHOT_API_KEY', 'MOONSHOT_KEY', 'MOONSHOT_TOKEN', 'KIMI_API_KEY'],
    urlEnv: ['MOONSHOT_BASE_URL', 'KIMI_BASE_URL'],
    defaultBaseUrl: DEFAULT_MOONSHOT_BASE_URL,
  },
  kimi: {
    keyEnv: ['KIMI_API_KEY', 'MOONSHOT_API_KEY', 'MOONSHOT_KEY', 'MOONSHOT_TOKEN'],
    urlEnv: ['KIMI_BASE_URL', 'MOONSHOT_BASE_URL'],
    defaultBaseUrl: DEFAULT_MOONSHOT_BASE_URL,
  },
};

export const PROVIDER_HELP_TEXT =
  'Provider: openai, anthropic, google/gemini, ollama, groq, xai/grok, openrouter, mistral, together, deepseek, fireworks, perplexity, moonshot/kimi';

export function normalizeProvider(
  provider: string | undefined
): string | undefined {
  const normalized = provider?.trim().toLowerCase();
  return normalized ? normalized : undefined;
}

function firstDefinedEnv(names: readonly string[]): string | undefined {
  for (const name of names) {
    const value = process.env[name];
    if (typeof value === 'string' && value.trim() !== '') {
      return value;
    }
  }

  return undefined;
}

export function resolveProviderConfig(
  provider: string | undefined,
  apiKey?: string
): ResolvedProviderConfig | undefined {
  const normalizedProvider = normalizeProvider(provider);
  if (!normalizedProvider) {
    return undefined;
  }

  const env = PROVIDER_ENV[normalizedProvider];
  if (!env) {
    return undefined;
  }

  return {
    provider: normalizedProvider,
    apiKey: apiKey ?? firstDefinedEnv(env.keyEnv),
    baseUrl: firstDefinedEnv(env.urlEnv),
    defaultBaseUrl: env.defaultBaseUrl,
  };
}
