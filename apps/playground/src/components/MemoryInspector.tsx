import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  memories,
  type AgentRelationship,
  type AgentSnapshot,
  type EvaluatedMemoryInput,
  type Memory,
  type MemoryScope,
  type MemoryEvidenceTrace,
  type MemoryEntity,
  type MemoryEvaluation,
  type MemoryEvaluationOutcome,
  type MemoryRecallResult,
  type MemoryReadiness,
  type MemoryType,
  type RelationshipEndpointKind,
} from '../lib/api';
import { playgroundUserId } from '../lib/playgroundUser';

type InspectorTab = 'state' | 'graph' | 'recall' | 'evaluate' | 'ready';

const inspectorTabs: Array<{ id: InspectorTab; label: string }> = [
  { id: 'state', label: 'state' },
  { id: 'graph', label: 'graph' },
  { id: 'recall', label: 'recall' },
  { id: 'evaluate', label: 'save' },
  { id: 'ready', label: 'ready' },
];

type EntityDraft = {
  kind: RelationshipEndpointKind;
  id: string;
  name: string;
  aliases: string;
  summary: string;
};

type RelationshipDraft = {
  sourceKind: RelationshipEndpointKind;
  sourceAgentId: string;
  sourceAgentName: string;
  targetKind: RelationshipEndpointKind;
  targetAgentId: string;
  targetAgentName: string;
  relationshipType: string;
  summary: string;
  strength: number;
  confidence: number;
  evidenceMemoryIds: string;
  tags: string;
};

type GraphFilter = 'all' | 'swarm' | 'handoffs' | 'broadcasts' | 'users';

const graphFilters: Array<{ id: GraphFilter; label: string }> = [
  { id: 'all', label: 'all' },
  { id: 'swarm', label: 'swarm' },
  { id: 'handoffs', label: 'handoffs' },
  { id: 'broadcasts', label: 'broadcasts' },
  { id: 'users', label: 'users' },
];

interface Props {
  agents: AgentSnapshot[];
  selectedAgentId: string | null;
  refreshKey: number;
}

