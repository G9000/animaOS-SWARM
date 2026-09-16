/// <reference lib="dom" />
import { expect, test, type Route } from '@playwright/test';
import type { AgentTasks, AgentSchedule } from '@animaOS-SWARM/sdk';

interface Message {
  id: string;
  agentId: string;
  roomId: string;
  role: string;
  content: { text: string; metadata?: Record<string, unknown> };
  createdAtMs: number;
}

function agent(id: string, name: string, createdAtMs: number, manager = false) {
  return {
    state: {
      id,
      name,
      status: 'idle',
      createdAtMs,
      config: {
        name,
        provider: 'deterministic',
        model: manager ? 'manager-fixture' : 'research-fixture',
        system: manager
          ? 'Coordinate the workspace.'
          : 'Research independently.',
        tools: [],
        settings: { additional: manager ? { workspaceRole: 'lead' } : {} },
      },
      tokenUsage: { promptTokens: 0, completionTokens: 0, totalTokens: 0 },
    },
    messageCount: 0,
    eventCount: 0,
    messages: [] as Message[],
  };
}

async function json(route: Route, body: unknown, status = 200) {
  await route.fulfill({
    status,
    contentType: 'application/json',
    body: JSON.stringify(body),
  });
}

for (const viewport of [
  { width: 1280, height: 900 },
  { width: 390, height: 844 },
]) {
  test(`single companion preserves independent agents at ${viewport.width}px`, async ({
    page,
  }, testInfo) => {
    await page.setViewportSize(viewport);
    const manager = agent('manager', 'Anima', 1, true);
    const researcher = agent('researcher', 'Researcher', 0);
    researcher.messages.push({
      id: 'peer-1',
      agentId: 'researcher',
      roomId: 'peer:manager:researcher',
      role: 'user',
      content: {
        text: 'Compare the research sources for the team.',
        metadata: {
          communication: {
            kind: 'peer',
            fromAgentId: 'manager',
            toAgentId: 'researcher',
          },
        },
      },
      createdAtMs: 3,
    });
    researcher.messages.push({
      id: 'peer-reply',
      agentId: 'researcher',
      roomId: 'peer:manager:researcher',
      role: 'assistant',
      content: { text: 'I compared the sources for Anima.' },
      createdAtMs: 4,
    });
    researcher.messageCount = researcher.messages.length;
    const agents = [researcher, manager];
    const runs: { agentId: string; text: string; roomId: string }[] = [];
    const avatars = new Map<string, Buffer>();
    const taskLists = new Map<string, AgentTasks>();
    const schedules = new Map<string, AgentSchedule[]>();
    await page.route('**/api/**', async (route) => {
      const request = route.request();
      const path = new URL(request.url()).pathname.replace(/^\/api/, '');
      if (path === '/health') return json(route, { status: 'ok' });
      if (path === '/agents') return json(route, { agents });
      const avatarMatch = path.match(/^\/agents\/([^/]+)\/avatar$/);
      if (avatarMatch) {
        if (request.method() === 'PUT') {
          avatars.set(avatarMatch[1], request.postDataBuffer()!);
          return route.fulfill({ status: 204 });
        }
        if (request.method() === 'DELETE') {
          avatars.delete(avatarMatch[1]);
          return route.fulfill({ status: 204 });
        }
        const image = avatars.get(avatarMatch[1]);
        return image
          ? route.fulfill({
              status: 200,
              contentType: 'image/png',
              body: image,
            })
          : route.fulfill({ status: 404 });
      }
      if (path.endsWith('/memories/recent'))
        return json(route, {
          memories: [
            {
              id: 'memory-1',
              agentId: 'researcher',
              agentName: 'Researcher',
              type: 'fact',
              content: 'The owner prefers primary research sources.',
              importance: 0.8,
              createdAt: 10,
              scope: 'private',
              tags: ['research'],
            },
          ],
        });
      const agentMatch = path.match(/^\/agents\/([^/]+)$/);
      if (agentMatch && request.method() === 'PATCH') {
        const selected = agents.find(
          (item) => item.state.id === agentMatch[1],
        )!;
        const patch = request.postDataJSON();
        Object.assign(selected.state.config, patch);
        if (patch.name) selected.state.name = patch.name;
        return json(route, { agent: selected });
      }
      if (path === '/workspace')
        return json(route, {
          configured: false,
          workspace: null,
          defaultRoot: '/tmp/independent-agents',
        });
      if (path === '/providers')
        return json(route, {
          providers: [
            {
              id: 'deterministic',
              label: 'Deterministic',
              configured: true,
              requiresKey: false,
              apiKeyEnvs: [],
            },
          ],
        });
      if (/^\/agents\/[^/]+\/connectors$/.test(path))
        return json(route, { connectors: [] });
      const tasksMatch = path.match(/^\/agents\/([^/]+)\/tasks$/);
      if (tasksMatch) {
        const current = taskLists.get(tasksMatch[1]) ?? {
          tasks: [],
          revision: '0',
        };
        if (request.method() === 'PUT') {
          const input = request.postDataJSON() as AgentTasks;
          if (input.revision !== current.revision)
            return json(
              route,
              { error: 'Tasks changed. Refresh before saving again.' },
              409,
            );
          const saved = {
            tasks: input.tasks,
            revision: String(Number(current.revision) + 1),
          };
          taskLists.set(tasksMatch[1], saved);
          return json(route, saved);
        }
        return json(route, current);
      }
      const scheduleMatch = path.match(
        /^\/agents\/([^/]+)\/schedules(?:\/([^/]+))?$/,
      );
      if (scheduleMatch) {
        const current = schedules.get(scheduleMatch[1]) ?? [];
        if (request.method() === 'POST') {
          const schedule: AgentSchedule = {
            ...request.postDataJSON(),
            id: 'schedule-1',
            agentId: scheduleMatch[1],
            nextDueAtMs: Date.now() + 60000,
            lastFiredAtMs: null,
            lastOutcome: null,
            createdAtMs: Date.now(),
            updatedAtMs: Date.now(),
          };
          schedules.set(scheduleMatch[1], [...current, schedule]);
          return json(route, { schedule });
        }
        if (request.method() === 'PATCH') {
          const schedule = current.find(
            (item) => item.id === scheduleMatch[2],
          )!;
          Object.assign(schedule, request.postDataJSON());
          return json(route, { schedule });
        }
        return json(route, { schedules: current });
      }
      const match = path.match(/^\/agents\/([^/]+)\/run$/);
      if (match && request.method() === 'POST') {
        const selected = agents.find((item) => item.state.id === match[1]);
        if (!selected) return json(route, { error: 'unknown agent' }, 404);
        const input = request.postDataJSON() as {
          text: string;
          roomId: string;
        };
        runs.push({
          agentId: match[1],
          text: input.text,
          roomId: input.roomId,
        });
        const reply = `${selected.state.name} completed its own request.`;
        selected.messages.push(
          {
            id: `user-${runs.length}`,
            agentId: match[1],
            roomId: input.roomId,
            role: 'user',
            content: { text: input.text },
            createdAtMs: 10 + runs.length * 2,
          },
          {
            id: `reply-${runs.length}`,
            agentId: match[1],
            roomId: input.roomId,
            role: 'assistant',
            content: { text: reply },
            createdAtMs: 11 + runs.length * 2,
          },
        );
        selected.messageCount = selected.messages.length;
        return json(route, {
          agent: selected,
          result: { status: 'success', durationMs: 1, data: { text: reply } },
        });
      }
      return json(route, { error: `Unexpected fixture route ${path}` }, 404);
    });

    const researcherBefore = structuredClone(researcher);
    await page.goto('/');
    const composer = page.getByPlaceholder('Message Anima…');
    await expect(composer).toBeVisible();
    await expect(
      page.getByRole('combobox', { name: 'Chat with agent' }),
    ).toHaveCount(0);
    await expect(
      page.getByRole('navigation', { name: 'Direct messages' }),
    ).toHaveCount(0);
    await expect(
      page.getByText('I compared the sources for Anima.', { exact: true }),
    ).toHaveCount(0);
    await composer.fill('Companion draft stays here');
    await page.getByRole('button', { name: 'Activity', exact: true }).click();
    await page.getByRole('button', { name: 'Chat', exact: true }).click();
    await expect(composer).toHaveValue('Companion draft stays here');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await expect(
      page.getByText('Anima completed its own request.', { exact: true }),
    ).toBeVisible();
    expect(runs).toEqual([
      {
        agentId: 'manager',
        text: 'Companion draft stays here',
        roomId: 'direct:manager',
      },
    ]);

    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    const profile = page.getByRole('dialog', { name: 'Agent settings' });
    await profile.getByRole('button', { name: 'tasks', exact: true }).click();
    await profile
      .getByRole('textbox', { name: 'New agent task' })
      .fill('Find primary sources for the campaign');
    await profile
      .getByRole('button', { name: 'Add task', exact: true })
      .click();
    await profile
      .getByRole('button', { name: 'Save tasks', exact: true })
      .click();
    await expect(
      profile.getByRole('button', { name: 'Tasks saved', exact: true }),
    ).toBeVisible();
    expect(taskLists.get('manager')?.tasks).toHaveLength(1);
    expect(taskLists.has('researcher')).toBe(false);
    await profile
      .getByRole('button', { name: 'proactive', exact: true })
      .click();
    await profile.getByRole('button', { name: 'Use my task list' }).click();
    await profile.getByRole('button', { name: 'Enable schedule' }).click();
    await expect(
      profile.getByRole('button', { name: 'Pause', exact: true }),
    ).toBeVisible();
    expect(schedules.get('manager')?.[0].enabled).toBe(true);
    await profile.getByRole('button', { name: 'Pause', exact: true }).click();
    await expect(
      profile.getByRole('button', { name: 'Resume', exact: true }),
    ).toBeVisible();
    expect(schedules.get('manager')?.[0].enabled).toBe(false);
    expect(schedules.has('researcher')).toBe(false);
    await profile.getByRole('button', { name: 'Close settings' }).click();
    expect(researcher).toEqual(researcherBefore);
    expect(agents).toHaveLength(2);
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= window.innerWidth,
      ),
    ).toBe(true);
    await page.screenshot({
      path: testInfo.outputPath('companion-preserved-agents.png'),
      animations: 'disabled',
    });
  });
}
