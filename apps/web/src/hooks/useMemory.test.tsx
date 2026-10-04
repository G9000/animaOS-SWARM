import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { DaemonHttpError } from '@animaOS-SWARM/sdk';

import { COMPANION_UNREACHABLE } from '../lib/approvals';
import { daemon } from '../lib/daemon-api';
import { ENTITY_GONE, FACT_GONE, MEMORY_GONE } from '../lib/memory';
import {
  entityFixture,
  factFixture,
  memoryFixture,
  relationshipFixture,
} from '../test/memory';
import { useMemory } from './useMemory';

const deleted = {
  id: 'm1',
  removedRelationships: 0,
  updatedRelationships: 0,
  updatedFacts: 0,
};

beforeEach(() => {
  vi.spyOn(daemon, 'recentMemories').mockResolvedValue([memoryFixture('m1')]);
  vi.spyOn(daemon, 'searchMemories').mockResolvedValue([
    { ...memoryFixture('m2'), score: 0.8 },
  ]);
  vi.spyOn(daemon, 'listFacts').mockResolvedValue([factFixture('f1')]);
  vi.spyOn(daemon, 'listMemoryEntities').mockResolvedValue([
    entityFixture('e1'),
  ]);
  vi.spyOn(daemon, 'listMemoryRelationships').mockResolvedValue([
    relationshipFixture('r1'),
  ]);
  vi.spyOn(daemon, 'updateMemory').mockResolvedValue(memoryFixture('m1'));
  vi.spyOn(daemon, 'deleteMemory').mockResolvedValue(deleted);
  vi.spyOn(daemon, 'replaceFact').mockResolvedValue({
    fact: factFixture('f2'),
    superseded: factFixture('f1', { status: 'superseded' }),
  });
  vi.spyOn(daemon, 'deleteFact').mockResolvedValue({ id: 'f1' });
  vi.spyOn(daemon, 'deleteMemoryEntity').mockResolvedValue({
    kind: 'external',
    id: 'e1',
    removedRelationships: 0,
    removedFacts: 0,
  });
  vi.spyOn(daemon, 'traceMemory').mockResolvedValue({
    memory: memoryFixture('m1'),
    relationships: [],
    entities: [],
  });
});

afterEach(() => {
  vi.restoreAllMocks();
});

const online = { agentId: 'agent-main', epoch: 1, enabled: true };

async function loaded(options = online) {
  const hook = renderHook((props) => useMemory(props), {
    initialProps: options,
  });
  await waitFor(() => expect(hook.result.current.loaded).toBe(true));
  return hook;
}

