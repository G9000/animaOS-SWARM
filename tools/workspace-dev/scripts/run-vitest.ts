import { spawn } from 'node:child_process';

import { normalizeVitestArgs } from '../src/vitest-args.js';

const child = spawn(
  process.execPath,
  ['x', 'vitest', ...normalizeVitestArgs(process.argv.slice(2))],
  {
    cwd: process.cwd(),
    env: process.env,
    stdio: 'inherit',
  }
);

child.once('exit', (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
    return;
  }

  process.exit(code ?? 1);
});

child.once('error', (error) => {
  console.error(error);
  process.exit(1);
});
