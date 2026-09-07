import { expect, it } from 'vitest';
import { createDaemonClient } from './index.js';

it('lists workspace files and encodes relative paths for text preview', async () => {
  const urls: string[] = [];
  const client = createDaemonClient({ baseUrl: '', fetch: async (url) => {
    urls.push(String(url));
    return Response.json(String(url).includes('/files') ? { files: [{ path: 'notes/a #1.md', name: 'a #1.md', sizeBytes: 12, modifiedAtMs: null }], truncated: true } : { path: 'notes/a #1.md', content: 'Hello', truncated: false });
  } });
  expect(await client.workspace.listFiles()).toMatchObject({ truncated: true, files: [{ modifiedAtMs: null }] });
  expect(await client.workspace.readFile('notes/a #1.md')).toMatchObject({ content: 'Hello' });
  expect(urls).toEqual(['/api/workspace/files', '/api/workspace/file?path=notes%2Fa%20%231.md']);
});
