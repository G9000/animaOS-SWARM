import { spawn, type ChildProcess } from 'node:child_process';

export interface ManagedProcessDefinition {
  name: string;
  command: string;
  args: string[];
  env?: Record<string, string>;
}

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
  });
}

export async function runManagedProcesses(
  definitions: ManagedProcessDefinition[]
): Promise<void> {
  const children = definitions.map(spawnManagedProcess);
  let shuttingDown = false;

  const stopChildren = () => {
    for (const child of children) {
      if (child.exitCode === null) {
        child.kill();
      }
    }
  };

  const onSignal = () => {
    shuttingDown = true;
    stopChildren();
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
    stopChildren();
  }
}
