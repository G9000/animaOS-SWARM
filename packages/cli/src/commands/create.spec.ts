import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { REMOTE_PROVIDER_IDS, providerKeyEnvNames } from '../provider-config.js';
import {
  buildSingleAgentConfig,
  getCreateGenerationCredentialIssue,
  getCreateProviderIssue,
  resolveCreateProvider,
  resolveCreateTarget,
} from './create.js';

describe('create command flow helpers', () => {
  const envSnapshot = new Map<string, string | undefined>();

  beforeEach(() => {
    envSnapshot.clear();
    for (const provider of REMOTE_PROVIDER_IDS) {
      for (const envName of providerKeyEnvNames(provider)) {
        if (!envSnapshot.has(envName)) {
          envSnapshot.set(envName, process.env[envName]);
        }
        delete process.env[envName];
      }
    }
  });

  afterEach(() => {
    for (const [envName, value] of envSnapshot) {
      if (value === undefined) {
        delete process.env[envName];
      } else {
        process.env[envName] = value;
      }
    }
  });

  it('keeps the legacy shorthand as agency creation', () => {
    expect(resolveCreateTarget('content-team', undefined, undefined)).toEqual({
      kind: 'agency',
      nameArg: 'content-team',
      needsKindPrompt: false,
    });
  });

  it('routes explicit agent and agency subflows', () => {
    expect(resolveCreateTarget('agent', 'helper', undefined)).toEqual({
      kind: 'agent',
      nameArg: 'helper',
      needsKindPrompt: false,
    });
    expect(resolveCreateTarget('agency', 'lab', undefined)).toEqual({
      kind: 'agency',
      nameArg: 'lab',
      needsKindPrompt: false,
    });
  });

  it('falls back to deterministic scaffolding when no provider is configured', () => {
    expect(resolveCreateProvider(undefined)).toBe('deterministic');
  });

  it('auto-selects configured remote providers before deterministic scaffolding', () => {
    process.env.OPENAI_API_KEY = 'test-key';

    expect(resolveCreateProvider(undefined)).toBe('openai');
  });

  it('reports missing credentials before remote agency generation', () => {
    expect(getCreateGenerationCredentialIssue('openai')).toContain('OPENAI_API_KEY');
    expect(getCreateGenerationCredentialIssue('deterministic')).toBeUndefined();
  });

  it('reports explicit unknown providers before falling back to auto mode', () => {
    expect(getCreateProviderIssue('not-a-provider')).toContain('Unknown provider');
    expect(getCreateProviderIssue(undefined)).toBeUndefined();
    expect(getCreateProviderIssue('deterministic')).toBeUndefined();
  });

  it('builds single-agent workspaces as a launchable anima.yaml shape', () => {
    const config = buildSingleAgentConfig({
      name: 'medicine-helper',
      description: 'testing a medicine',
      provider: 'deterministic',
      model: 'local-model',
    });

    expect(config).toMatchObject({
      name: 'medicine-helper',
      provider: 'deterministic',
      model: 'local-model',
      strategy: 'supervisor',
      agents: [],
      orchestrator: {
        name: 'medicine-helper',
        role: 'orchestrator',
        model: 'local-model',
      },
    });
    expect(config.orchestrator.system).toContain('testing a medicine');
  });
});