import { afterEach, describe, expect, it, vi } from 'vitest';
import { createAdapter } from './generator.js';

const SUPPORTED_CREATE_PROVIDERS = [
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
    vi.unstubAllGlobals();
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
    vi.stubGlobal('fetch', fetchMock);

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
