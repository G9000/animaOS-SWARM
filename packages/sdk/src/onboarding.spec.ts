import { describe, expect, it } from 'vitest';
import { createDaemonClient, DaemonHttpError } from './index.js';

describe('onboarding HTTP contracts', () => {
  it('posts exact and automatic generation inputs without transforming nullable output', async () => {
    const calls: { url: string; init?: RequestInit }[] = [];
    const response = { name: 'Lab', mission: null, values: null, agents: [] };
    const client = createDaemonClient({
      baseUrl: '',
      fetch: async (url, init) => {
        calls.push({ url: String(url), init });
        return Response.json(response);
      },
    });
    const base = {
      name: 'Lab',
      description: 'Research',
      provider: 'openai',
      model: 'model',
      modelPool: ['model'],
    };
    expect(await client.agencies.generate({ ...base, teamSize: 3 })).toEqual(
      response,
    );
    await client.agencies.generate({ ...base, maxTeamSize: 5 });
    expect(
      calls.map(({ url, init }) => [
        url,
        init?.method,
        JSON.parse(String(init?.body)),
      ]),
    ).toEqual([
      ['/api/agencies/generate', 'POST', { ...base, teamSize: 3 }],
      ['/api/agencies/generate', 'POST', { ...base, maxTeamSize: 5 }],
    ]);
  });

  it('uses workspace endpoints, forces validation, encodes paths and preserves explicit tools and providers', async () => {
    const calls: { url: string; init?: RequestInit }[] = [];
    const client = createDaemonClient({
      baseUrl: '',
      fetch: async (url, init) => {
        calls.push({ url: String(url), init });
        return Response.json({
          configured: false,
          workspace: null,
          rootPath: null,
        });
      },
    });
    const workspace = {
      rootPath: 'C:\\Lab & #1',
      companyName: 'Lab',
      mission: 'Research',
      values: [],
    };
    const agent = {
      name: 'Manager',
      presetId: 'custom',
      system: 'Coordinate',
      provider: 'openai',
      model: 'model',
      tools: [],
    };
    const bootstrap = {
      workspace,
      agent,
      workers: [{ ...agent, name: 'Worker', tools: ['read_file'] }],
    };
    expect((await client.workspace.get()).workspace).toBeNull();
    await client.workspace.put(workspace);
    await client.workspace.validate({ ...workspace, validateOnly: false });
    await client.workspace.bootstrap(bootstrap);
    await client.workspace.inspect(workspace.rootPath);
    expect((await client.workspace.pickFolder()).rootPath).toBeNull();
    await client.workspace.resume(workspace.rootPath);
    expect(
      calls.map(({ url, init }) => [
        url,
        init?.method ?? 'GET',
        init?.body ? JSON.parse(String(init.body)) : undefined,
      ]),
    ).toEqual([
      ['/api/workspace', 'GET', undefined],
      ['/api/workspace', 'PUT', workspace],
      ['/api/workspace', 'PUT', { ...workspace, validateOnly: true }],
      ['/api/workspace/bootstrap', 'POST', bootstrap],
      [
        `/api/workspace/inspect?rootPath=${encodeURIComponent(workspace.rootPath)}`,
        'GET',
        undefined,
      ],
      ['/api/workspace/pick-folder', 'POST', undefined],
      ['/api/workspace/resume', 'POST', { rootPath: workspace.rootPath }],
    ]);
  });

  it('preserves daemon failures for caller recovery', async () => {
    const client = createDaemonClient({
      fetch: async () =>
        Response.json(
          { error: 'workspace already configured' },
          { status: 409 },
        ),
    });
    await expect(client.workspace.resume('C:\\Lab')).rejects.toBeInstanceOf(
      DaemonHttpError,
    );
    await expect(
      client.agencies.generate({
        name: 'Lab',
        description: 'Research',
        provider: 'openai',
        model: 'model',
        teamSize: 3,
      }),
    ).rejects.toMatchObject({ status: 409 });
  });
});
