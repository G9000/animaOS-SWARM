import { defineConfig, devices } from '@playwright/test';
import { nxE2EPreset } from '@nx/playwright/preset';
import { workspaceRoot } from '@nx/devkit';

const serverPort = 4300;
const uiPort = 4301;
const serverOrigin = `http://127.0.0.1:${String(serverPort)}`;
const baseURL = process.env['BASE_URL'] || `http://127.0.0.1:${String(uiPort)}`;

export default defineConfig({
  ...nxE2EPreset(__filename, { testDir: './src' }),
  use: {
    baseURL,
    trace: 'on-first-retry',
  },
  webServer: [
    {
      command: 'bun x nx run @animaOS-SWARM/server:serve',
      url: `${serverOrigin}/api/health`,
      reuseExistingServer: false,
      cwd: workspaceRoot,
      timeout: 120_000,
      env: {
        ANIMA_MODEL_ADAPTER: 'mock',
        OPENAI_API_KEY: 'test-key',
        PORT: String(serverPort),
      },
    },
    {
      command: `bun x nx run @animaOS-SWARM/web:serve -- --host 127.0.0.1 --port ${String(
        uiPort
      )}`,
      url: baseURL,
      reuseExistingServer: false,
      cwd: workspaceRoot,
      timeout: 120_000,
      env: {
        CI: '',
        UI_BACKEND_ORIGIN: serverOrigin,
        UI_SUPPRESS_WS_PROXY_RESET: '1',
      },
    },
  ],
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
    {
      name: 'firefox',
      use: { ...devices['Desktop Firefox'] },
    },
    {
      name: 'webkit',
      use: { ...devices['Desktop Safari'] },
    },
  ],
});
