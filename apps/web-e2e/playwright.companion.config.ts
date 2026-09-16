import { defineConfig, devices } from '@playwright/test';
import { workspaceRoot } from '@nx/devkit';

export default defineConfig({
  testDir: './src',
  testMatch: 'companion.spec.ts',
  outputDir: './test-output/companion',
  reporter: 'list',
  use: { baseURL: 'http://127.0.0.1:4271', trace: 'retain-on-failure', screenshot: 'only-on-failure' },
  webServer: {
    command: 'bun x nx run @animaOS-SWARM/web:preview -- --host 127.0.0.1 --port 4271',
    url: 'http://127.0.0.1:4271', cwd: workspaceRoot, reuseExistingServer: false,
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
});
