import type { DaemonClient } from './client.js';

/** What the owner can rely on (spec §8.1): only `active` skills load. */
export type SkillStatus = 'active' | 'changed' | 'missing' | 'invalid';

/** A registered skill, pinned to the hash the owner approved. */
export interface Skill {
  slug: string;
  /** The approved front matter's, never a changed file's. */
  name: string;
  description: string;
  enabled: boolean;
  status: SkillStatus;
  approvedHash: string;
  approvedAtMs: number;
  updatedAtMs: number;
}

/** What a SKILL.md holds now. Not approved: show it as text only. */
export interface SkillFile {
  /** Send it back to approve exactly this content; null when unreadable. */
  hash: string | null;
  name: string | null;
  description: string | null;
  body: string | null;
  problem: string | null;
}

export interface SkillDetail {
  skill: Skill | null;
  file: SkillFile | null;
}

export type SkillDraftSource = 'agent' | 'import' | 'file';
export type SkillDraftStatus = 'pending' | 'approved' | 'rejected';

export interface SkillDraftProposer {
  agentId: string;
  sessionId: string;
  runId: string;
}

/** A draft waiting for the owner, or decided (spec §8.2). Untrusted: the
 *  model or a file wrote it, so show it as text, never as markup. */
export interface SkillDraft {
  /** `skd_<uuid>`, or `file:<slug>` for a SKILL.md without a record. */
  id: string;
  slug: string;
  name: string;
  description: string;
  body: string;
  source: SkillDraftSource;
  proposedBy: SkillDraftProposer | null;
  baseHash: string | null;
  currentHash: string | null;
  /** The skill was approved again since this draft was made. */
  stale: boolean;
  /** A file draft's hash: send it back to approve it. */
  fileHash: string | null;
  createdAtMs: number;
  status: SkillDraftStatus;
  decidedAtMs: number | null;
  /** Why a file draft's SKILL.md is not valid. */
  problem: string | null;
}

export interface SkillInput {
  name: string;
  description: string;
  body: string;
  /** Absent: a new skill starts on, an existing one keeps its switch. */
  enabled?: boolean;
}

export interface SkillDraftApproval {
  /** The owner's edit of the body. */
  body?: string;
  /** Required for a file draft: the `fileHash` the owner reviewed. */
  hash?: string;
}

export interface ApprovedSkillDraft {
  skill: Skill;
  draft: SkillDraft;
}

/** The daemon's limits (spec §8.1, §16). */
export const MAX_SKILL_BODY_BYTES = 32 * 1024;
export const MAX_SKILL_NAME_CHARS = 64;
export const MAX_SKILL_DESCRIPTION_CHARS = 300;
/** The daemon also reserves `import` and the Windows device names (con, prn, aux, nul, com0-com9, lpt0-lpt9); the web checks them in lib/skills.ts. */
export const SKILL_SLUG_PATTERN = /^[a-z0-9][a-z0-9-]{0,63}$/;

function skillPath(slug: string): string {
  return `/api/skills/${encodeURIComponent(slug)}`;
}

function draftPath(id: string): string {
  return `/api/skill-drafts/${encodeURIComponent(id)}`;
}

export class SkillsClient {
  constructor(private readonly client: DaemonClient) {}

  /** Every skill by slug; the daemon rescans the folder first. */
  async list(options: { signal?: AbortSignal } = {}): Promise<Skill[]> {
    const response = await this.client.requestJson<{ skills: Skill[] }>(
      '/api/skills',
      { signal: options.signal },
    );
    return response.skills;
  }

  async get(
    slug: string,
    options: { signal?: AbortSignal } = {},
  ): Promise<SkillDetail> {
    return this.client.requestJson<SkillDetail>(skillPath(slug), {
      signal: options.signal,
    });
  }

  /** Creates or replaces a skill's content, which approves it. */
  async save(slug: string, input: SkillInput): Promise<Skill> {
    const body: SkillInput = {
      name: input.name,
      description: input.description,
      body: input.body,
    };
    if (input.enabled !== undefined) body.enabled = input.enabled;
    const response = await this.client.requestJson<{ skill: Skill }>(
      skillPath(slug),
      { method: 'PUT', body },
    );
    return response.skill;
  }

  async setEnabled(slug: string, enabled: boolean): Promise<Skill> {
    const response = await this.client.requestJson<{ skill: Skill }>(
      skillPath(slug),
      { method: 'PATCH', body: { enabled } },
    );
    return response.skill;
  }

  /** Moves the skill's folder to the workspace trash. */
  async remove(slug: string): Promise<{ trashPath: string | null }> {
    const response = await this.client.requestJson<{
      deleted: boolean;
      trashPath: string | null;
    }>(skillPath(slug), { method: 'DELETE' });
    return { trashPath: response.trashPath };
  }

  /** Approves a changed skill's current file, reviewed as `hash`. */
  async approve(slug: string, hash: string): Promise<Skill> {
    const response = await this.client.requestJson<{ skill: Skill }>(
      `${skillPath(slug)}/approve`,
      { method: 'POST', body: { hash } },
    );
    return response.skill;
  }

  /** `pending`: oldest first, files without a record included. `decided`:
   *  the last 30 days, newest first. */
  async drafts(options: {
    status: 'pending' | 'decided';
    signal?: AbortSignal;
  }): Promise<SkillDraft[]> {
    const search = new URLSearchParams({ status: options.status });
    const response = await this.client.requestJson<{ drafts: SkillDraft[] }>(
      `/api/skill-drafts?${search.toString()}`,
      { signal: options.signal },
    );
    return response.drafts;
  }

  async approveDraft(
    id: string,
    approval: SkillDraftApproval = {},
  ): Promise<ApprovedSkillDraft> {
    const body: SkillDraftApproval = {};
    if (approval.body !== undefined) body.body = approval.body;
    if (approval.hash !== undefined) body.hash = approval.hash;
    return this.client.requestJson<ApprovedSkillDraft>(
      `${draftPath(id)}/approve`,
      { method: 'POST', body },
    );
  }

  async rejectDraft(id: string): Promise<SkillDraft> {
    const response = await this.client.requestJson<{ draft: SkillDraft }>(
      `${draftPath(id)}/reject`,
      { method: 'POST' },
    );
    return response.draft;
  }

  /** Imports a SKILL.md as a pending draft (spec §8.4). */
  async importFile(
    file: Blob,
    options: { filename?: string; slug?: string } = {},
  ): Promise<SkillDraft> {
    const form = new FormData();
    form.append('file', file, options.filename ?? 'SKILL.md');
    if (options.slug) form.append('slug', options.slug);
    const response = await this.client.requestJson<{ draft: SkillDraft }>(
      '/api/skills/import',
      { method: 'POST', body: form },
    );
    return response.draft;
  }
}
