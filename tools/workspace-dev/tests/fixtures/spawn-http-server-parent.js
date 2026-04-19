import { spawn } from 'node:child_process';

const workspaceRoot = process.argv[2];
const port = process.argv[3];

spawn('bun', ['x', 'nx', 'run', 'rust-daemon:dev'], {
  cwd: workspaceRoot,
  env: {
    ...process.env,
    ANIMAOS_RS_HOST: '127.0.0.1',
    ANIMAOS_RS_PORT: port,
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