describe('useMemory', () => {
  it('loads the recent memories, facts, entities, and relationships for the companion', async () => {
    const { result } = await loaded();
    expect(result.current.memories.map((m) => m.id)).toEqual(['m1']);
    expect(result.current.facts.map((f) => f.id)).toEqual(['f1']);
    expect(result.current.entities.map((e) => e.id)).toEqual(['e1']);
    expect(result.current.relationships.map((r) => r.id)).toEqual(['r1']);
    expect(result.current.error).toBeNull();
    expect(daemon.recentMemories).toHaveBeenCalledWith('agent-main', 200);
    expect(daemon.listFacts).toHaveBeenCalledWith({
      agentId: 'agent-main',
      includeInactive: false,
      limit: 500,
    });
    expect(daemon.listMemoryEntities).toHaveBeenCalledWith(200);
    expect(daemon.listMemoryRelationships).toHaveBeenCalledWith(
      'agent-main',
      200,
    );
  });

  it('search reads the matching memories and an empty search returns to the recent list', async () => {
    const { result } = await loaded();

    act(() => result.current.search('tea'));
    await waitFor(() =>
      expect(result.current.memories.map((m) => m.id)).toEqual(['m2']),
    );
    expect(result.current.query).toBe('tea');
    expect(daemon.searchMemories).toHaveBeenCalledWith(
      'tea',
      'agent-main',
      200,
    );

    act(() => result.current.search(''));
    await waitFor(() =>
      expect(result.current.memories.map((m) => m.id)).toEqual(['m1']),
    );
  });

  it('includeReplaced reads the facts again with includeInactive', async () => {
    const { result } = await loaded();

    act(() => result.current.setIncludeReplaced(true));

    await waitFor(() =>
      expect(daemon.listFacts).toHaveBeenLastCalledWith({
        agentId: 'agent-main',
        includeInactive: true,
        limit: 500,
      }),
    );
    expect(result.current.includeReplaced).toBe(true);
  });

  it('a stale read is ignored', async () => {
    let releaseSlow: (value: ReturnType<typeof memoryFixture>[]) => void = () =>
      undefined;
    vi.mocked(daemon.recentMemories).mockImplementationOnce(
      () => new Promise((resolve) => (releaseSlow = resolve)),
    );
    const hook = renderHook((props) => useMemory(props), {
      initialProps: online,
    });
    await waitFor(() => expect(daemon.recentMemories).toHaveBeenCalledTimes(1));
    vi.mocked(daemon.recentMemories).mockResolvedValue([
      memoryFixture('fresh'),
    ]);

    hook.rerender({ ...online, epoch: 2 });
    await waitFor(() =>
      expect(hook.result.current.memories.map((m) => m.id)).toEqual(['fresh']),
    );
    await act(async () => {
      releaseSlow([memoryFixture('stale')]);
    });

    expect(hook.result.current.memories.map((m) => m.id)).toEqual(['fresh']);
  });

  it('a failed list keeps the others and sets the error', async () => {
    const { result } = await loaded();
    vi.mocked(daemon.listFacts).mockRejectedValue(
      new DaemonHttpError(503, { error: 'Memory is busy' }),
    );
    vi.mocked(daemon.recentMemories).mockResolvedValue([memoryFixture('m9')]);

    act(() => result.current.refresh());

    await waitFor(() => expect(result.current.error).toBe('Memory is busy'));
    expect(result.current.memories.map((m) => m.id)).toEqual(['m9']);
    expect(result.current.facts.map((f) => f.id)).toEqual(['f1']);
  });

  it('an empty reload keeps the empty arrays', async () => {
    vi.mocked(daemon.recentMemories).mockResolvedValue([]);
    vi.mocked(daemon.listFacts).mockResolvedValue([]);
    vi.mocked(daemon.listMemoryEntities).mockResolvedValue([]);
    vi.mocked(daemon.listMemoryRelationships).mockResolvedValue([]);
    const { result } = await loaded();
    const before = {
      memories: result.current.memories,
      facts: result.current.facts,
      entities: result.current.entities,
      relationships: result.current.relationships,
    };

    act(() => result.current.refresh());
    await waitFor(() => expect(daemon.recentMemories).toHaveBeenCalledTimes(2));
    await act(async () => undefined);

    expect(result.current.memories).toBe(before.memories);
    expect(result.current.facts).toBe(before.facts);
    expect(result.current.entities).toBe(before.entities);
    expect(result.current.relationships).toBe(before.relationships);
  });

  it('edit sends the patch and reloads', async () => {
    const { result } = await loaded();
    const memory = memoryFixture('m1');

    let ok = false;
    await act(async () => {
      ok = await result.current.edit(memory, { content: 'New text' });
    });

    expect(ok).toBe(true);
    expect(daemon.updateMemory).toHaveBeenCalledWith('m1', {
      content: 'New text',
    });
    expect(daemon.recentMemories).toHaveBeenCalledTimes(2);
  });

  it('a 404 on remove says the memory is gone and reloads', async () => {
    const { result } = await loaded();
    vi.mocked(daemon.deleteMemory).mockRejectedValue(
      new DaemonHttpError(404, { error: 'memory not found' }),
    );

    let ok = true;
    await act(async () => {
      ok = await result.current.remove(memoryFixture('m1'));
    });

    expect(ok).toBe(false);
    expect(result.current.error).toBe(MEMORY_GONE);
    expect(result.current.errorStatus).toBe(404);
    expect(daemon.recentMemories).toHaveBeenCalledTimes(2);
  });

  it('remove, replaceFact, removeFact, and removeEntity call the daemon and reload', async () => {
    const { result } = await loaded();
    const reads = () => vi.mocked(daemon.recentMemories).mock.calls.length;

    await act(async () => {
      expect(await result.current.remove(memoryFixture('m1'))).toBe(true);
    });
    expect(daemon.deleteMemory).toHaveBeenCalledWith('m1');
    expect(reads()).toBe(2);

    await act(async () => {
      expect(
        await result.current.replaceFact(factFixture('f1'), 'Long answers'),
      ).toBe(true);
    });
    expect(daemon.replaceFact).toHaveBeenCalledWith('f1', 'Long answers');
    expect(reads()).toBe(3);

    await act(async () => {
      expect(await result.current.removeFact(factFixture('f1'))).toBe(true);
    });
    expect(daemon.deleteFact).toHaveBeenCalledWith('f1');
    expect(reads()).toBe(4);

    await act(async () => {
      expect(
        await result.current.removeEntity(
          entityFixture('e1', { kind: 'user' }),
        ),
      ).toBe(true);
    });
    expect(daemon.deleteMemoryEntity).toHaveBeenCalledWith('user', 'e1');
    expect(reads()).toBe(5);
    expect(result.current.error).toBeNull();
  });

  it('answers a 404 on a fact or an entity with its own text and a 409 with the daemon text', async () => {
    const { result } = await loaded();
    vi.mocked(daemon.deleteFact).mockRejectedValue(
      new DaemonHttpError(404, { error: 'fact not found' }),
    );
    await act(async () => {
      await result.current.removeFact(factFixture('f1'));
    });
    expect(result.current.error).toBe(FACT_GONE);

    vi.mocked(daemon.deleteMemoryEntity).mockRejectedValue(
      new DaemonHttpError(404, { error: 'entity not found' }),
    );
    await act(async () => {
      await result.current.removeEntity(entityFixture('e1'));
    });
    expect(result.current.error).toBe(ENTITY_GONE);

    vi.mocked(daemon.deleteMemoryEntity).mockRejectedValue(
      new DaemonHttpError(409, {
        error: 'This entity still has memories; delete them first',
      }),
    );
    await act(async () => {
      await result.current.removeEntity(entityFixture('e1'));
    });
    expect(result.current.error).toBe(
      'This entity still has memories; delete them first',
    );
    expect(result.current.errorStatus).toBe(409);
  });

  it('a network failure says the companion is unreachable', async () => {
    const { result } = await loaded();
    vi.mocked(daemon.updateMemory).mockRejectedValue(new TypeError('offline'));

    await act(async () => {
      expect(await result.current.edit(memoryFixture('m1'), {})).toBe(false);
    });

    expect(result.current.error).toBe(COMPANION_UNREACHABLE);
    expect(result.current.errorStatus).toBeNull();
  });

  it('trace returns the evidence or null', async () => {
    const { result } = await loaded();

    let trace: Awaited<ReturnType<typeof result.current.trace>> = null;
    await act(async () => {
      trace = await result.current.trace(memoryFixture('m1'));
    });
    expect(trace).toEqual(expect.objectContaining({ relationships: [] }));
    expect(daemon.recentMemories).toHaveBeenCalledTimes(1);

    vi.mocked(daemon.traceMemory).mockRejectedValue(
      new DaemonHttpError(404, { error: 'memory not found' }),
    );
    await act(async () => {
      trace = await result.current.trace(memoryFixture('m1'));
    });
    expect(trace).toBeNull();
    expect(result.current.error).toBe(MEMORY_GONE);
    expect(daemon.recentMemories).toHaveBeenCalledTimes(1);
  });

  it('nothing is read while offline or without an agent', async () => {
    const hook = renderHook((props) => useMemory(props), {
      initialProps: { agentId: 'agent-main', epoch: 1, enabled: false },
    });
    hook.rerender({ agentId: null, epoch: 1, enabled: true });
    await act(async () => undefined);

    expect(daemon.recentMemories).not.toHaveBeenCalled();
    expect(daemon.listFacts).not.toHaveBeenCalled();
    expect(hook.result.current.loaded).toBe(false);
  });
});
