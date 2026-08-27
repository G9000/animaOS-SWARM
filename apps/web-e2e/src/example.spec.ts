import { test, expect } from '@playwright/test';

test('main workspace agent: boots the AnimaOS web surface', async ({
  page,
}) => {
  await page.goto('/');

  await expect(page.locator('body')).toContainText(/anima/i);
});
