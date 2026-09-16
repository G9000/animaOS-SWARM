import { expect, test, type Page } from '@playwright/test';

async function fixture(page: Page, empty = false) {
  const agent = {
    state: { id: 'companion', name: 'Anima', status: 'idle', createdAtMs: 1,
      config: { name: 'Anima', provider: 'openai', model: 'test-model', settings: { additional: { workspaceRole: 'lead' } }, tools: [] },
      tokenUsage: { promptTokens: 0, completionTokens: 0, totalTokens: 0 } },
    messages: [] as unknown[], messageCount: 0, eventCount: 0,
  };
  const helper = { ...agent, state: { ...agent.state, id: 'helper', name: 'Research helper', config: { ...agent.state.config, settings: { additional: { workspaceRole: 'helper' } } } } };
  let agents = empty ? [] : [agent, helper];
  let sent = 0;
  await page.route('**/api/**', async route => {
    const path = new URL(route.request().url()).pathname;
    let body: unknown = {};
    if (path === '/api/health') body = { status: 'ok' };
    else if (path === '/api/providers') body = { providers: [{ id: 'openai', label: 'OpenAI', configured: true, requiresKey: true, apiKeyEnvs: ['OPENAI_API_KEY'] }] };
    else if (path === '/api/workspace') body = { configured: !empty, workspace: empty ? null : { rootPath: '/workspace', companyName: 'Personal', mission: 'Everyday life', values: [], hasAvatar: false }, defaultRoot: '/workspace' };
    else if (path === '/api/workspace/bootstrap') {
      expect(route.request().postDataJSON()).not.toHaveProperty('workers');
      agents = [agent]; body = { agent };
    } else if (path === '/api/agents') body = { agents };
    else if (path.endsWith('/run')) {
      sent += 1;
      expect(path).toBe('/api/agents/companion/run');
      const text = route.request().postDataJSON().text;
      agent.messages = [
        { id: 'u1', role: 'user', agentId: 'companion', roomId: 'direct:companion', content: { text }, createdAtMs: 2 },
        { id: 'a1', role: 'assistant', agentId: 'companion', roomId: 'direct:companion', content: { text: 'Let’s make a little room in your day. What matters most today?' }, createdAtMs: 3 },
      ];
      agent.messageCount = 2;
      body = { agent, result: { status: 'success', durationMs: 1, data: {} } };
    } else if (path.includes('/connectors')) body = { connectors: [] };
    else if (path.includes('/schedules')) body = { schedules: [] };
    else if (path.includes('/tasks')) body = { tasks: [], revision: '1' };
    else if (path.includes('/memories')) body = { memories: [] };
    else if (path.includes('/jobs')) body = { jobs: [] };
    await route.fulfill({ contentType: 'application/json', body: JSON.stringify(body) });
  });
  return { sent: () => sent };
}

for (const viewport of [{ name: 'desktop', width: 1440, height: 960 }, { name: 'mobile', width: 390, height: 844 }]) {
  test(`${viewport.name}: companion chat, navigation and settings`, async ({ page }, testInfo) => {
    await page.setViewportSize(viewport);
    const state = await fixture(page);
    await page.goto('/');
    await expect(page.getByRole('heading', { name: 'Say something to Anima' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Team', exact: true })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Operations', exact: true })).toHaveCount(0);
    await expect(page.getByRole('combobox', { name: 'Chat with agent' })).toHaveCount(0);
    await page.screenshot({ path: testInfo.outputPath(`${viewport.name}-welcome.png`), fullPage: true });
    const input = page.getByPlaceholder('Message Anima…');
    await input.fill('Help me plan my day');
    await page.getByRole('button', { name: 'Activity', exact: true }).click();
    await page.getByRole('button', { name: 'Chat', exact: true }).click();
    await expect(input).toHaveValue('Help me plan my day');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await expect(page.getByText('Let’s make a little room in your day. What matters most today?')).toBeVisible();
    expect(state.sent()).toBe(1);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    await page.screenshot({ path: testInfo.outputPath(`${viewport.name}-conversation.png`), fullPage: true });
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    await expect(page.getByRole('dialog')).toBeVisible();
    await page.getByRole('button', { name: 'Close settings' }).click();
    await expect(input).toBeVisible();
  });
}

test('first run creates one companion without a team wizard', async ({ page }, testInfo) => {
  await fixture(page, true);
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'Set up your companion' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Start chatting' })).toBeEnabled();
  await page.screenshot({ path: testInfo.outputPath('setup.png'), fullPage: true });
  await page.getByRole('button', { name: 'Start chatting' }).click();
  await expect(page.getByPlaceholder('Message Anima…')).toBeVisible();
});
