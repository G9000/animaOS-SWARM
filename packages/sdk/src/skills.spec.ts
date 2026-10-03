import { describe, expect, it } from 'vitest';

import {
  createDaemonClient,
  MAX_SKILL_BODY_BYTES,
  SKILL_SLUG_PATTERN,
  type Skill,
  type SkillDraft,
} from './index.js';

function transport(respond: (url: string, init?: RequestInit) => Response) {
  const requests: { url: string; init?: RequestInit }[] = [];
  const client = createDaemonClient({
    baseUrl: '',
    fetch: async (url, init) => {
      requests.push({ url: String(url), init });
      return respond(String(url), init);
    },
  });
  return { skills: client.skills, requests };
}

const skill: Skill = {
  slug: 'notes',
  name: 'Notes',
  description: 'Take notes',
  enabled: true,
  status: 'active',
  approvedHash: 'a'.repeat(64),
  approvedAtMs: 1,
  updatedAtMs: 1,
};

const draft: SkillDraft = {
  id: 'file:found',
  slug: 'found',
  name: 'Found',
  description: 'd',
  body: 'b',
  source: 'file',
  proposedBy: null,
  baseHash: null,
  currentHash: null,
  stale: false,
  fileHash: 'f'.repeat(64),
  createdAtMs: 2,
  status: 'pending',
  decidedAtMs: null,
  problem: null,
};

describe('skills client', () => {
  it('lists, reads, saves, toggles, removes, and approves skills', async () => {
    const { skills, requests } = transport((url, init) => {
      if (init?.method === 'DELETE')
        return Response.json({
          deleted: true,
          trashPath: '.anima-trash/skills/notes-1',
        });
      if (url.endsWith('/api/skills'))
        return Response.json({ skills: [skill] });
      if (!init?.method) return Response.json({ skill, file: null });
      return Response.json({ skill });
    });

    expect(await skills.list()).toEqual([skill]);
    expect(await skills.get('notes')).toEqual({ skill, file: null });
    expect(
      await skills.save('notes', {
        name: 'Notes',
        description: 'Take notes',
        body: 'b',
      }),
    ).toEqual(skill);
    await skills.setEnabled('notes', false);
    expect(await skills.remove('notes')).toEqual({
      trashPath: '.anima-trash/skills/notes-1',
    });
    await skills.approve('notes', 'c'.repeat(64));

    expect(
      requests.map(({ url, init }) => [init?.method ?? 'GET', url, init?.body]),
    ).toEqual([
      ['GET', '/api/skills', undefined],
      ['GET', '/api/skills/notes', undefined],
      [
        'PUT',
        '/api/skills/notes',
        JSON.stringify({ name: 'Notes', description: 'Take notes', body: 'b' }),
      ],
      ['PATCH', '/api/skills/notes', JSON.stringify({ enabled: false })],
      ['DELETE', '/api/skills/notes', undefined],
      [
        'POST',
        '/api/skills/notes/approve',
        JSON.stringify({ hash: 'c'.repeat(64) }),
      ],
    ]);
  });

  it('lists, approves, and rejects drafts with encoded ids', async () => {
    const { skills, requests } = transport((url) => {
      if (url.startsWith('/api/skill-drafts?'))
        return Response.json({ drafts: [draft] });
      if (url.endsWith('/approve'))
        return Response.json({
          skill,
          draft: { ...draft, status: 'approved' },
        });
      return Response.json({ draft: { ...draft, status: 'rejected' } });
    });

    expect(await skills.drafts({ status: 'pending' })).toEqual([draft]);
    const approved = await skills.approveDraft('file:found', {
      hash: draft.fileHash!,
    });
    expect(approved.skill).toEqual(skill);
    expect((await skills.rejectDraft('skd_1')).status).toBe('rejected');
    await skills.approveDraft('skd_2');

    expect(requests.map(({ url, init }) => [url, init?.body])).toEqual([
      ['/api/skill-drafts?status=pending', undefined],
      [
        '/api/skill-drafts/file%3Afound/approve',
        JSON.stringify({ hash: draft.fileHash }),
      ],
      ['/api/skill-drafts/skd_1/reject', undefined],
      ['/api/skill-drafts/skd_2/approve', JSON.stringify({})],
    ]);
  });

  it('imports a SKILL.md as multipart form data', async () => {
    const { skills, requests } = transport(() =>
      Response.json({ draft: { ...draft, source: 'import' } }, { status: 201 }),
    );

    const imported = await skills.importFile(
      new Blob(['---\nname: n\n---\n\nb'], { type: 'text/markdown' }),
      { filename: 'SKILL.md', slug: 'chosen' },
    );

    expect(imported.source).toBe('import');
    const body = requests[0].init?.body;
    expect(requests[0].url).toBe('/api/skills/import');
    expect(body).toBeInstanceOf(FormData);
    const form = body as FormData;
    expect((form.get('file') as File).name).toBe('SKILL.md');
    expect(form.get('slug')).toBe('chosen');
    expect(
      new Headers(requests[0].init?.headers).get('content-type'),
    ).toBeNull();
  });

  it('exports the daemon limits', () => {
    expect(MAX_SKILL_BODY_BYTES).toBe(32 * 1024);
    expect(SKILL_SLUG_PATTERN.test('weekly-review')).toBe(true);
    expect(SKILL_SLUG_PATTERN.test('-x')).toBe(false);
  });
});
