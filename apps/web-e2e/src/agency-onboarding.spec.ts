/// <reference lib="dom" />
import { test, expect } from '@playwright/test';

for (const viewport of [
  { width: 1280, height: 900 },
  { width: 390, height: 844 },
]) {
  test(`agency template review at ${viewport.width}px`, async ({
    page,
  }, testInfo) => {
    await page.setViewportSize(viewport);
    await page.route('**/api/**', async (route) => {
      const path = new URL(route.request().url()).pathname.replace(
        /^\/api/,
        '',
      );
      const fixtures: Record<string, unknown> = {
        '/health': { status: 'ok' },
        '/agents': { agents: [] },
        '/workspace': {
          configured: false,
          workspace: null,
          defaultRoot: '/tmp/agency-preview',
        },
        '/providers': {
          providers: [
            {
              id: 'deterministic',
              label: 'Deterministic',
              configured: true,
              requiresKey: false,
              apiKeyEnvs: [],
            },
          ],
        },
      };
      await route.fulfill({
        status: path in fixtures ? 200 : 404,
        contentType: 'application/json',
        body: JSON.stringify(
          fixtures[path] ?? { error: 'Not in preview fixture' },
        ),
      });
    });
    await page.goto('/');
    await page.getByRole('button', { name: /Creator Studio/ }).click();
    await expect(
      page.getByRole('button', { name: 'Change template', exact: true }),
    ).toBeVisible();
    await expect(
      page.getByRole('button', { name: /Start from scratch/ }),
    ).toHaveCount(0);
    await expect(
      page.getByRole('textbox', { name: 'Company name' }),
    ).toHaveValue('My Creator Studio');
    await page.screenshot({
      path: testInfo.outputPath('agency-picker.png'),
      fullPage: false,
      animations: 'disabled',
    });
    await page.getByRole('button', { name: 'Next', exact: true }).click();
    await page.getByRole('button', { name: 'Next', exact: true }).click();
    await expect(
      page.getByRole('heading', { name: 'Shape your team', exact: true }),
    ).toBeVisible();
    await expect(
      page.getByRole('textbox', { name: 'Manager name' }),
    ).toHaveCount(0);
    await page.screenshot({
      path: testInfo.outputPath('agency-team.png'),
      fullPage: false,
      animations: 'disabled',
    });
    await page
      .getByRole('button', { name: 'Edit Content Planner', exact: true })
      .click();
    await expect(
      page.getByRole('textbox', { name: 'Specialist 1 name' }),
    ).toHaveValue('Content Planner');
    await page
      .getByRole('textbox', { name: 'Specialist 1 name' })
      .fill('Editorial Planner');
    await page
      .getByRole('button', { name: 'Remove Community Manager' })
      .click();
    await page.getByRole('button', { name: 'Next', exact: true }).click();
    await expect(
      page.getByRole('heading', { name: 'Workspace Manager' }),
    ).toBeVisible();
    await expect(
      page.getByRole('textbox', { name: 'Manager name' }),
    ).toHaveValue('Anima');
    await expect(page.getByRole('radio', { name: /^Balanced/ })).toBeChecked();
    await expect(page.getByRole('radio', { name: /^Concise/ })).toBeChecked();
    await page.getByRole('radio', { name: /^Proactive/ }).check();
    await page.getByRole('radio', { name: /^Detailed/ }).check();
    await page
      .getByRole('textbox', { name: 'Workspace preferences' })
      .fill('Keep the editorial calendar current.');
    await page.getByText('View manager instructions').click();
    await expect(
      page.getByText(/You are Anima, the workspace manager/),
    ).toContainText('Keep the editorial calendar current.');
    await page.getByText('View manager instructions').click();
    await page
      .getByRole('heading', { name: 'Workspace Manager' })
      .scrollIntoViewIfNeeded();
    await page.screenshot({
      path: testInfo.outputPath('workspace-manager.png'),
      fullPage: false,
      animations: 'disabled',
    });
    await page.getByRole('radio', { name: /^Observe/ }).check();
    await page.getByRole('button', { name: 'Next', exact: true }).click();
    await expect(
      page.getByRole('heading', { name: 'Review', exact: true }),
    ).toBeVisible();
    await expect(
      page
        .getByRole('region', { name: 'Review', exact: true })
        .getByText('Editorial Planner', { exact: true }),
    ).toBeVisible();
    await expect(
      page
        .getByRole('region', { name: 'Review', exact: true })
        .getByText('Workspace Manager', { exact: true }),
    ).toBeVisible();
    await expect(page.getByText(/^Proactive$/i)).toBeVisible();
    await expect(page.getByText(/^Detailed$/i)).toBeVisible();
    await expect(
      page.getByRole('button', { name: 'Create agency' }),
    ).toBeEnabled();
    const overflow = await page.evaluate(() =>
      Array.from(document.querySelectorAll('body *'))
        .filter((element) => {
          const rect = element.getBoundingClientRect();
          const style = getComputedStyle(element);
          return (
            style.position !== 'absolute' &&
            style.position !== 'fixed' &&
            rect.width > innerWidth + 1
          );
        })
        .map((element) => element.tagName),
    );
    expect(overflow).toEqual([]);
    await page.locator('.setup-shell').evaluate((element) => {
      element.scrollTop = 0;
    });
    await page.screenshot({
      path: testInfo.outputPath('agency-review.png'),
      fullPage: false,
      animations: 'disabled',
    });
  });
}
