import { spawn } from 'node:child_process';
import { resolve } from 'node:path';

const workspaceRoot = process.argv[2];
const port = process.argv[3];
const targetDir = resolve(workspaceRoot, 'target', 'workspace-dev-fixture');

spawn('bun', ['x', 'nx', 'run', 'rust-daemon:dev'], {
  cwd: workspaceRoot,
  env: {
    ...process.env,
    ANIMAOS_RS_HOST: '127.0.0.1',
    ANIMAOS_RS_PORT: port,
    CARGO_TARGET_DIR: process.env.CARGO_TARGET_DIR ?? targetDir,
  },
  stdio: 'ignore',
});

const holdOpen = setInterval(() => {}, 1000);

const shutdown = () => {
  clearInterval(holdOpen);
  process.exit(0);
};

process.on('SIGINT', shutdown);
process.on('SIGTERM', shutdown);
