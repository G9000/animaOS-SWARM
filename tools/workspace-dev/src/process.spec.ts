import { execFileSync } from 'node:child_process';
import { createServer } from 'node:net';
import { resolve } from 'node:path';

import { afterEach, describe, expect, it } from 'vitest';

import { runManagedProcesses } from './process.js';

const parentFixturePath = resolve(
  import.meta.dirname,
  '../tests/fixtures/spawn-http-server-parent.js'
);
const workspaceRoot = resolve(import.meta.dirname, '../../..');

describe.skipIf(process.platform !== 'win32').sequential(
  'workspace-dev process management',
  () => {
    let leakedPid: number | null = null;

    afterEach(() => {
      if (leakedPid === null) {
        return;
      }

      try {
        process.kill(leakedPid, 'SIGKILL');
      } catch {
        // Best-effort cleanup for leaked fixture descendants.
      } finally {
        leakedPid = null;
      }
    });

    it(
      'stops descendant processes when the launcher shuts down',
      async () => {
        const port = await reservePort();
        let runSettled = false;
        const runPromise = runManagedProcesses([
          {
            name: 'fixture-parent',
            command: process.execPath,
            args: [parentFixturePath, workspaceRoot, String(port)],
          },
        ]).finally(() => {
          runSettled = true;
        });

        try {
          const ready = await Promise.race([
            waitForCondition(() => isHealthOk(port), 90_000),
            runPromise.then(() => false),
          ]);
          expect(ready).toBe(true);

          process.emit('SIGINT');
          await runPromise;

          const descendantStopped = await waitForCondition(
            async () => !(await isHealthOk(port)),
            5_000
          );

          if (!descendantStopped) {
            leakedPid = findListeningPid(port);
          } else {
            leakedPid = null;
          }

          expect(descendantStopped).toBe(true);
        } finally {
          if (!runSettled) {
            process.emit('SIGINT');
            await runPromise.catch(() => undefined);
          }
        }
      },
      120_000
    );
  }
);

async function reservePort(): Promise<number> {
  return await new Promise((resolvePort, reject) => {
    const server = createServer();
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      if (!address || typeof address === 'string') {
        reject(new Error('failed to reserve test port'));
        return;
      }

      server.close((error) => {
        if (error) {
          reject(error);
          return;
        }

        resolvePort(address.port);
      });
    });
    server.once('error', reject);
  });
}

async function waitForCondition(
  predicate: () => Promise<boolean>,
  timeoutMs = 10_000,
  intervalMs = 100
): Promise<boolean> {
  const deadline = Date.now() + timeoutMs;

  while (Date.now() < deadline) {
    if (await predicate()) {
      return true;
    }

    await new Promise((resolve) => setTimeout(resolve, intervalMs));
  }

  return false;
}

async function isHealthOk(port: number): Promise<boolean> {
  try {
    const response = await fetch(
      `http://127.0.0.1:${String(port)}/api/health`
    );
    return response.ok;
  } catch {
    return false;
  }
}

function findListeningPid(port: number): number | null {
  const output = execFileSync('netstat', ['-ano'], {
    encoding: 'utf8',
  });
  const matchingLine = output
    .split(/\r?\n/)
    .find(
      (line) =>
        line.includes(`127.0.0.1:${String(port)}`) &&
        line.includes('LISTENING')
    );

  if (!matchingLine) {
    return null;
  }

  const parts = matchingLine.trim().split(/\s+/);
  const pid = Number(parts.at(-1));
  return Number.isFinite(pid) ? pid : null;
}