export function MemoryInspector({ agents, selectedAgentId, refreshKey }: Props) {
  const [tab, setTab] = useState<InspectorTab>('state');
  const [agentId, setAgentId] = useState(selectedAgentId ?? '');
  const [entityId, setEntityId] = useState('');
  const [recent, setRecent] = useState<Memory[]>([]);
  const [entities, setEntities] = useState<MemoryEntity[]>([]);
  const [relationships, setRelationships] = useState<AgentRelationship[]>([]);
  const [trace, setTrace] = useState<MemoryEvidenceTrace | null>(null);
  const [readiness, setReadiness] = useState<MemoryReadiness | null>(null);
  const [recallQuery, setRecallQuery] = useState('relationship evidence probe');
  const [recallResults, setRecallResults] = useState<MemoryRecallResult[]>([]);
  const [draftContent, setDraftContent] = useState('');
  const [draftType, setDraftType] = useState<MemoryType>('fact');
  const [draftScope, setDraftScope] = useState<MemoryScope>('private');
  const [draftTags, setDraftTags] = useState('playground-inspector');
  const [importance, setImportance] = useState(0.6);
  const [evaluation, setEvaluation] = useState<MemoryEvaluation | null>(null);
  const [outcome, setOutcome] = useState<MemoryEvaluationOutcome | null>(null);
  const [savedMemory, setSavedMemory] = useState<Memory | null>(null);
  const [entityDraft, setEntityDraft] = useState<EntityDraft>(() => ({
    kind: 'user',
    id: playgroundUserId(),
    name: 'Playground User',
    aliases: 'operator,me',
    summary: '',
  }));
  const [relationshipDraft, setRelationshipDraft] = useState<RelationshipDraft>(() => ({
    sourceKind: 'agent',
    sourceAgentId: '',
    sourceAgentName: '',
    targetKind: 'user',
    targetAgentId: playgroundUserId(),
    targetAgentName: 'Playground User',
    relationshipType: 'knows',
    summary: '',
    strength: 0.6,
    confidence: 0.7,
    evidenceMemoryIds: '',
    tags: 'playground-inspector',
  }));
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const knownRelationshipIds = useRef<Set<string>>(new Set());

  const selectedAgent = useMemo(
    () => agents.find((agent) => agent.state.id === agentId) ?? null,
    [agentId, agents]
  );
  const selectedAgentStateId = selectedAgent?.state.id;
  const selectedAgentStateName = selectedAgent?.state.name;

  useEffect(() => {
    if (selectedAgentId) {
      setAgentId(selectedAgentId);
    }
  }, [selectedAgentId]);

  useEffect(() => {
    setEntityId((current) => current || playgroundUserId());
  }, []);

  useEffect(() => {
    if (!selectedAgentStateId || !selectedAgentStateName) return;
    setRelationshipDraft((current) => ({
      ...current,
      sourceKind: 'agent',
      sourceAgentId: selectedAgentStateId,
      sourceAgentName: selectedAgentStateName,
    }));
  }, [selectedAgentStateId, selectedAgentStateName]);

  const loadState = useCallback(async () => {
    setLoading(true);
    try {
      const trimmedEntityId = entityId.trim();
      const [recentMemories, entityList, relationshipList, readinessReport] = await Promise.all([
        memories.recent({
          agentId: agentId || undefined,
          limit: 8,
        }),
        memories.entities({ limit: 16 }),
        memories.relationships({
          agentId: agentId || undefined,
          entityId: trimmedEntityId || undefined,
          limit: 24,
        }),
        memories.readiness(),
      ]);
      setRecent(recentMemories);
      setEntities(entityList);
      setRelationships(relationshipList);
      setReadiness(readinessReport);
      const nextRelationshipIds = new Set(relationshipList.map((relationship) => relationship.id));
      const hasNewRelationship = relationshipList.some(
        (relationship) => !knownRelationshipIds.current.has(relationship.id)
      );
      knownRelationshipIds.current = nextRelationshipIds;
      if (refreshKey > 0 && hasNewRelationship) {
        setTab('graph');
      }
      setError(null);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setLoading(false);
    }
  }, [agentId, entityId, refreshKey]);

  useEffect(() => {
    loadState();
  }, [loadState, refreshKey]);

  async function runRecall() {
    const query = recallQuery.trim();
    if (!query) return;

    setLoading(true);
    try {
      const results = await memories.recall(query, {
        agentId: agentId || undefined,
        entityId: entityId.trim() || undefined,
        recentLimit: 0,
        limit: 5,
      });
      setRecallResults(results);
      setError(null);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setLoading(false);
    }
  }

  async function loadTrace(memoryId: string) {
    setLoading(true);
    try {
      setTrace(await memories.trace(memoryId));
      setError(null);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setLoading(false);
    }
  }

  async function evaluateDraft(store: boolean) {
    if (!selectedAgent || !draftContent.trim()) return;

    const input: EvaluatedMemoryInput = {
      agentId: selectedAgent.state.id,
      agentName: selectedAgent.state.name,
      type: draftType,
      content: draftContent.trim(),
      importance,
      tags: ['playground-inspector'],
      minContentChars: 8,
    };

    setLoading(true);
    try {
      setSavedMemory(null);
      if (store) {
        const nextOutcome = await memories.addEvaluated(input);
        setOutcome(nextOutcome);
        setEvaluation(nextOutcome.evaluation);
        await loadState();
      } else {
        setEvaluation(await memories.evaluate(input));
        setOutcome(null);
      }
      setError(null);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setLoading(false);
    }
  }

  async function saveDraftDirect() {
    if (!selectedAgent || !draftContent.trim()) return;

    setLoading(true);
    try {
      const memory = await memories.create({
        agentId: selectedAgent.state.id,
        agentName: selectedAgent.state.name,
        type: draftType,
        content: draftContent.trim(),
        importance,
        tags: parseTagList(draftTags),
        scope: draftScope,
      });
      setSavedMemory(memory);
      setEvaluation(null);
      setOutcome(null);
      await loadState();
      setError(null);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setLoading(false);
    }
  }

  async function createEntity() {
    if (!entityDraft.id.trim() || !entityDraft.name.trim()) return;

    setLoading(true);
    try {
      const entity = await memories.createEntity({
        kind: entityDraft.kind,
        id: entityDraft.id.trim(),
        name: entityDraft.name.trim(),
        aliases: parseTagList(entityDraft.aliases),
        summary: trimOptional(entityDraft.summary),
      });
      setEntities((items) => upsertEntity(items, entity));
      setEntityId(entity.id);
      await loadState();
      setError(null);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setLoading(false);
    }
  }

  async function createRelationship() {
    const sourceAgentId = relationshipDraft.sourceAgentId.trim();
    const sourceAgentName = relationshipDraft.sourceAgentName.trim();
    const targetAgentId = relationshipDraft.targetAgentId.trim();
    const targetAgentName = relationshipDraft.targetAgentName.trim();
    const relationshipType = relationshipDraft.relationshipType.trim();
    if (!sourceAgentId || !sourceAgentName || !targetAgentId || !targetAgentName || !relationshipType) {
      return;
    }

    setLoading(true);
    try {
      const relationship = await memories.createRelationship({
        sourceKind: relationshipDraft.sourceKind,
        sourceAgentId,
        sourceAgentName,
        targetKind: relationshipDraft.targetKind,
        targetAgentId,
        targetAgentName,
        relationshipType,
        summary: trimOptional(relationshipDraft.summary),
        strength: relationshipDraft.strength,
        confidence: relationshipDraft.confidence,
        evidenceMemoryIds: parseTagList(relationshipDraft.evidenceMemoryIds),
        tags: parseTagList(relationshipDraft.tags),
      });
      setRelationships((items) => upsertRelationship(items, relationship));
      await loadState();
      setError(null);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setLoading(false);
    }
  }

  return (
    <aside className="hidden xl:flex min-h-0 flex-col border-l border-[var(--border)] bg-[var(--surface)]">
      <div className="border-b border-[var(--border)] px-4 py-3">
        <div className="flex items-center justify-between gap-3">
          <div>
            <h2 className="m-0 text-sm font-semibold text-[var(--text)]">
              Memory
            </h2>
            <div className="mt-0.5 text-[11px] text-[var(--muted-2)]">
              {loading ? 'syncing' : 'live daemon state'}
            </div>
          </div>
          <button
            onClick={loadState}
            className="px-2.5 py-1.5 text-xs rounded-md border border-[var(--border)] text-[var(--text-2)] hover:bg-[var(--hover)]"
          >
            Refresh
          </button>
        </div>

        <div className="mt-3 flex rounded-md border border-[var(--border)] bg-[var(--surface-2)] p-1">
          {inspectorTabs.map(({ id, label }) => (
            <button
              key={id}
              onClick={() => setTab(id)}
              className={`flex-1 rounded px-2 py-1 text-xs capitalize ${
                tab === id
                  ? 'bg-[var(--surface-3)] text-[var(--text)]'
                  : 'text-[var(--muted)] hover:text-[var(--text-2)]'
              }`}
            >
              {label}
            </button>
          ))}
        </div>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto px-4 py-4">
        <Filters
          agents={agents}
          agentId={agentId}
          entityId={entityId}
          onAgentIdChange={setAgentId}
          onEntityIdChange={setEntityId}
        />

        {error && (
          <div className="mt-3 rounded-md border border-[var(--err)]/30 bg-[var(--err)]/10 px-3 py-2 text-xs text-[var(--err)]">
            {error}
          </div>
        )}

        {tab === 'state' && (
          <StateView
            recent={recent}
            entities={entities}
            relationships={relationships}
            onTrace={loadTrace}
          />
        )}

        {tab === 'graph' && (
          <GraphView
            entities={entities}
            relationships={relationships}
            onTrace={loadTrace}
            entityDraft={entityDraft}
            relationshipDraft={relationshipDraft}
            loading={loading}
            onEntityDraftChange={(patch) =>
              setEntityDraft((current) => ({ ...current, ...patch }))
            }
            onRelationshipDraftChange={(patch) =>
              setRelationshipDraft((current) => ({ ...current, ...patch }))
            }
            onCreateEntity={createEntity}
            onCreateRelationship={createRelationship}
          />
        )}

        {tab === 'recall' && (
          <RecallView
            query={recallQuery}
            results={recallResults}
            onQueryChange={setRecallQuery}
            onRun={runRecall}
            onTrace={loadTrace}
          />
        )}

        {tab === 'evaluate' && (
          <EvaluateView
            selectedAgent={selectedAgent}
            content={draftContent}
            memoryType={draftType}
            memoryScope={draftScope}
            tags={draftTags}
            importance={importance}
            evaluation={evaluation}
            outcome={outcome}
            savedMemory={savedMemory}
            loading={loading}
            onContentChange={setDraftContent}
            onMemoryTypeChange={setDraftType}
            onMemoryScopeChange={setDraftScope}
            onTagsChange={setDraftTags}
            onImportanceChange={setImportance}
            onEvaluate={() => evaluateDraft(false)}
            onStore={() => evaluateDraft(true)}
            onSaveDirect={saveDraftDirect}
          />
        )}

        {tab === 'ready' && (
          <ReadinessView readiness={readiness} onRefresh={loadState} />
        )}

        {trace && <TraceView trace={trace} onClose={() => setTrace(null)} />}
      </div>
    </aside>
  );
}

