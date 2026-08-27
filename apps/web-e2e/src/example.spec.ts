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
          : path === '/providers'
            ? { providers: [] }
            : { error: `unexpected fixture request: ${path}` };
    await route.fulfill({
      status:
        path === '/health' || path === '/agents' || path === '/providers'
          ? 200
          : 404,
      contentType: 'application/json',
      body: JSON.stringify(body),
    });
  });
  await page.goto('/');

  await expect(
    page.getByRole('heading', { name: 'Create your main agent' }),
  ).toBeVisible();
  await expect(page.getByRole('navigation')).toHaveCount(0);
});
