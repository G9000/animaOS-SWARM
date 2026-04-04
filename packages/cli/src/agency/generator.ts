import {
  OpenAIAdapter,
  AnthropicAdapter,
  OllamaAdapter,
  type IModelAdapter,
  type ModelConfig,
  type GenerateOptions,
  type GenerateResult,
  type ModelProvider,
  type UUID,
} from '@animaOS-SWARM/core';
import type { AgentDefinition } from './types.js';
import { resolveProviderConfig } from '../provider-config.js';

class DelegatingModelAdapter implements IModelAdapter {
  readonly provider: ModelProvider;

  constructor(
    provider: ModelProvider,
    private readonly delegate: IModelAdapter,
    private readonly defaults: Pick<ModelConfig, 'apiKey' | 'baseUrl'> = {}
  ) {
    this.provider = provider;
  }

  generate(
    config: ModelConfig,
    options: GenerateOptions
  ): Promise<GenerateResult> {
    return this.delegate.generate(this.resolveConfig(config), options);
  }

  async *generateStream(
    config: ModelConfig,
    options: GenerateOptions
  ): AsyncGenerator<
    Awaited<
      ReturnType<
        NonNullable<IModelAdapter['generateStream']>
      > extends AsyncGenerator<infer T>
        ? T
        : never
    >
  > {
    if (!this.delegate.generateStream) {
      throw new Error(`Streaming is not supported by ${this.provider}`);
    }

    for await (const chunk of this.delegate.generateStream(
      this.resolveConfig(config),
      options
    )) {
      yield chunk;
    }
  }

  private resolveConfig(config: ModelConfig): ModelConfig {
    return {
      ...config,
      provider: this.provider,
      apiKey: config.apiKey ?? this.defaults.apiKey,
      baseUrl: config.baseUrl ?? this.defaults.baseUrl,
    };
  }
}

class GoogleGenerativeAIAdapter implements IModelAdapter {
  readonly provider: ModelProvider;

  constructor(provider: ModelProvider) {
    this.provider = provider;
  }

  async generate(
    config: ModelConfig,
    options: GenerateOptions
  ): Promise<GenerateResult> {
    const apiKey = config.apiKey;
    if (!apiKey) {
      throw new Error(
        `API key is not configured for provider: ${this.provider}`
      );
    }

    const baseUrl = (
      config.baseUrl ?? 'https://generativelanguage.googleapis.com'
    ).replace(/\/+$/, '');
    const response = await fetch(
      `${baseUrl}/v1beta/models/${encodeURIComponent(
        config.model
      )}:generateContent?key=${encodeURIComponent(apiKey)}`,
      {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
        },
        body: JSON.stringify({
          system_instruction: {
            parts: [{ text: options.system }],
          },
          contents: options.messages
            .filter(
              (message) => message.role !== 'tool' && message.role !== 'system'
            )
            .map((message) => ({
              role: message.role === 'assistant' ? 'model' : 'user',
              parts: [{ text: message.content.text }],
            })),
          generationConfig: {
            ...(options.temperature ?? config.temperature
              ? { temperature: options.temperature ?? config.temperature }
              : {}),
            ...(options.maxTokens ?? config.maxTokens
              ? { maxOutputTokens: options.maxTokens ?? config.maxTokens }
              : {}),
          },
        }),
      }
    );

    if (!response.ok) {
      throw new Error(
        `Google Generative AI request failed: ${await response.text()}`
      );
    }

    const payload = (await response.json()) as {
      candidates?: Array<{
        content?: { parts?: Array<{ text?: string }> };
        finishReason?: string;
      }>;
      usageMetadata?: {
        promptTokenCount?: number;
        candidatesTokenCount?: number;
        totalTokenCount?: number;
      };
    };
    const candidate = payload.candidates?.[0];
    const text =
      candidate?.content?.parts?.map((part) => part.text ?? '').join('') ?? '';

    return {
      content: { text },
      usage: {
        promptTokens: payload.usageMetadata?.promptTokenCount ?? 0,
        completionTokens: payload.usageMetadata?.candidatesTokenCount ?? 0,
        totalTokens: payload.usageMetadata?.totalTokenCount ?? 0,
      },
      stopReason:
        candidate?.finishReason === 'MAX_TOKENS' ? 'max_tokens' : 'end',
    };
  }
}

/**
 * Create a model adapter for the given provider.
 */
