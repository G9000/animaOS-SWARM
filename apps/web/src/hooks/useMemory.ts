import { useCallback, useEffect, useRef, useState } from 'react';
import type {
  AgentRelationship,
  Memory,
  MemoryEntity,
  MemoryEvidenceTrace,
  MemoryFact,
  MemoryPatch,
} from '@animaOS-SWARM/sdk';
import { MAX_FACTS_SHOWN } from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';
import {
  ENTITY_GONE,
  FACT_GONE,
  MEMORY_GONE,
  MEMORY_PAGE_LIMIT,
  memoryErrorMessage,
  type MemoryEntry,
} from '../lib/memory';

export interface MemoryOptions {
  /** The companion whose memory is shown; null until it is known. */
  agentId: string | null;
  /** `LiveState.epoch`: bumped by every snapshot and resync. */
  epoch: number;
  /** False while the daemon is offline: nothing is read. */
  enabled: boolean;
}

export interface MemoryView {
  memories: MemoryEntry[];
  facts: MemoryFact[];
  entities: MemoryEntity[];
  relationships: AgentRelationship[];
  loaded: boolean;
  error: string | null;
  /** The HTTP status of the failed action, when the daemon refused it. */
  errorStatus: number | null;
  query: string;
  includeReplaced: boolean;
  /** Reads the list again with this search ('' is the recent list). */
  search: (query: string) => void;
  setIncludeReplaced: (value: boolean) => void;
  refresh: () => void;
  /** Each answers true when the daemon took it, after the lists were read
   *  again. */
  edit: (memory: Memory, patch: MemoryPatch) => Promise<boolean>;
  remove: (memory: Memory) => Promise<boolean>;
  trace: (memory: Memory) => Promise<MemoryEvidenceTrace | null>;
  replaceFact: (fact: MemoryFact, value: string) => Promise<boolean>;
  removeFact: (fact: MemoryFact) => Promise<boolean>;
  removeEntity: (entity: MemoryEntity) => Promise<boolean>;
}

/** An empty list that is empty again keeps its array, so a harness that goes
 *  online with nothing remembered does not render again (the M5 I4 lesson). */
function keepEmpty<T>(previous: T[], next: T[]): T[] {
  return previous.length === 0 && next.length === 0 ? previous : next;
}

/** The Memory page's data (spec §15.4, §15.5 `useMemory`). The daemon sends
 *  no memory event: the page reads on open, on resync, after its own actions,
 *  and on Refresh. */
