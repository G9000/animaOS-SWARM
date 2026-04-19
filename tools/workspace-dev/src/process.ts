import { execFile, spawn, type ChildProcess } from 'node:child_process';
import { promisify } from 'node:util';

export interface ManagedProcessDefinition {
  name: string;
  command: string;
  args: string[];
  env?: Record<string, string>;
}

const execFileAsync = promisify(execFile);

export function spawnManagedProcess(
  definition: ManagedProcessDefinition
): ChildProcess {
  return spawn(definition.command, definition.args, {
    cwd: process.cwd(),
    env: {
      ...process.env,
      ...definition.env,
    },
    stdio: 'inherit',
    detached: process.platform !== 'win32',
  });
}

async function stopManagedProcess(child: ChildProcess): Promise<void> {
  if (child.exitCode !== null || child.pid === undefined) {
    return;
  }

  if (process.platform === 'win32') {
    try {
      await execFileAsync('taskkill', [
        '/PID',
        String(child.pid),
        '/T',
        '/F',
      ]);
    } catch {
      // A concurrent exit is fine during shutdown cleanup.
    }

    return;
  }

  try {
    process.kill(-child.pid, 'SIGTERM');
  } catch {
    // A concurrent exit is fine during shutdown cleanup.
  }
}

export async function runManagedProcesses(
  definitions: ManagedProcessDefinition[]
): Promise<void> {
  const children = definitions.map(spawnManagedProcess);
  let shuttingDown = false;

  const stopChildren = async () => {
    await Promise.all(children.map((child) => stopManagedProcess(child)));
  };

  const onSignal = () => {
    shuttingDown = true;
    void stopChildren();
  };

  process.once('SIGINT', onSignal);
  process.once('SIGTERM', onSignal);

  try {
    await Promise.race([
      new Promise<void>((resolve) => {
        process.once('SIGINT', resolve);
        process.once('SIGTERM', resolve);
      }),
      ...children.map(
        (child, index) =>
          new Promise<never>((_resolve, reject) => {
            child.once('exit', (code, signal) => {
              if (shuttingDown) {
                return;
              }

              reject(
                new Error(
                  `Process '${definitions[index]?.name ?? 'unknown'}' exited early with code ${String(
                    code
                  )} and signal ${String(signal)}.`
                )
              );
            });
            child.once('error', reject);
          })
      ),
    ]);

    if (shuttingDown) {
      await stopChildren();
      await Promise.all(
        children.map(
          (child) =>
            new Promise<void>((resolve) => {
              if (child.exitCode !== null) {
                resolve();
                return;
              }

              child.once('exit', () => resolve());
            })
        )
      );
    }
  } finally {
    process.removeListener('SIGINT', onSignal);
    process.removeListener('SIGTERM', onSignal);
    await stopChildren();
  }
}