export function createAdapter(
  provider: string,
  apiKey?: string
): IModelAdapter {
  const resolved = resolveProviderConfig(provider, apiKey);
  const normalizedProvider =
    resolved?.provider ?? provider.trim().toLowerCase();
  const baseUrl = resolved?.baseUrl ?? resolved?.defaultBaseUrl;

  switch (normalizedProvider) {
    case 'openai':
      return new DelegatingModelAdapter(
        normalizedProvider,
        new OpenAIAdapter(resolved?.apiKey, baseUrl)
      );
    case 'anthropic':
      return new DelegatingModelAdapter(
        normalizedProvider,
        new AnthropicAdapter(resolved?.apiKey, baseUrl)
      );
    case 'google':
    case 'gemini':
      return new DelegatingModelAdapter(
        normalizedProvider,
        new GoogleGenerativeAIAdapter(normalizedProvider),
        {
          apiKey: resolved?.apiKey,
          baseUrl,
        }
      );
    case 'ollama':
      return new DelegatingModelAdapter(
        normalizedProvider,
        new OllamaAdapter(baseUrl),
        {
          apiKey: resolved?.apiKey,
          baseUrl,
        }
      );
    case 'groq':
    case 'xai':
    case 'grok':
    case 'openrouter':
    case 'mistral':
    case 'together':
    case 'deepseek':
    case 'fireworks':
    case 'perplexity':
      return new DelegatingModelAdapter(
        normalizedProvider,
        new OpenAIAdapter(resolved?.apiKey, baseUrl)
      );
    default:
      throw new Error(`Unsupported provider: ${provider}`);
  }
}

export interface GenerateAgentTeamOptions {
  adapter: IModelAdapter;
  model: string;
  agencyName: string;
  agencyDescription: string;
}

/**
 * Use an LLM to generate 2-4 worker agent suggestions for an agency
 * based on its name, description, and orchestrator bio.
 */
export async function generateAgentTeam(
  opts: GenerateAgentTeamOptions
): Promise<AgentDefinition[]> {
  const dummyId = '00000000-0000-0000-0000-000000000000' as UUID;

  const config: ModelConfig = {
    provider: opts.adapter.provider,
    model: opts.model,
  };

  const prompt = [
    `You are designing a team of AI agents for an agency called "${opts.agencyName}".`,
    `Agency purpose: ${opts.agencyDescription}`,
    '',
    'Suggest 3-5 agents (including an orchestrator) that would form this agency.',
    'The first agent should be the orchestrator — the one who coordinates the team.',
    'The rest are workers with distinct roles.',
    '',
    'Respond with ONLY valid JSON — an array of agent objects. No markdown, no explanation.',
    'Each object must have these fields:',
    '  - "name": a short snake_case identifier',
    '  - "role": either "orchestrator" or "worker"',
    '  - "bio": 1-2 sentences describing who this agent is — personality and expertise',
    '  - "lore": 1-2 sentences of backstory — what shaped them, their origin',
    '  - "adjectives": array of 3-5 personality trait words (e.g. ["analytical", "thorough", "methodical"])',
    '  - "topics": array of 3-6 short expertise tags (e.g. ["web research", "data analysis", "fact checking"])',
    '  - "knowledge": array of 2-4 specific things this agent knows deeply',
    '  - "style": 1-2 sentences describing how this agent communicates',
    '  - "system": core instruction — what they do and how (2-3 sentences)',
  ].join('\n');

  const result = await opts.adapter.generate(config, {
    system: 'You are a helpful assistant that outputs only valid JSON.',
    messages: [
      {
        id: dummyId,
        agentId: dummyId,
        roomId: dummyId,
        content: { text: prompt },
        role: 'user',
        createdAt: Date.now(),
      },
    ],
  });

  const text = result.content.text.trim();

  // Strip markdown code fences if the LLM wrapped the response
  const cleaned = text
    .replace(/^```(?:json)?\s*\n?/i, '')
    .replace(/\n?```\s*$/i, '')
    .trim();

  let parsed: unknown;
  try {
    parsed = JSON.parse(cleaned);
  } catch {
    throw new Error(
      `Failed to parse LLM response as JSON. Raw response:\n${text}`
    );
  }

  if (!Array.isArray(parsed)) {
    throw new Error(`Expected JSON array from LLM, got ${typeof parsed}`);
  }

  return parsed.map((item: Record<string, unknown>) => ({
    name: (item.name as string) ?? 'unnamed',
    role: ((item.role as string) ?? 'worker') as 'orchestrator' | 'worker',
    bio: (item.bio as string) ?? '',
    lore: item.lore as string | undefined,
    adjectives: Array.isArray(item.adjectives)
      ? (item.adjectives as string[])
      : undefined,
    topics: Array.isArray(item.topics) ? (item.topics as string[]) : undefined,
    knowledge: Array.isArray(item.knowledge)
      ? (item.knowledge as string[])
      : undefined,
    style: item.style as string | undefined,
    system: (item.system as string) ?? '',
  }));
}