export function useMemory({
  agentId,
  epoch,
  enabled,
}: MemoryOptions): MemoryView {
  const [memories, setMemories] = useState<MemoryEntry[]>([]);
  const [facts, setFacts] = useState<MemoryFact[]>([]);
  const [entities, setEntities] = useState<MemoryEntity[]>([]);
  const [relationships, setRelationships] = useState<AgentRelationship[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [query, setQueryState] = useState('');
  const [includeReplaced, setIncludeReplacedState] = useState(false);
  // A failed read and a failed action keep their own messages, so a
  // successful reload does not hide a refused action, nor the reverse.
  const [readError, setReadError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [actionStatus, setActionStatus] = useState<number | null>(null);

  // The latest inputs, so the actions' reload sees what the page shows now.
  const agentIdRef = useRef(agentId);
  agentIdRef.current = agentId;
  const queryRef = useRef('');
  const includeReplacedRef = useRef(false);
  // A read answered after a newer one started (or after unmount) is ignored.
  const sequence = useRef(0);
  const mounted = useRef(true);

  const load = useCallback(async () => {
    const id = agentIdRef.current;
    if (!id) return;
    const current = ++sequence.current;
    const text = queryRef.current;
    const results = await Promise.allSettled([
      text
        ? daemon.searchMemories(text, id, MEMORY_PAGE_LIMIT)
        : daemon.recentMemories(id, MEMORY_PAGE_LIMIT),
      // Facts belong to the owner, not to one agent: a fact whose evidence
      // memories were deleted, or that only a helper's memories evidence,
      // is still in use.
      daemon.listFacts({
        includeInactive: includeReplacedRef.current,
        limit: MAX_FACTS_SHOWN,
      }),
      daemon.listMemoryEntities(MEMORY_PAGE_LIMIT),
      daemon.listMemoryRelationships(id, MEMORY_PAGE_LIMIT),
    ]);
    if (current !== sequence.current) return;
    const [memoriesResult, factsResult, entitiesResult, relationshipsResult] =
      results;
    if (memoriesResult.status === 'fulfilled') {
      const next = memoriesResult.value;
      setMemories((previous) => keepEmpty(previous, next));
    }
    if (factsResult.status === 'fulfilled') {
      const next = factsResult.value;
      setFacts((previous) => keepEmpty(previous, next));
    }
    if (entitiesResult.status === 'fulfilled') {
      const next = entitiesResult.value;
      setEntities((previous) => keepEmpty(previous, next));
    }
    if (relationshipsResult.status === 'fulfilled') {
      const next = relationshipsResult.value;
      setRelationships((previous) => keepEmpty(previous, next));
    }
    const failed = results.find((result) => result.status === 'rejected');
    setReadError(
      failed
        ? memoryErrorMessage((failed as PromiseRejectedResult).reason).message
        : null,
    );
    setLoaded(true);
  }, []);

  useEffect(() => {
    if (!enabled || !agentId) return;
    void load();
  }, [enabled, agentId, epoch, query, includeReplaced, load]);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      sequence.current += 1;
    };
  }, []);

  // A new read starts with a clean header: a refused action's message goes
  // when the owner reads again, on any tab.
  const clearActionError = useCallback(() => {
    setActionError(null);
    setActionStatus(null);
  }, []);

  const search = useCallback(
    (next: string) => {
      clearActionError();
      queryRef.current = next;
      setQueryState(next);
    },
    [clearActionError],
  );

  const setIncludeReplaced = useCallback(
    (value: boolean) => {
      clearActionError();
      includeReplacedRef.current = value;
      setIncludeReplacedState(value);
    },
    [clearActionError],
  );

  const refresh = useCallback(() => {
    clearActionError();
    void load();
  }, [clearActionError, load]);

  // The page does not depend on any stream event, so an action reads again
  // before it answers.
  const act = useCallback(
    async (work: () => Promise<unknown>, gone: string) => {
      setActionError(null);
      setActionStatus(null);
      try {
        await work();
      } catch (caught) {
        const { message, status } = memoryErrorMessage(caught);
        if (mounted.current) {
          setActionError(status === 404 ? gone : message);
          setActionStatus(status);
        }
        // The thing acted on changed or went away since the lists were
        // read: read them again so the page shows what is there now.
        if (status === 404 || status === 409) await load();
        return false;
      }
      await load();
      return true;
    },
    [load],
  );

  const edit = useCallback(
    (memory: Memory, patch: MemoryPatch) =>
      act(() => daemon.updateMemory(memory.id, patch), MEMORY_GONE),
    [act],
  );
  const remove = useCallback(
    (memory: Memory) => act(() => daemon.deleteMemory(memory.id), MEMORY_GONE),
    [act],
  );
  const replaceFact = useCallback(
    (fact: MemoryFact, value: string) =>
      act(() => daemon.replaceFact(fact.id, value), FACT_GONE),
    [act],
  );
  const removeFact = useCallback(
    (fact: MemoryFact) => act(() => daemon.deleteFact(fact.id), FACT_GONE),
    [act],
  );
  const removeEntity = useCallback(
    (entity: MemoryEntity) =>
      act(() => daemon.deleteMemoryEntity(entity.kind, entity.id), ENTITY_GONE),
    [act],
  );

  // Reading evidence changes nothing, so it does not read the lists again.
  const trace = useCallback(
    async (memory: Memory): Promise<MemoryEvidenceTrace | null> => {
      setActionError(null);
      setActionStatus(null);
      try {
        return await daemon.traceMemory(memory.id);
      } catch (caught) {
        const { message, status } = memoryErrorMessage(caught);
        if (mounted.current) {
          setActionError(status === 404 ? MEMORY_GONE : message);
          setActionStatus(status);
        }
        // The memory is gone: read the lists again so the page drops it.
        if (status === 404) await load();
        return null;
      }
    },
    [load],
  );

  return {
    memories,
    facts,
    entities,
    relationships,
    loaded,
    error: actionError ?? readError,
    errorStatus: actionError ? actionStatus : null,
    query,
    includeReplaced,
    search,
    setIncludeReplaced,
    refresh,
    edit,
    remove,
    trace,
    replaceFact,
    removeFact,
    removeEntity,
  };
}