function ReadinessView({
  readiness,
  onRefresh,
}: {
  readiness: MemoryReadiness | null;
  onRefresh: () => void;
}) {
  if (!readiness) {
    return (
      <div className="mt-4 grid gap-3">
        <EmptyLine text="No readiness report." />
        <button
          onClick={onRefresh}
          className="px-3 py-2 text-sm rounded-md border border-[var(--border)] text-[var(--text-2)] hover:bg-[var(--hover)]"
        >
          Refresh
        </button>
      </div>
    );
  }

  const failedChecks = readiness.evaluation.cases.flatMap((evalCase) =>
    evalCase.checks.filter((check) => !check.passed)
  );

  return (
    <div className="mt-4 grid gap-3">
      <div className="rounded-md border border-[var(--border)] bg-[var(--surface-2)] px-3 py-2">
        <div className="flex items-center justify-between gap-2">
          <span className="text-sm font-medium text-[var(--text)]">Memory readiness</span>
          <span
            className={`rounded px-2 py-0.5 text-[11px] font-medium ${
              readiness.passed
                ? 'bg-[var(--ok)]/15 text-[var(--ok)]'
                : 'bg-[var(--err)]/15 text-[var(--err)]'
            }`}
          >
            {readiness.passed ? 'passing' : 'failing'}
          </span>
        </div>
        <div className="mt-2 grid grid-cols-2 gap-2 text-[11px] text-[var(--muted-2)]">
          <Metric label="checks" value={`${readiness.evaluation.passedChecks}/${readiness.evaluation.totalChecks}`} />
          <Metric label="vectors" value={String(readiness.embeddings.vectorCount)} />
          <Metric label="provider" value={readiness.embeddings.provider} />
          <Metric label="persisted" value={readiness.embeddings.persisted ? 'yes' : 'no'} />
        </div>
      </div>

      <div className="rounded-md border border-[var(--border)] bg-[var(--surface-2)] px-3 py-2 text-xs">
        <div className="font-medium text-[var(--text)]">Embedding index</div>
        <div className="mt-2 grid gap-1 text-[var(--muted)]">
          <div className="truncate">{readiness.embeddings.model}</div>
          <div className="tabular-nums">{readiness.embeddings.dimension} dimensions</div>
          {readiness.embeddings.storageFile && (
            <div className="truncate font-mono text-[11px] text-[var(--muted-2)]">
              {readiness.embeddings.storageFile}
            </div>
          )}
        </div>
      </div>

      <SectionTitle title="Eval Cases" count={readiness.evaluation.cases.length} />
      <div className="grid gap-2">
        {readiness.evaluation.cases.map((evalCase) => (
          <div
            key={evalCase.name}
            className="rounded-md border border-[var(--border)] bg-[var(--surface-2)] px-3 py-2 text-xs"
          >
            <div className="flex items-center justify-between gap-2">
              <span className="truncate font-medium text-[var(--text)]">{evalCase.name}</span>
              <span className="tabular-nums text-[var(--muted-2)]">
                {evalCase.checks.filter((check) => check.passed).length}/{evalCase.checks.length}
              </span>
            </div>
          </div>
        ))}
      </div>

      {failedChecks.length > 0 && (
        <div className="grid gap-2">
          <SectionTitle title="Failures" count={failedChecks.length} />
          {failedChecks.map((check) => (
            <div
              key={`${check.name}:${check.detail}`}
              className="rounded-md border border-[var(--err)]/30 bg-[var(--err)]/10 px-3 py-2 text-xs text-[var(--err)]"
            >
              <div className="font-medium">{check.name}</div>
              <div className="mt-1">{check.detail}</div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function Metric({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded border border-[var(--border)] bg-[var(--surface)] px-2 py-1">
      <div>{label}</div>
      <div className="truncate tabular-nums text-[var(--text-2)]">{value}</div>
    </div>
  );
}

function Filters({
  agents,
  agentId,
  entityId,
  onAgentIdChange,
  onEntityIdChange,
}: {
  agents: AgentSnapshot[];
  agentId: string;
  entityId: string;
  onAgentIdChange: (value: string) => void;
  onEntityIdChange: (value: string) => void;
}) {
  return (
    <div className="grid gap-3">
      <label className="grid gap-1.5 text-xs text-[var(--muted)]">
        Agent
        <select value={agentId} onChange={(event) => onAgentIdChange(event.target.value)}>
          <option value="">All agents</option>
          {agents.map((agent) => (
            <option key={agent.state.id} value={agent.state.id}>
              {agent.state.name}
            </option>
          ))}
        </select>
      </label>
      <label className="grid gap-1.5 text-xs text-[var(--muted)]">
        Entity
        <input
          value={entityId}
          onChange={(event) => onEntityIdChange(event.target.value)}
          placeholder="agent, user, system, external ID"
        />
      </label>
    </div>
  );
}

function StateView({
  recent,
  entities,
  relationships,
  onTrace,
}: {
  recent: Memory[];
  entities: MemoryEntity[];
  relationships: AgentRelationship[];
  onTrace: (memoryId: string) => void;
}) {
  return (
    <div className="mt-4 grid gap-4">
      <SectionTitle title="Recent" count={recent.length} />
      <div className="grid gap-2">
        {recent.map((memory) => (
          <MemoryRow key={memory.id} memory={memory} onTrace={onTrace} />
        ))}
        {recent.length === 0 && <EmptyLine text="No memories." />}
      </div>

      <SectionTitle title="Relationships" count={relationships.length} />
      <div className="grid gap-2">
        {relationships.map((relationship) => (
          <RelationshipRow
            key={relationship.id}
            relationship={relationship}
            onTrace={onTrace}
          />
        ))}
        {relationships.length === 0 && <EmptyLine text="No relationships." />}
      </div>

      <SectionTitle title="Entities" count={entities.length} />
      <div className="grid gap-2">
        {entities.map((entity) => (
          <div
            key={`${entity.kind}:${entity.id}`}
            className="rounded-md border border-[var(--border)] bg-[var(--surface-2)] px-3 py-2"
          >
            <div className="flex items-center justify-between gap-2">
              <span className="truncate text-sm font-medium text-[var(--text)]">
                {entity.name}
              </span>
              <span className="shrink-0 text-[11px] text-[var(--muted)]">
                {entity.kind}
              </span>
            </div>
            <div className="mt-1 truncate font-mono text-[11px] text-[var(--muted-2)]">
              {entity.id}
            </div>
          </div>
        ))}
        {entities.length === 0 && <EmptyLine text="No entities." />}
      </div>
    </div>
  );
}

function GraphView({
  entities,
  relationships,
  onTrace,
  entityDraft,
  relationshipDraft,
  loading,
  onEntityDraftChange,
  onRelationshipDraftChange,
  onCreateEntity,
  onCreateRelationship,
}: {
  entities: MemoryEntity[];
  relationships: AgentRelationship[];
  onTrace: (memoryId: string) => void;
  entityDraft: EntityDraft;
  relationshipDraft: RelationshipDraft;
  loading: boolean;
  onEntityDraftChange: (patch: Partial<EntityDraft>) => void;
  onRelationshipDraftChange: (patch: Partial<RelationshipDraft>) => void;
  onCreateEntity: () => void;
  onCreateRelationship: () => void;
}) {
  const [filter, setFilter] = useState<GraphFilter>('all');
  const [traceableOnly, setTraceableOnly] = useState(false);
  const graphCounts = useMemo(
    () => relationshipGraphCounts(relationships),
    [relationships]
  );
  const filteredRelationships = useMemo(
    () =>
      relationships.filter(
        (relationship) =>
          relationshipMatchesGraphFilter(relationship, filter) &&
          (!traceableOnly || relationship.evidenceMemoryIds.length > 0)
      ),
    [filter, relationships, traceableOnly]
  );
  const graph = useMemo(
    () => buildRelationshipGraph(entities, filteredRelationships),
    [entities, filteredRelationships]
  );
  const hiddenRelationshipCount = relationships.length - filteredRelationships.length;

  return (
    <div className="mt-4 grid gap-4">
      <div className="rounded-md border border-[var(--border)] bg-[var(--surface-2)] px-3 py-3">
        <div className="flex items-center justify-between gap-2">
          <div>
            <div className="text-sm font-medium text-[var(--text)]">Relationship graph</div>
            <div className="text-[11px] text-[var(--muted-2)]">
              {graph.nodes.length} nodes · {graph.edges.length} edges
              {hiddenRelationshipCount > 0 ? ` · ${hiddenRelationshipCount} hidden` : ''}
            </div>
          </div>
        </div>

        <GraphFilterControls
          filter={filter}
          counts={graphCounts}
          traceableOnly={traceableOnly}
          onFilterChange={setFilter}
          onTraceableOnlyChange={setTraceableOnly}
        />

        {graph.nodes.length === 0 ? (
          <div className="mt-3">
            <EmptyLine
              text={relationships.length === 0 ? 'No graph data yet.' : 'No matching relationships.'}
            />
          </div>
        ) : (
          <>
            <svg
              viewBox="0 0 640 320"
              className="mt-3 h-72 w-full rounded-md border border-[var(--border)] bg-[var(--surface)]"
            >
              <defs>
                <marker
                  id="memory-graph-arrow"
                  markerWidth="8"
                  markerHeight="8"
                  refX="7"
                  refY="4"
                  orient="auto"
                >
                  <path d="M0,0 L8,4 L0,8 Z" fill="currentColor" />
                </marker>
              </defs>

              {graph.edges.map((edge) => {
                const source = graph.nodeMap.get(edge.sourceKey);
                const target = graph.nodeMap.get(edge.targetKey);
                if (!source || !target) return null;
                const midX = (source.x + target.x) / 2;
                const midY = (source.y + target.y) / 2;
                const evidenceMemoryId = edge.evidenceMemoryIds[0];

                return (
                  <g
                    key={edge.id}
                    className={evidenceMemoryId ? 'cursor-pointer' : ''}
                    style={{ color: graphEdgeColor(edge.tone) }}
                    role={evidenceMemoryId ? 'button' : undefined}
                    tabIndex={evidenceMemoryId ? 0 : undefined}
                    onClick={() => {
                      if (evidenceMemoryId) onTrace(evidenceMemoryId);
                    }}
                    onKeyDown={(event) => {
                      if (!evidenceMemoryId) return;
                      if (event.key === 'Enter' || event.key === ' ') {
                        event.preventDefault();
                        onTrace(evidenceMemoryId);
                      }
                    }}
                  >
                    <title>{graphEdgeTitle(edge)}</title>
                    <line
                      x1={source.x}
                      y1={source.y}
                      x2={target.x}
                      y2={target.y}
                      stroke="transparent"
                      strokeWidth="16"
                    />
                    <line
                      x1={source.x}
                      y1={source.y}
                      x2={target.x}
                      y2={target.y}
                      stroke="currentColor"
                      strokeWidth={1 + edge.strength * 3}
                      strokeDasharray={edge.tone === 'broadcast' ? '5 4' : undefined}
                      strokeOpacity={edge.tone === 'default' ? '0.45' : '0.74'}
                      markerEnd="url(#memory-graph-arrow)"
                    />
                    <rect
                      x={midX - 38}
                      y={midY - 10}
                      width="76"
                      height="20"
                      rx="10"
                      fill="var(--surface)"
                      opacity="0.92"
                    />
                    <text
                      x={midX}
                      y={midY + 4}
                      textAnchor="middle"
                      fontSize="10"
                      fill="var(--text-2)"
                    >
                      {truncateGraphLabel(edge.label, 14)}
                    </text>
                    {edge.tags.length > 0 && (
                      <text
                        x={midX}
                        y={midY + 18}
                        textAnchor="middle"
                        fontSize="9"
                        fill="var(--muted-2)"
                      >
                        {truncateGraphLabel(preferredTagLabel(edge.tags), 18)}
                      </text>
                    )}
                  </g>
                );
              })}

              {graph.nodes.map((node) => (
                <g key={node.key} transform={`translate(${node.x}, ${node.y})`}>
                  <circle
                    r="22"
                    fill={graphNodeColor(node.kind)}
                    fillOpacity="0.16"
                    stroke={graphNodeColor(node.kind)}
                    strokeWidth="2"
                  />
                  <text
                    y="4"
                    textAnchor="middle"
                    fontSize="10"
                    fontWeight="600"
                    fill="var(--text)"
                  >
                    {truncateGraphLabel(node.name, 12)}
                  </text>
                  <text
                    y="36"
                    textAnchor="middle"
                    fontSize="10"
                    fill="var(--muted-2)"
                  >
                    {node.kind}
                  </text>
                </g>
              ))}
            </svg>

            <div className="mt-3 grid gap-2">
              {filteredRelationships.map((relationship) => (
                <RelationshipRow
                  key={relationship.id}
                  relationship={relationship}
                  onTrace={onTrace}
                />
              ))}
              {filteredRelationships.length === 0 && (
                <EmptyLine text="No matching relationships." />
              )}
            </div>
          </>
        )}
      </div>

      <GraphCreateForms
        entityDraft={entityDraft}
        relationshipDraft={relationshipDraft}
        loading={loading}
        onEntityDraftChange={onEntityDraftChange}
        onRelationshipDraftChange={onRelationshipDraftChange}
        onCreateEntity={onCreateEntity}
        onCreateRelationship={onCreateRelationship}
      />
    </div>
  );
}

function GraphFilterControls({
  filter,
  counts,
  traceableOnly,
  onFilterChange,
  onTraceableOnlyChange,
}: {
  filter: GraphFilter;
  counts: GraphCounts;
  traceableOnly: boolean;
  onFilterChange: (value: GraphFilter) => void;
  onTraceableOnlyChange: (value: boolean) => void;
}) {
  return (
    <div className="mt-3 grid gap-3">
      <div className="grid grid-cols-5 gap-1 rounded-md border border-[var(--border)] bg-[var(--surface)] p-1">
        {graphFilters.map(({ id, label }) => (
          <button
            key={id}
            onClick={() => onFilterChange(id)}
            className={`rounded px-1.5 py-1 text-[11px] capitalize ${
              filter === id
                ? 'bg-[var(--surface-3)] text-[var(--text)]'
                : 'text-[var(--muted)] hover:text-[var(--text-2)]'
            }`}
          >
            <span>{label}</span>
            <span className="ml-1 tabular-nums text-[var(--muted-2)]">
              {graphFilterCount(counts, id)}
            </span>
          </button>
        ))}
      </div>

      <div className="grid grid-cols-3 gap-2 text-[11px] text-[var(--muted-2)]">
        <GraphCountBadge label="handoff" value={counts.handoffs} tone="handoff" />
        <GraphCountBadge label="broadcast" value={counts.broadcasts} tone="broadcast" />
        <GraphCountBadge label="evidence" value={counts.traceable} tone="default" />
      </div>

      <label className="flex items-center justify-between gap-3 rounded-md border border-[var(--border)] bg-[var(--surface)] px-3 py-2 text-xs text-[var(--muted)]">
        <span>Evidence only</span>
        <input
          type="checkbox"
          checked={traceableOnly}
          onChange={(event) => onTraceableOnlyChange(event.target.checked)}
        />
      </label>
    </div>
  );
}

function GraphCountBadge({
  label,
  value,
  tone,
}: {
  label: string;
  value: number;
  tone: GraphRelationshipTone;
}) {
  return (
    <div
      className="rounded-md border bg-[var(--surface)] px-2 py-1"
      style={{ borderColor: graphEdgeColor(tone) }}
    >
      <div className="truncate">{label}</div>
      <div className="tabular-nums text-[var(--text-2)]">{value}</div>
    </div>
  );
}

function GraphCreateForms({
  entityDraft,
  relationshipDraft,
  loading,
  onEntityDraftChange,
  onRelationshipDraftChange,
  onCreateEntity,
  onCreateRelationship,
}: {
  entityDraft: EntityDraft;
  relationshipDraft: RelationshipDraft;
  loading: boolean;
  onEntityDraftChange: (patch: Partial<EntityDraft>) => void;
  onRelationshipDraftChange: (patch: Partial<RelationshipDraft>) => void;
  onCreateEntity: () => void;
  onCreateRelationship: () => void;
}) {
  const entityDisabled =
    loading || !entityDraft.id.trim() || !entityDraft.name.trim();
  const relationshipDisabled =
    loading ||
    !relationshipDraft.sourceAgentId.trim() ||
    !relationshipDraft.sourceAgentName.trim() ||
    !relationshipDraft.targetAgentId.trim() ||
    !relationshipDraft.targetAgentName.trim() ||
    !relationshipDraft.relationshipType.trim();

  return (
    <div className="grid gap-4">
      <div className="rounded-md border border-[var(--border)] bg-[var(--surface-2)] px-3 py-3">
        <div className="text-sm font-medium text-[var(--text)]">Create entity</div>
        <div className="mt-3 grid gap-2">
          <EndpointKindSelect
            label="Kind"
            value={entityDraft.kind}
            onChange={(kind) => onEntityDraftChange({ kind })}
          />
          <label className="grid gap-1.5 text-xs text-[var(--muted)]">
            ID
            <input
              value={entityDraft.id}
              onChange={(event) => onEntityDraftChange({ id: event.target.value })}
              placeholder="stable entity id"
            />
          </label>
          <label className="grid gap-1.5 text-xs text-[var(--muted)]">
            Name
            <input
              value={entityDraft.name}
              onChange={(event) => onEntityDraftChange({ name: event.target.value })}
              placeholder="display name"
            />
          </label>
          <label className="grid gap-1.5 text-xs text-[var(--muted)]">
            Aliases
            <input
              value={entityDraft.aliases}
              onChange={(event) => onEntityDraftChange({ aliases: event.target.value })}
              placeholder="comma,separated,aliases"
            />
          </label>
          <label className="grid gap-1.5 text-xs text-[var(--muted)]">
            Summary
            <textarea
              value={entityDraft.summary}
              onChange={(event) => onEntityDraftChange({ summary: event.target.value })}
              rows={2}
              placeholder="optional entity note"
            />
          </label>
          <button
            onClick={onCreateEntity}
            disabled={entityDisabled}
            className="px-3 py-2 text-sm font-medium rounded-md bg-[var(--accent)] text-[var(--accent-fg)] hover:bg-[var(--accent-hover)] disabled:bg-[var(--surface-3)] disabled:text-[var(--muted-2)]"
          >
            Save entity
          </button>
        </div>
      </div>

      <div className="rounded-md border border-[var(--border)] bg-[var(--surface-2)] px-3 py-3">
        <div className="text-sm font-medium text-[var(--text)]">Create relationship</div>
        <div className="mt-3 grid gap-3">
          <div className="grid grid-cols-2 gap-2">
            <EndpointKindSelect
              label="Source kind"
              value={relationshipDraft.sourceKind}
              onChange={(sourceKind) => onRelationshipDraftChange({ sourceKind })}
            />
            <EndpointKindSelect
              label="Target kind"
              value={relationshipDraft.targetKind}
              onChange={(targetKind) => onRelationshipDraftChange({ targetKind })}
            />
          </div>
          <div className="grid grid-cols-2 gap-2">
            <label className="grid gap-1.5 text-xs text-[var(--muted)]">
              Source ID
              <input
                value={relationshipDraft.sourceAgentId}
                onChange={(event) => onRelationshipDraftChange({ sourceAgentId: event.target.value })}
                placeholder="source id"
              />
            </label>
            <label className="grid gap-1.5 text-xs text-[var(--muted)]">
              Source name
              <input
                value={relationshipDraft.sourceAgentName}
                onChange={(event) => onRelationshipDraftChange({ sourceAgentName: event.target.value })}
                placeholder="source name"
              />
            </label>
          </div>
          <div className="grid grid-cols-2 gap-2">
            <label className="grid gap-1.5 text-xs text-[var(--muted)]">
              Target ID
              <input
                value={relationshipDraft.targetAgentId}
                onChange={(event) => onRelationshipDraftChange({ targetAgentId: event.target.value })}
                placeholder="target id"
              />
            </label>
            <label className="grid gap-1.5 text-xs text-[var(--muted)]">
              Target name
              <input
                value={relationshipDraft.targetAgentName}
                onChange={(event) => onRelationshipDraftChange({ targetAgentName: event.target.value })}
                placeholder="target name"
              />
            </label>
          </div>
          <label className="grid gap-1.5 text-xs text-[var(--muted)]">
            Type
            <input
              value={relationshipDraft.relationshipType}
              onChange={(event) => onRelationshipDraftChange({ relationshipType: event.target.value })}
              placeholder="knows, trusts, collaborates_with"
            />
          </label>
          <label className="grid gap-1.5 text-xs text-[var(--muted)]">
            Summary
            <textarea
              value={relationshipDraft.summary}
              onChange={(event) => onRelationshipDraftChange({ summary: event.target.value })}
              rows={2}
              placeholder="optional relationship evidence"
            />
          </label>
          <div className="grid grid-cols-2 gap-2">
            <RangeInput
              label="Strength"
              value={relationshipDraft.strength}
              onChange={(strength) => onRelationshipDraftChange({ strength })}
            />
            <RangeInput
              label="Confidence"
              value={relationshipDraft.confidence}
              onChange={(confidence) => onRelationshipDraftChange({ confidence })}
            />
          </div>
          <label className="grid gap-1.5 text-xs text-[var(--muted)]">
            Evidence memory IDs
            <input
              value={relationshipDraft.evidenceMemoryIds}
              onChange={(event) => onRelationshipDraftChange({ evidenceMemoryIds: event.target.value })}
              placeholder="comma,separated,memoryIds"
            />
          </label>
          <label className="grid gap-1.5 text-xs text-[var(--muted)]">
            Tags
            <input
              value={relationshipDraft.tags}
              onChange={(event) => onRelationshipDraftChange({ tags: event.target.value })}
              placeholder="comma,separated,tags"
            />
          </label>
          <button
            onClick={onCreateRelationship}
            disabled={relationshipDisabled}
            className="px-3 py-2 text-sm font-medium rounded-md bg-[var(--accent)] text-[var(--accent-fg)] hover:bg-[var(--accent-hover)] disabled:bg-[var(--surface-3)] disabled:text-[var(--muted-2)]"
          >
            Save relationship
          </button>
        </div>
      </div>
    </div>
  );
}

function EndpointKindSelect({
  label,
  value,
  onChange,
}: {
  label: string;
  value: RelationshipEndpointKind;
  onChange: (value: RelationshipEndpointKind) => void;
}) {
  return (
    <label className="grid gap-1.5 text-xs text-[var(--muted)]">
      {label}
      <select
        value={value}
        onChange={(event) => onChange(event.target.value as RelationshipEndpointKind)}
      >
        <option value="agent">Agent</option>
        <option value="user">User</option>
        <option value="system">System</option>
        <option value="external">External</option>
      </select>
    </label>
  );
}

function RangeInput({
  label,
  value,
  onChange,
}: {
  label: string;
  value: number;
  onChange: (value: number) => void;
}) {
  return (
    <label className="grid gap-1.5 text-xs text-[var(--muted)]">
      <span className="flex justify-between">
        <span>{label}</span>
        <span className="tabular-nums text-[var(--muted-2)]">
          {Math.round(value * 100)}%
        </span>
      </span>
      <input
        type="range"
        min="0"
        max="1"
        step="0.05"
        value={value}
        onChange={(event) => onChange(Number(event.target.value))}
      />
    </label>
  );
}

function RecallView({
  query,
  results,
  onQueryChange,
  onRun,
  onTrace,
}: {
  query: string;
  results: MemoryRecallResult[];
  onQueryChange: (value: string) => void;
  onRun: () => void;
  onTrace: (memoryId: string) => void;
}) {
  return (
    <div className="mt-4 grid gap-3">
      <label className="grid gap-1.5 text-xs text-[var(--muted)]">
        Query
        <input
          value={query}
          onChange={(event) => onQueryChange(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter') onRun();
          }}
          placeholder="memory recall query"
        />
      </label>
      <button
        onClick={onRun}
        disabled={!query.trim()}
        className="px-3 py-2 text-sm font-medium rounded-md bg-[var(--accent)] text-[var(--accent-fg)] hover:bg-[var(--accent-hover)] disabled:bg-[var(--surface-3)] disabled:text-[var(--muted-2)]"
      >
        Run recall
      </button>
      <div className="grid gap-2">
        {results.map((result) => (
          <RecallRow key={result.memory.id} result={result} onTrace={onTrace} />
        ))}
        {results.length === 0 && <EmptyLine text="No recall results." />}
      </div>
    </div>
  );
}

function EvaluateView({
  selectedAgent,
  content,
  memoryType,
  memoryScope,
  tags,
  importance,
  evaluation,
  outcome,
  savedMemory,
  loading,
  onContentChange,
  onMemoryTypeChange,
  onMemoryScopeChange,
  onTagsChange,
  onImportanceChange,
  onEvaluate,
  onStore,
  onSaveDirect,
}: {
  selectedAgent: AgentSnapshot | null;
  content: string;
  memoryType: MemoryType;
  memoryScope: MemoryScope;
  tags: string;
  importance: number;
  evaluation: MemoryEvaluation | null;
  outcome: MemoryEvaluationOutcome | null;
  savedMemory: Memory | null;
  loading: boolean;
  onContentChange: (value: string) => void;
  onMemoryTypeChange: (value: MemoryType) => void;
  onMemoryScopeChange: (value: MemoryScope) => void;
  onTagsChange: (value: string) => void;
  onImportanceChange: (value: number) => void;
  onEvaluate: () => void;
  onStore: () => void;
  onSaveDirect: () => void;
}) {
  const disabled = !selectedAgent || !content.trim() || loading;

  return (
    <div className="mt-4 grid gap-3">
      <div className="rounded-md border border-[var(--border)] bg-[var(--surface-2)] px-3 py-2 text-xs text-[var(--muted)]">
        {selectedAgent ? selectedAgent.state.name : 'Select an agent'}
      </div>
      <label className="grid gap-1.5 text-xs text-[var(--muted)]">
        Type
        <select
          value={memoryType}
          onChange={(event) => onMemoryTypeChange(event.target.value as MemoryType)}
        >
          <option value="fact">Fact</option>
          <option value="observation">Observation</option>
          <option value="task_result">Task result</option>
          <option value="reflection">Reflection</option>
        </select>
      </label>
      <label className="grid gap-1.5 text-xs text-[var(--muted)]">
        Scope
        <select
          value={memoryScope}
          onChange={(event) => onMemoryScopeChange(event.target.value as MemoryScope)}
        >
          <option value="private">Private</option>
          <option value="shared">Shared</option>
          <option value="room">Room</option>
        </select>
      </label>
      <label className="grid gap-1.5 text-xs text-[var(--muted)]">
        Tags
        <input
          value={tags}
          onChange={(event) => onTagsChange(event.target.value)}
          placeholder="comma,separated,tags"
        />
      </label>
      <label className="grid gap-1.5 text-xs text-[var(--muted)]">
        Content
        <textarea
          value={content}
          onChange={(event) => onContentChange(event.target.value)}
          rows={5}
          placeholder="Candidate memory"
        />
      </label>
      <label className="grid gap-1.5 text-xs text-[var(--muted)]">
        <span className="flex justify-between">
          <span>Importance</span>
          <span className="tabular-nums text-[var(--muted-2)]">
            {Math.round(importance * 100)}%
          </span>
        </span>
        <input
          type="range"
          min="0"
          max="1"
          step="0.05"
          value={importance}
          onChange={(event) => onImportanceChange(Number(event.target.value))}
        />
      </label>
      <div className="rounded-md border border-[var(--border)] bg-[var(--surface-2)] px-3 py-2 text-xs text-[var(--muted)]">
        Evaluate previews the memory policy. Store runs the evaluated path. Save direct writes immediately.
      </div>
      <div className="grid grid-cols-3 gap-2">
        <button
          onClick={onEvaluate}
          disabled={disabled}
          className="px-3 py-2 text-sm rounded-md border border-[var(--border)] text-[var(--text-2)] hover:bg-[var(--hover)] disabled:text-[var(--muted-2)] disabled:bg-transparent"
        >
          Evaluate
        </button>
        <button
          onClick={onStore}
          disabled={disabled}
          className="px-3 py-2 text-sm font-medium rounded-md bg-[var(--accent)] text-[var(--accent-fg)] hover:bg-[var(--accent-hover)] disabled:bg-[var(--surface-3)] disabled:text-[var(--muted-2)]"
        >
          Store
        </button>
        <button
          onClick={onSaveDirect}
          disabled={disabled}
          className="px-3 py-2 text-sm rounded-md border border-[var(--border)] text-[var(--text-2)] hover:bg-[var(--hover)] disabled:text-[var(--muted-2)] disabled:bg-transparent"
        >
          Save direct
        </button>
      </div>

      {evaluation && <EvaluationResult evaluation={evaluation} outcome={outcome} />}
      {savedMemory && (
        <div className="rounded-md border border-[var(--ok)]/30 bg-[var(--ok)]/10 px-3 py-2 text-xs text-[var(--ok)]">
          saved {savedMemory.id}
        </div>
      )}
    </div>
  );
}

type GraphNode = {
  key: string;
  id: string;
  name: string;
  kind: string;
  x: number;
  y: number;
};

type GraphEdge = {
  id: string;
  sourceKey: string;
  targetKey: string;
  label: string;
  tone: GraphRelationshipTone;
  strength: number;
  confidence: number;
  tags: string[];
  evidenceMemoryIds: string[];
};

type GraphRelationshipTone = 'handoff' | 'broadcast' | 'user' | 'default';

type GraphCounts = Record<GraphFilter, number> & {
  traceable: number;
};

function buildRelationshipGraph(
  entities: MemoryEntity[],
  relationships: AgentRelationship[]
): {
  nodes: GraphNode[];
  edges: GraphEdge[];
  nodeMap: Map<string, GraphNode>;
} {
  const nodeSeed = new Map<string, { id: string; name: string; kind: string }>();

  for (const entity of entities) {
    nodeSeed.set(`${entity.kind}:${entity.id}`, {
      id: entity.id,
      name: entity.name,
      kind: entity.kind,
    });
  }

  for (const relationship of relationships) {
    nodeSeed.set(`${relationship.sourceKind}:${relationship.sourceAgentId}`, {
      id: relationship.sourceAgentId,
      name: relationship.sourceAgentName,
      kind: relationship.sourceKind,
    });
    nodeSeed.set(`${relationship.targetKind}:${relationship.targetAgentId}`, {
      id: relationship.targetAgentId,
      name: relationship.targetAgentName,
      kind: relationship.targetKind,
    });
  }

  const seeds = Array.from(nodeSeed.entries());
  const width = 640;
  const height = 320;
  const centerX = width / 2;
  const centerY = height / 2;
  const radius = Math.min(width, height) / 2 - 64;

  const nodes = seeds.map(([key, node], index) => {
    if (seeds.length === 1) {
      return { ...node, key, x: centerX, y: centerY };
    }
    const angle = (Math.PI * 2 * index) / Math.max(seeds.length, 1) - Math.PI / 2;
    return {
      ...node,
      key,
      x: centerX + Math.cos(angle) * radius,
      y: centerY + Math.sin(angle) * radius,
    };
  });

  const nodeMap = new Map(nodes.map((node) => [node.key, node]));
  const edges = relationships.map((relationship) => ({
    id: relationship.id,
    sourceKey: `${relationship.sourceKind}:${relationship.sourceAgentId}`,
    targetKey: `${relationship.targetKind}:${relationship.targetAgentId}`,
    label: relationship.relationshipType,
    tone: relationshipTone(relationship),
    strength: relationship.strength,
    confidence: relationship.confidence,
    tags: relationship.tags ?? [],
    evidenceMemoryIds: relationship.evidenceMemoryIds,
  }));

  return { nodes, edges, nodeMap };
}

function graphNodeColor(kind: string): string {
  switch (kind) {
    case 'agent':
      return '#0f766e';
    case 'user':
      return '#2563eb';
    case 'system':
      return '#7c3aed';
    default:
      return '#b45309';
  }
}

function graphEdgeColor(tone: GraphRelationshipTone): string {
  switch (tone) {
    case 'handoff':
      return '#0f766e';
    case 'broadcast':
      return '#7c3aed';
    case 'user':
      return '#2563eb';
    default:
      return '#94a3b8';
  }
}

function graphToneBackground(tone: GraphRelationshipTone): string {
  switch (tone) {
    case 'handoff':
      return 'rgba(15, 118, 110, 0.12)';
    case 'broadcast':
      return 'rgba(124, 58, 237, 0.12)';
    case 'user':
      return 'rgba(37, 99, 235, 0.12)';
    default:
      return 'rgba(148, 163, 184, 0.12)';
  }
}

function relationshipTags(relationship: AgentRelationship): string[] {
  return relationship.tags ?? [];
}

function hasRelationshipTag(relationship: AgentRelationship, tag: string): boolean {
  return relationshipTags(relationship).includes(tag);
}

function isHandoffRelationship(relationship: AgentRelationship): boolean {
  return (
    relationship.relationshipType === 'hands_off_to' ||
    hasRelationshipTag(relationship, 'relation:handoff')
  );
}

function isBroadcastRelationship(relationship: AgentRelationship): boolean {
  return (
    relationship.relationshipType === 'broadcasts_to' ||
    hasRelationshipTag(relationship, 'relation:broadcast')
  );
}

function isUserRelationship(relationship: AgentRelationship): boolean {
  return relationship.sourceKind === 'user' || relationship.targetKind === 'user';
}

function isSwarmRelationship(relationship: AgentRelationship): boolean {
  const tags = relationshipTags(relationship);
  return (
    tags.includes('swarm') ||
    tags.includes('swarm-message') ||
    isHandoffRelationship(relationship) ||
    isBroadcastRelationship(relationship)
  );
}

function relationshipTone(relationship: AgentRelationship): GraphRelationshipTone {
  if (isHandoffRelationship(relationship)) return 'handoff';
  if (isBroadcastRelationship(relationship)) return 'broadcast';
  if (isUserRelationship(relationship)) return 'user';
  return 'default';
}

function relationshipMatchesGraphFilter(
  relationship: AgentRelationship,
  filter: GraphFilter
): boolean {
  switch (filter) {
    case 'all':
      return true;
    case 'swarm':
      return isSwarmRelationship(relationship);
    case 'handoffs':
      return isHandoffRelationship(relationship);
    case 'broadcasts':
      return isBroadcastRelationship(relationship);
    case 'users':
      return isUserRelationship(relationship);
  }
}

function relationshipGraphCounts(relationships: AgentRelationship[]): GraphCounts {
  return relationships.reduce<GraphCounts>(
    (counts, relationship) => {
      counts.all += 1;
      if (isSwarmRelationship(relationship)) counts.swarm += 1;
      if (isHandoffRelationship(relationship)) counts.handoffs += 1;
      if (isBroadcastRelationship(relationship)) counts.broadcasts += 1;
      if (isUserRelationship(relationship)) counts.users += 1;
      if (relationship.evidenceMemoryIds.length > 0) counts.traceable += 1;
      return counts;
    },
    {
      all: 0,
      swarm: 0,
      handoffs: 0,
      broadcasts: 0,
      users: 0,
      traceable: 0,
    }
  );
}

function graphFilterCount(counts: GraphCounts, filter: GraphFilter): number {
  return counts[filter];
}

function truncateGraphLabel(value: string, maxLength: number): string {
  if (value.length <= maxLength) return value;
  return `${value.slice(0, maxLength - 1)}…`;
}

function graphEdgeTitle(edge: GraphEdge): string {
  const tags = edge.tags.length > 0 ? ` tags: ${edge.tags.join(', ')}` : '';
  const evidence = edge.evidenceMemoryIds.length > 0
    ? ` evidence: ${edge.evidenceMemoryIds.join(', ')}`
    : ' no evidence memory ids';
  return `${edge.label} strength ${Math.round(edge.strength * 100)}%, confidence ${Math.round(edge.confidence * 100)}%. ${tags}${evidence}`;
}

function preferredTagLabel(tags: string[]): string {
  return tags.find((tag) => tag.startsWith('relation:')) ?? tags[0] ?? '';
}

function parseTagList(value: string): string[] | undefined {
  const tags = value
    .split(',')
    .map((tag) => tag.trim())
    .filter(Boolean);

  return tags.length > 0 ? tags : undefined;
}

function trimOptional(value: string): string | undefined {
  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : undefined;
}

function upsertEntity(items: MemoryEntity[], entity: MemoryEntity): MemoryEntity[] {
  const key = `${entity.kind}:${entity.id}`;
  return [
    entity,
    ...items.filter((item) => `${item.kind}:${item.id}` !== key),
  ].slice(0, 8);
}

function upsertRelationship(
  items: AgentRelationship[],
  relationship: AgentRelationship
): AgentRelationship[] {
  return [relationship, ...items.filter((item) => item.id !== relationship.id)].slice(0, 8);
}

function EvaluationResult({
  evaluation,
  outcome,
}: {
  evaluation: MemoryEvaluation;
  outcome: MemoryEvaluationOutcome | null;
}) {
  return (
    <div className="rounded-md border border-[var(--border)] bg-[var(--surface-2)] px-3 py-2 text-xs">
      <div className="flex items-center justify-between gap-2">
        <span className="font-medium capitalize text-[var(--text)]">
          {evaluation.decision}
        </span>
        <span className="tabular-nums text-[var(--muted-2)]">
          {Math.round(evaluation.score * 100)}%
        </span>
      </div>
      <div className="mt-1 text-[var(--muted)]">{evaluation.reason}</div>
      {evaluation.duplicateMemoryId && (
        <div className="mt-1 truncate font-mono text-[var(--muted-2)]">
          {evaluation.duplicateMemoryId}
        </div>
      )}
      {outcome?.memory && (
        <div className="mt-2 rounded border border-[var(--border)] bg-[var(--surface)] px-2 py-1.5 text-[var(--muted)]">
          stored {outcome.memory.id}
        </div>
      )}
    </div>
  );
}

function MemoryRow({
  memory,
  onTrace,
}: {
  memory: Memory;
  onTrace?: (memoryId: string) => void;
}) {
  return (
    <div className="rounded-md border border-[var(--border)] bg-[var(--surface-2)] px-3 py-2">
      <div className="flex items-center justify-between gap-2">
        <span className="truncate text-sm text-[var(--text)]">
          {memory.content}
        </span>
        <span className="shrink-0 text-[11px] text-[var(--muted)]">
          {memory.type}
        </span>
      </div>
      <div className="mt-1 flex items-center justify-between gap-2 text-[11px] text-[var(--muted-2)]">
        <span className="truncate">{memory.agentName}</span>
        <span className="flex items-center gap-2">
          {onTrace && (
            <button
              onClick={() => onTrace(memory.id)}
              className="text-[11px] text-[var(--accent)] hover:underline"
            >
              Trace
            </button>
          )}
          <span className="tabular-nums">
            {Math.round(memory.importance * 100)}%
          </span>
        </span>
      </div>
    </div>
  );
}

function RelationshipRow({
  relationship,
  onTrace,
}: {
  relationship: AgentRelationship;
  onTrace?: (memoryId: string) => void;
}) {
  const tags = relationship.tags ?? [];
  const tone = relationshipTone(relationship);

  return (
    <div
      className="rounded-md border border-l-4 bg-[var(--surface-2)] px-3 py-2"
      style={{ borderColor: 'var(--border)', borderLeftColor: graphEdgeColor(tone) }}
    >
      <div className="flex items-center justify-between gap-2">
        <div className="min-w-0 truncate text-sm text-[var(--text)]">
          {relationship.sourceAgentName} {'->'} {relationship.targetAgentName}
        </div>
        <span
          className="shrink-0 rounded-full border px-2 py-0.5 text-[10px]"
          style={{
            backgroundColor: graphToneBackground(tone),
            borderColor: graphEdgeColor(tone),
            color: graphEdgeColor(tone),
          }}
        >
          {relationship.relationshipType}
        </span>
      </div>
      <div className="mt-1 flex items-center justify-between gap-2 text-[11px] text-[var(--muted-2)]">
        <span className="truncate">
          {relationship.sourceKind}/{relationship.targetKind}
        </span>
        <span className="tabular-nums">
          {Math.round(relationship.strength * 100)}%
        </span>
      </div>
      <div className="mt-2 grid gap-2">
        {relationship.summary && (
          <div className="text-xs text-[var(--muted)]">{relationship.summary}</div>
        )}
        {tags.length > 0 && (
          <div className="flex flex-wrap gap-1">
            {tags.slice(0, 5).map((tag) => (
              <span
                key={tag}
                className="rounded-full border border-[var(--border)] bg-[var(--surface)] px-2 py-0.5 text-[10px] text-[var(--muted)]"
              >
                {tag}
              </span>
            ))}
          </div>
        )}
        <div className="flex flex-wrap items-center gap-2 text-[11px] text-[var(--muted-2)]">
          <span className="tabular-nums">
            confidence {Math.round(relationship.confidence * 100)}%
          </span>
          {relationship.evidenceMemoryIds.length > 0 && onTrace &&
            relationship.evidenceMemoryIds.slice(0, 3).map((memoryId, index) => (
              <button
                key={memoryId}
                onClick={() => onTrace(memoryId)}
                className="text-[var(--accent)] hover:underline"
              >
                trace evidence {index + 1}
              </button>
            ))}
          {relationship.evidenceMemoryIds.length > 0 && !onTrace && (
            <span>{relationship.evidenceMemoryIds.length} evidence memories</span>
          )}
          {relationship.evidenceMemoryIds.length === 0 && (
            <span>no evidence memories</span>
          )}
        </div>
      </div>
    </div>
  );
}

function RecallRow({
  result,
  onTrace,
}: {
  result: MemoryRecallResult;
  onTrace: (memoryId: string) => void;
}) {
  return (
    <div className="rounded-md border border-[var(--border)] bg-[var(--surface-2)] px-3 py-2">
      <div className="flex items-center justify-between gap-2">
        <span className="truncate text-sm text-[var(--text)]">
          {result.memory.content}
        </span>
        <span className="shrink-0 tabular-nums text-xs text-[var(--muted)]">
          {Math.round(result.score * 100)}%
        </span>
      </div>
      <div className="mt-2 grid grid-cols-6 gap-1 text-[10px] text-[var(--muted-2)]">
        <Score label="lex" value={result.lexicalScore} />
        <Score label="vec" value={result.vectorScore} />
        <Score label="rel" value={result.relationshipScore} />
        <Score label="tmp" value={result.temporalScore} />
        <Score label="new" value={result.recencyScore} />
        <Score label="imp" value={result.importanceScore} />
      </div>
      <button
        onClick={() => onTrace(result.memory.id)}
        className="mt-2 text-[11px] text-[var(--accent)] hover:underline"
      >
        Trace evidence
      </button>
    </div>
  );
}

function TraceView({
  trace,
  onClose,
}: {
  trace: MemoryEvidenceTrace;
  onClose: () => void;
}) {
  return (
    <div className="mt-4 grid gap-3 rounded-md border border-[var(--border)] bg-[var(--surface-2)] p-3">
      <div className="flex items-center justify-between gap-2">
        <span className="text-[11px] font-medium uppercase tracking-wide text-[var(--muted-2)]">
          Evidence trace
        </span>
        <button
          onClick={onClose}
          className="text-[11px] text-[var(--muted)] hover:text-[var(--text)]"
        >
          Close
        </button>
      </div>
      <MemoryRow memory={trace.memory} />

      <div className="grid gap-2">
        <SectionTitle title="Edges" count={trace.relationships.length} />
        {trace.relationships.map((relationship) => (
          <RelationshipRow
            key={relationship.id}
            relationship={relationship}
          />
        ))}
        {trace.relationships.length === 0 && <EmptyLine text="No citing edges." />}
      </div>

      <div className="grid gap-2">
        <SectionTitle title="Entities" count={trace.entities.length} />
        {trace.entities.map((entity) => (
          <div
            key={`${entity.kind}:${entity.id}`}
            className="rounded-md border border-[var(--border)] bg-[var(--surface)] px-3 py-2"
          >
            <div className="flex items-center justify-between gap-2">
              <span className="truncate text-xs font-medium text-[var(--text)]">
                {entity.name}
              </span>
              <span className="shrink-0 text-[11px] text-[var(--muted)]">
                {entity.kind}
              </span>
            </div>
            <div className="mt-1 truncate font-mono text-[11px] text-[var(--muted-2)]">
              {entity.id}
            </div>
          </div>
        ))}
        {trace.entities.length === 0 && <EmptyLine text="No entities." />}
      </div>
    </div>
  );
}

function Score({ label, value }: { label: string; value: number }) {
  return (
    <div className="rounded border border-[var(--border)] bg-[var(--surface)] px-1.5 py-1 text-center">
      <div>{label}</div>
      <div className="tabular-nums text-[var(--text-2)]">
        {Math.round(value * 100)}
      </div>
    </div>
  );
}

function SectionTitle({ title, count }: { title: string; count: number }) {
  return (
    <div className="flex items-center justify-between gap-2 border-t border-[var(--border)] pt-3 first:border-t-0 first:pt-0">
      <span className="text-[11px] font-medium uppercase tracking-wide text-[var(--muted-2)]">
        {title}
      </span>
      <span className="tabular-nums text-[11px] text-[var(--muted-2)]">
        {count}
      </span>
    </div>
  );
}

function EmptyLine({ text }: { text: string }) {
  return <div className="text-xs text-[var(--muted)]">{text}</div>;
}