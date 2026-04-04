import { afterEach, describe, expect, it } from 'vitest';
import { resolveDaemonModelSettings } from './utils.js';

const ENV_KEYS = [
  'GEMINI_API_KEY',
  'GOOGLE_BASE_URL',
  'GROK_API_KEY',
  'XAI_BASE_URL',
  'OPENROUTER_API_KEY',
] as const;

afterEach(() => {
  for (const key of ENV_KEYS) {
    delete process.env[key];
  }
});

describe('resolveDaemonModelSettings', () => {
  it('reads ElizaOS-style provider aliases for google and xai-family providers', () => {
    process.env.GEMINI_API_KEY = 'gemini-key';
    process.env.GOOGLE_BASE_URL = 'https://google.example';
    process.env.GROK_API_KEY = 'grok-key';
    process.env.XAI_BASE_URL = 'https://xai.example';

    expect(resolveDaemonModelSettings('google')).toEqual({
      apiKey: 'gemini-key',
      baseUrl: 'https://google.example',
    });
    expect(resolveDaemonModelSettings('grok')).toEqual({
      apiKey: 'grok-key',
      baseUrl: 'https://xai.example',
    });
  });

  it('prefers the explicit api key override over environment variables', () => {
    process.env.OPENROUTER_API_KEY = 'env-key';

    expect(resolveDaemonModelSettings('openrouter', 'flag-key')).toEqual({
      apiKey: 'flag-key',
    });
  });
});
