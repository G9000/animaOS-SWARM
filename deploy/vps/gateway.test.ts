// Exercise the actual Caddy configuration against a harmless local HTTP stub.
// CADDY_BINARY=/path/to/caddy bun test deploy/vps/gateway.test.ts
import { afterAll, beforeAll, expect, test } from 'bun:test';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const binary = process.env.CADDY_BINARY || 'caddy';
const owner = 'test-owner';
const password = 'test-only-password';
const apiKey = 'test-api-key-not-a-production-secret';
const admin = 'test-admin-token-not-a-production-secret';
const origin = 'https://companion.example.test';
const authorization = `Basic ${Buffer.from(`${owner}:${password}`).toString('base64')}`;
let base: string;
let folder: string;
let proxy: ReturnType<typeof Bun.spawn> | undefined;
let upstream: ReturnType<typeof Bun.serve> | undefined;
let hits = 0;

beforeAll(async () => {
  folder = await mkdtemp(join(tmpdir(), 'anima-gateway-test-'));
  upstream = Bun.serve({
    hostname: '127.0.0.1', port: 0,
    fetch(request) {
      hits += 1;
      if (new URL(request.url).pathname === '/api/test-stream') {
        return new Response(new ReadableStream({
          start(controller) {
            controller.enqueue(new TextEncoder().encode('data: first\n\n'));
            setTimeout(() => { controller.enqueue(new TextEncoder().encode('data: last\n\n')); controller.close(); }, 500);
          },
        }), { headers: { 'Content-Type': 'text/event-stream' } });
      }
      return Response.json(Object.fromEntries(request.headers));
    },
  });
  const reservation = Bun.serve({ hostname: '127.0.0.1', port: 0, fetch: () => new Response() });
  const port = reservation.port;
  reservation.stop(true);
  base = `http://127.0.0.1:${port}`;
  const hashProcess = Bun.spawn([binary, 'hash-password', '--plaintext', password], { stdout: 'pipe', stderr: 'pipe' });
  const hash = (await new Response(hashProcess.stdout).text()).trim();
  expect(await hashProcess.exited).toBe(0);
  // Keep the real security handlers unchanged; only isolate addresses/TLS.
  const source = await readFile(join(import.meta.dir, 'Caddyfile'), 'utf8');
  const config = source
    .replace('email {$ACME_EMAIL}', 'auto_https off')
    .replace('http://127.0.0.1:9090', 'http://127.0.0.1:0')
    .replace('{$ANIMA_DOMAIN} {', `${base} {`)
    .replace('reverse_proxy 127.0.0.1:8080', `reverse_proxy 127.0.0.1:${upstream.port}`);
  const path = join(folder, 'Caddyfile');
  await writeFile(path, config);
  proxy = Bun.spawn([binary, 'run', '--config', path, '--adapter', 'caddyfile'], {
    env: { ...process.env, ANIMA_DOMAIN: 'companion.example.test', ANIMA_OWNER_USER: owner, ANIMA_OWNER_PASSWORD_HASH: hash, ANIMA_INTERNAL_API_KEY: apiKey, ANIMA_LOCAL_ADMIN_TOKEN: admin },
    stdout: 'pipe', stderr: 'pipe',
  });
  for (let attempt = 0; attempt < 50; attempt += 1) {
    try { await fetch(`${base}/`); return; } catch { await Bun.sleep(100); }
  }
  throw new Error(`Caddy failed to start: ${await new Response(proxy.stderr).text()}`);
}, 15000);

afterAll(async () => {
  proxy?.kill();
  if (proxy) await proxy.exited;
  upstream?.stop(true);
  if (folder) await rm(folder, { recursive: true, force: true });
});

test('site and API require owner authentication', async () => {
  for (const path of ['/', '/api', '/api/agents', '/health', '/ready', '/metrics', '/openapi.json', '/docs']) {
    expect((await fetch(`${base}${path}`)).status).toBe(401);
  }
});

test('proxy supplies separate owner/API credentials and removes every forwarding header', async () => {
  const response = await fetch(`${base}/api/agents`, { headers: {
    authorization, Origin: origin, Forwarded: 'for=attacker', Via: 'attacker',
    'X-Forwarded-For': 'attacker', 'X-Forwarded-Evil': 'attacker', 'X-Real-IP': 'attacker',
    'Client-IP': 'attacker', 'True-Client-IP': 'attacker', 'CF-Connecting-IP': 'attacker',
    'X-Api-Key': 'attacker',
  } });
  expect(response.status).toBe(200);
  const headers = await response.json();
  expect(headers.host).toBe('127.0.0.1:8080');
  expect(headers.authorization).toBe(`Bearer ${admin}`);
  expect(headers['x-api-key']).toBe(apiKey);
  for (const name of Object.keys(headers)) {
    expect(name.startsWith('x-forwarded-')).toBe(false);
    expect(['origin', 'forwarded', 'via', 'x-real-ip', 'client-ip', 'true-client-ip', 'cf-connecting-ip'].includes(name)).toBe(false);
  }
});

test('cross-origin reads/writes, missing-origin writes, and websocket upgrades never reach daemon', async () => {
  const previousHits = hits;
  const cases = [
    { method: 'GET', headers: { Origin: 'https://evil.example' } },
    { method: 'GET', headers: { Origin: 'null' } },
    { method: 'GET', headers: { 'Sec-Fetch-Site': 'cross-site' } },
    { method: 'GET', headers: { 'Sec-Fetch-Site': 'same-site' } },
    { method: 'POST', headers: {} },
    { method: 'POST', headers: { Origin: 'https://evil.example' } },
    { method: 'GET', headers: { Upgrade: 'websocket', Connection: 'Upgrade' } },
    { method: 'GET', headers: { Upgrade: 'websocket', Connection: 'Upgrade', Origin: 'https://evil.example' } },
  ];
  for (const request of cases) {
    expect((await fetch(`${base}/api/agents`, { ...request, headers: { authorization, ...request.headers } })).status).toBe(403);
  }
  expect(hits).toBe(previousHits);
});

test('same-origin writes pass and callback exceptions are exact GET paths', async () => {
  expect((await fetch(`${base}/api/agents`, { method: 'POST', headers: { authorization, Origin: origin } })).status).toBe(200);
  for (const callback of ['/api/connectors/gcalendar/callback', '/api/connectors/mail/gmail/callback', '/api/connectors/mail/outlook/callback']) {
    const headers = { authorization, 'Sec-Fetch-Site': 'cross-site' };
    expect((await fetch(`${base}${callback}`, { headers })).status).toBe(200);
    expect((await fetch(`${base}${callback}/extra`, { headers })).status).toBe(403);
    expect((await fetch(`${base}${callback}`, { method: 'POST', headers })).status).toBe(403);
  }
});

test('SSE first event arrives while the upstream response is still open', async () => {
  const started = Date.now();
  const response = await fetch(`${base}/api/test-stream`, { headers: { authorization } });
  expect(response.status).toBe(200);
  const reader = response.body!.getReader();
  const first = await reader.read();
  expect(new TextDecoder().decode(first.value)).toContain('data: first');
  expect(new TextDecoder().decode(first.value)).not.toContain('data: last');
  expect(Date.now() - started).toBeLessThan(450);
  await reader.cancel();
});
