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
import type { AgentSeedMemories, SeedMemoryEntry, SeedMemoryType } from './seed-memory.js';
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
    case 'moonshot':
    case 'kimi':
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
  /** Total team size including the orchestrator. Defaults to 4. Clamped to [2, 10]. */
  teamSize?: number;
  /**
   * Optional pool of model identifiers to distribute across agents.
   * When provided, the LLM assigns each agent a model from this list.
   * Heterogeneous models significantly reduce consensus collapse (see arXiv:2604.18005).
   */
  modelPool?: string[];
}

export interface GeneratedAgency {
  mission?: string;
  values?: string[];
  agents: AgentDefinition[];
}

/**
 * Use an LLM to generate a full agency: mission, values, and the agent roster.
 */
export async function generateAgentTeam(
  opts: GenerateAgentTeamOptions
): Promise<GeneratedAgency> {
  const dummyId = '00000000-0000-0000-0000-000000000000' as UUID;

  const config: ModelConfig = {
    provider: opts.adapter.provider,
    model: opts.model,
  };

  const requestedSize = Math.max(2, Math.min(10, opts.teamSize ?? 4));
  const workerCount = requestedSize - 1;
  const needsSkeptic = workerCount >= 3;
  const modelPool = opts.modelPool && opts.modelPool.length > 0 ? opts.modelPool : null;

  const modelInstruction = modelPool
    ? [
        '',
        `MODEL POOL — assign each agent one model from this list: [${modelPool.map((m) => `"${m}"`).join(', ')}].`,
        'Distribute them so no two adjacent collaborators share the same model.',
        'Each AgentObject must include a "model" field set to one of the listed values.',
      ]
    : [];

  const skepticInstruction = needsSkeptic
    ? [
        '',
        'SKEPTIC RULE: one worker must be a dedicated contrarian — their explicit job is to challenge',
        'assumptions, poke holes in plans, and surface risks others miss. Their "system" must include',
        'an instruction to actively disagree when they see flaws, not to reach consensus.',
        'Give this agent adjectives like "skeptical", "rigorous", "contrarian".',
      ]
    : [];

  const prompt = [
    `You are designing a team of AI agents for an agency called "${opts.agencyName}".`,
    `Agency purpose: ${opts.agencyDescription}`,
    '',
    `Generate EXACTLY ${requestedSize} agents in total: 1 orchestrator + ${workerCount} workers.`,
    'The first agent must be the orchestrator — the one who coordinates the team.',
    'Workers should have focused, distinct mandates.',
    '',
    'OVERLAP RULE: when cross-validation, multiple perspectives, or parallel exploration adds clear value,',
    'you may include 2-3 agents in similar roles but with DIFFERENT angles or methodologies',
    '(e.g. researcher_quantitative + researcher_qualitative, or writer_long_form + writer_punchy).',
    'Never duplicate an agent verbatim — each must contribute something distinct.',
    ...skepticInstruction,
    '',
    'ANTI-SYCOPHANCY: every worker\'s "system" field MUST include a sentence instructing them to',
    'challenge assumptions and disagree with the orchestrator whenever they have a different view.',
    'Workers should surface dissent, not defer.',
    ...modelInstruction,
    '',
    'Respond with ONLY valid JSON — a single object. No markdown, no explanation.',
    'The object MUST have this shape:',
    '{',
    '  "mission": string  — one-sentence north star the whole team shares',
    '  "values": string[] — 3-5 cultural principles the team operates under (short phrases)',
    '  "agents": AgentObject[]  — exactly the requested size',
    '}',
    '',
    'Each AgentObject must have:',
    '  - "name": a real human name. First name only when distinct (e.g. "Sarah", "Marcus", "Aiko"),',
    '             OR full first + last name when richer characterization fits (e.g. "Sarah Chen",',
    '             "Marcus Rivera", "Aiko Tanaka"). Pick culturally diverse names that fit each',
    '             personality. NEVER use single-letter suffixes or initials like "Sarah_C" — if two',
    '             agents would share a first name, give them different first names entirely or use',
    '             full last names. Treat each agent as a real teammate, not a serial number.',
    '  - "position": real-world job title (e.g. "Head of Growth", "Chief Brand Officer")',
    '  - "role": either "orchestrator" or "worker"',
    '  - "bio": 1-2 sentences — personality and expertise',
    '  - "lore": 1-2 sentences of backstory',
    '  - "adjectives": array of 3-5 personality trait words',
    '  - "topics": array of 3-6 short expertise tags',
    '  - "knowledge": array of 2-4 specific things this agent knows deeply',
    '  - "style": 1-2 sentences describing how this agent communicates',
    '  - "system": core instruction — what they do, decide, and own (2-3 sentences). Must include',
    '              a line instructing the agent to voice disagreement when they see a better path.',
    '  - "tools": array of 2-5 skill slugs in snake_case (e.g. ["web_search", "trend_forecast"])',
    ...(modelPool ? ['  - "model": one model from the provided pool'] : []),
    '  - "collaborates_with": array of agent names (snake_case) this agent frequently pairs with.',
    '             Use this to express working relationships — which workers naturally hand off to,',
    '             review, or build on each other. Reference names that exist in this same array.',
    '             The orchestrator may leave this empty (it implicitly delegates to all workers).',
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

  // Tolerate both shapes: a bare array (legacy) or a structured object (new).
  let mission: string | undefined;
  let values: string[] | undefined;
  let rawAgents: unknown[];

  if (Array.isArray(parsed)) {
    rawAgents = parsed;
  } else if (parsed && typeof parsed === 'object') {
    const obj = parsed as Record<string, unknown>;
    mission = typeof obj.mission === 'string' ? obj.mission : undefined;
    values = Array.isArray(obj.values) ? (obj.values as string[]) : undefined;
    if (!Array.isArray(obj.agents)) {
      throw new Error(
        `Expected "agents" array in LLM response, got ${typeof obj.agents}`
      );
    }
    rawAgents = obj.agents;
  } else {
    throw new Error(`Expected JSON object or array from LLM, got ${typeof parsed}`);
  }

  const agents = rawAgents.map((raw): AgentDefinition => {
    const item = raw as Record<string, unknown>;
    return {
      name: (item.name as string) ?? 'unnamed',
      position: item.position as string | undefined,
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
      model: typeof item.model === 'string' ? item.model : undefined,
      tools: Array.isArray(item.tools) ? (item.tools as string[]) : undefined,
      collaboratesWith: Array.isArray(item.collaborates_with)
        ? (item.collaborates_with as string[])
        : Array.isArray(item.collaboratesWith)
        ? (item.collaboratesWith as string[])
        : undefined,
    };
  });

  return { mission, values, agents };
}

export interface GenerateSeedMemoriesOptions {
  adapter: IModelAdapter;
  model: string;
  agencyName: string;
  agencyDescription: string;
  mission?: string;
  agents: AgentDefinition[];
}

const VALID_SEED_TYPES = new Set<SeedMemoryType>([
  'fact',
  'observation',
  'task_result',
  'reflection',
]);

/**
 * Use an LLM to generate realistic seed memories for every agent in the
 * agency. Returns one AgentSeedMemories per agent (3-5 entries each).
 */
export async function generateAgentSeeds(
  opts: GenerateSeedMemoriesOptions
): Promise<AgentSeedMemories[]> {
  const dummyId = '00000000-0000-0000-0000-000000000000' as UUID;

  const config: ModelConfig = {
    provider: opts.adapter.provider,
    model: opts.model,
  };

  const agentSummaries = opts.agents.map((a) => {
    const lines = [`Name: ${a.name}`, `Position: ${a.position ?? 'unspecified'}`];
    if (a.bio) lines.push(`Bio: ${a.bio}`);
    if (a.topics?.length) lines.push(`Expertise: ${a.topics.join(', ')}`);
    if (a.knowledge?.length) lines.push(`Knows: ${a.knowledge.join('; ')}`);
    return lines.join('\n');
  });

  const prompt = [
    `Agency: "${opts.agencyName}"`,
    `Purpose: ${opts.agencyDescription}`,
    ...(opts.mission ? [`Mission: ${opts.mission}`] : []),
    '',
    'For each agent below, generate 3-5 seed memories — concrete facts, observations,',
    'or prior knowledge this person would realistically hold given their role and the agency context.',
    'Make them specific and useful, not generic platitudes.',
    '',
    'Agents:',
    ...agentSummaries.map((s, i) => `\n[${i + 1}]\n${s}`),
    '',
    'Respond with ONLY valid JSON — a single object, no markdown.',
    '{',
    '  "seeds": [',
    '    {',
    '      "agentName": string  — exactly as given above',
    '      "memories": [',
    '        {',
    '          "type": "fact" | "observation" | "task_result" | "reflection"',
    '          "content": string  — the memory (1-2 sentences)',
    '          "importance": number  — 0.0 to 1.0, how relevant this is to day-to-day work',
    '          "tags": string[]  — 1-3 short labels (optional)',
    '        }',
    '      ]',
    '    }',
    '  ]',
    '}',
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
  const cleaned = text
    .replace(/^```(?:json)?\s*\n?/i, '')
    .replace(/\n?```\s*$/i, '')
    .trim();

  let parsed: unknown;
  try {
    parsed = JSON.parse(cleaned);
  } catch {
    throw new Error(`Failed to parse seed LLM response as JSON. Raw:\n${text}`);
  }

  if (!parsed || typeof parsed !== 'object' || !Array.isArray((parsed as Record<string, unknown>).seeds)) {
    throw new Error('Expected { seeds: [...] } from seed generator');
  }

  const rawSeeds = (parsed as Record<string, unknown>).seeds as unknown[];

  return rawSeeds.map((raw): AgentSeedMemories => {
    const item = raw as Record<string, unknown>;
    const agentName = typeof item.agentName === 'string' ? item.agentName : 'unknown';
    const rawMemories = Array.isArray(item.memories) ? item.memories : [];

    const entries = rawMemories
      .map((m): SeedMemoryEntry | null => {
        const mem = m as Record<string, unknown>;
        const type = mem.type as string;
        const content = typeof mem.content === 'string' ? mem.content.trim() : '';
        if (!content || !VALID_SEED_TYPES.has(type as SeedMemoryType)) return null;

        const importance =
          typeof mem.importance === 'number' &&
          mem.importance >= 0 &&
          mem.importance <= 1
            ? mem.importance
            : 0.5;

        const tags = Array.isArray(mem.tags)
          ? (mem.tags as unknown[]).filter((t): t is string => typeof t === 'string')
          : undefined;

        return {
          type: type as SeedMemoryType,
          content,
          importance,
          ...(tags && tags.length > 0 ? { tags } : {}),
        };
      })
      .filter((e): e is SeedMemoryEntry => e !== null);

    return { agentName, entries };
  });
}
