import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { daemon } from './daemon-api';
import {
  importLegacyCheckins,
  legacyImportKey,
  readLegacyCheckins,
} from './checkins';

describe('legacy check-in migration', () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => vi.restoreAllMocks());

  it('strictly projects valid timing and preserves malformed records', () => {
    localStorage.setItem(
      'animaos.checkins.agent-1',
      JSON.stringify([
        {
          id: 'valid',
          prompt: 'Check goals',
          intervalSecs: 60,
          createdAtMs: 1000,
          lastRunAtMs: 2000,
        },
        { id: 'bad', prompt: '', intervalSecs: -1, createdAtMs: 'nope' },
      ]),
    );
    const parsed = readLegacyCheckins('agent-1');
    expect(parsed.valid).toEqual([
      {
        id: 'valid',
        prompt: 'Check goals',
        intervalSecs: 60,
        createdAtMs: 1000,
        lastRunAtMs: 2000,
      },
    ]);
    expect(parsed.malformedCount).toBe(1);
    expect(legacyImportKey('agent-1', 'valid')).toBe('legacy:agent-1:valid');
  });

  it('clears local data only after a fully successful import', async () => {
    localStorage.setItem(
      'animaos.checkins.agent-1',
      JSON.stringify([
        {
          id: 'valid',
          prompt: 'Check goals',
          intervalSecs: 60,
          createdAtMs: 1000,
        },
      ]),
    );
    const imported = vi
      .spyOn(daemon, 'importLegacySchedules')
      .mockResolvedValue({ schedules: [] });
    expect(await importLegacyCheckins('agent-1')).toEqual({
      imported: 1,
      malformed: 0,
      complete: true,
    });
    expect(imported).toHaveBeenCalledWith('agent-1', {
      schedules: [
        {
          id: 'valid',
          prompt: 'Check goals',
          intervalSecs: 60,
          createdAtMs: 1000,
        },
      ],
    });
    expect(localStorage.getItem('animaos.checkins.agent-1')).toBeNull();
  });

  it('retains local data on import failure or malformed records', async () => {
    localStorage.setItem(
      'animaos.checkins.agent-1',
      JSON.stringify([{ id: 'bad' }]),
    );
    expect(await importLegacyCheckins('agent-1')).toEqual({
      imported: 0,
      malformed: 1,
      complete: false,
    });
    expect(localStorage.getItem('animaos.checkins.agent-1')).not.toBeNull();
    localStorage.setItem(
      'animaos.checkins.agent-1',
      JSON.stringify([
        { id: 'valid', prompt: 'Check', intervalSecs: 60, createdAtMs: 1 },
      ]),
    );
    vi.spyOn(daemon, 'importLegacySchedules').mockRejectedValue(
      new Error('offline'),
    );
    await expect(importLegacyCheckins('agent-1')).rejects.toThrow('offline');
    expect(localStorage.getItem('animaos.checkins.agent-1')).not.toBeNull();
  });

  it('imports valid siblings while retaining a mixed legacy payload for malformed recovery', async () => {
    localStorage.setItem(
      'animaos.checkins.agent-1',
      JSON.stringify([
        { id: 'valid', prompt: 'Check', intervalSecs: 60, createdAtMs: 1 },
        { id: 'bad' },
      ]),
    );
    const imported = vi
      .spyOn(daemon, 'importLegacySchedules')
      .mockResolvedValue({ schedules: [] });
    expect(await importLegacyCheckins('agent-1')).toEqual({
      imported: 1,
      malformed: 1,
      complete: false,
    });
    expect(imported).toHaveBeenCalledTimes(1);
    expect(localStorage.getItem('animaos.checkins.agent-1')).toContain('"bad"');
  });
});
