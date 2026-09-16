import { test, expect } from '@playwright/test';

test('main workspace agent: healthy zero-agent daemon opens onboarding', async ({
  page,
}) => {
  await page.route('**/api/**', async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname.replace(/^\/api/, '');
    const body =
      path === '/health'
        ? { status: 'ok' }
        : path === '/agents'
          ? { agents: [] }
          : path === '/workspace'
            ? { configured: false, workspace: null, defaultRoot: '/workspace' }
            : path === '/providers'
              ? { providers: [] }
              : { error: `unexpected fixture request: ${path}` };
    await route.fulfill({
      status: ['/health', '/agents', '/providers', '/workspace'].includes(path)
        ? 200
        : 404,
      contentType: 'application/json',
      body: JSON.stringify(body),
    });
  });
  await page.goto('/');

  await expect(
    page.getByRole('heading', { name: 'Set up your companion' }),
  ).toBeVisible();
  await expect(page.getByRole('navigation')).toHaveCount(0);
});
