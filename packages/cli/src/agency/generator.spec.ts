import { afterEach, describe, expect, it, vi } from 'vitest';
import { createAdapter, generateAgentSeeds, generateAgentTeam } from './generator.js';

const originalFetch = globalThis.fetch;

const SUPPORTED_CREATE_PROVIDERS = [
  'deterministic',
  'openai',
  'anthropic',
  'google',
  'gemini',
  'ollama',
  'groq',
  'xai',
  'grok',
  'openrouter',
  'mistral',
  'together',
  'deepseek',
  'fireworks',
  'perplexity',
  'moonshot',
  'kimi',
] as const;

describe('createAdapter', () => {
  afterEach(() => {
    vi.restoreAllMocks();
    if (originalFetch) {
      globalThis.fetch = originalFetch;
    } else {
      Reflect.deleteProperty(globalThis, 'fetch');
    }
    delete process.env.GEMINI_API_KEY;
    delete process.env.GOOGLE_BASE_URL;
  });

  it.each(SUPPORTED_CREATE_PROVIDERS)('supports %s', (provider) => {
    expect(createAdapter(provider, 'test-key').provider).toBe(provider);
  });

  it('throws for unsupported providers', () => {
    expect(() => createAdapter('unsupported-provider')).toThrow(
      'Unsupported provider: unsupported-provider'
    );
  });

  it('generates a deterministic agency without provider credentials', async () => {
    const adapter = createAdapter('deterministic');

    const generated = await generateAgentTeam({
      adapter,
      model: 'local-model',
      agencyName: 'Medicine Lab',
      agencyDescription: 'testing a medicine',
      teamSize: 3,
    });

    expect(generated.agents).toHaveLength(3);
    expect(generated.agents[0]).toMatchObject({
      role: 'orchestrator',
      model: 'local-model',
    });
    expect(generated.agents[1]?.system).toContain('Challenge assumptions');
    expect(generated.mission).toContain('Medicine Lab');
  });

  it('generates deterministic seed memories without provider credentials', async () => {
    const adapter = createAdapter('deterministic');
    const seeds = await generateAgentSeeds({
      adapter,
      model: 'local-model',
      agencyName: 'Medicine Lab',
      agencyDescription: 'testing a medicine',
      agents: [
        {
          name: 'Avery',
          bio: 'Lead',
          system: 'Coordinate work',
        },
      ],
    });

    expect(seeds).toEqual([
      expect.objectContaining({
        agentName: 'Avery',
        entries: expect.arrayContaining([
          expect.objectContaining({ type: 'fact' }),
          expect.objectContaining({ type: 'observation' }),
          expect.objectContaining({ type: 'reflection' }),
        ]),
      }),
    ]);
  });

  it('uses gemini aliases and google generateContent for create flow', async () => {
    process.env.GEMINI_API_KEY = 'gemini-key';
    process.env.GOOGLE_BASE_URL = 'https://google.example';

    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          candidates: [
            {
              content: {
                parts: [
                  {
                    text: '[{"name":"planner","role":"orchestrator","bio":"bio","system":"system"}]',
                  },
                ],
              },
              finishReason: 'STOP',
            },
          ],
          usageMetadata: {
            promptTokenCount: 10,
            candidatesTokenCount: 12,
            totalTokenCount: 22,
          },
        }),
        { status: 200, headers: { 'Content-Type': 'application/json' } }
      )
    );
    Object.defineProperty(globalThis, 'fetch', {
      value: fetchMock,
      configurable: true,
      writable: true,
    });

    const adapter = createAdapter('gemini');
    const result = await adapter.generate(
      { provider: 'gemini', model: 'gemini-2.0-flash' },
      {
        system: 'Return JSON only.',
        messages: [
          {
            id: '00000000-0000-0000-0000-000000000000',
            agentId: '00000000-0000-0000-0000-000000000000',
            roomId: '00000000-0000-0000-0000-000000000000',
            role: 'user',
            content: { text: 'make a team' },
            createdAt: Date.now(),
          },
        ],
      }
    );

    expect(fetchMock).toHaveBeenCalledWith(
      'https://google.example/v1beta/models/gemini-2.0-flash:generateContent?key=gemini-key',
      expect.objectContaining({ method: 'POST' })
    );
    expect(result.content.text).toContain('planner');
    expect(result.usage.totalTokens).toBe(22);
  });
});
