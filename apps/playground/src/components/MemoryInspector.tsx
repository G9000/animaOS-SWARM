import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  memories,
  type AgentRelationship,
  type AgentSnapshot,
  type EvaluatedMemoryInput,
  type Memory,
  type MemoryEntity,
  type MemoryEvaluation,
  type MemoryEvaluationOutcome,
  type MemoryRecallResult,
  type MemoryType,
} from '../lib/api';
import { playgroundUserId } from '../lib/playgroundUser';

type InspectorTab = 'state' | 'recall' | 'evaluate';

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
  const [recallQuery, setRecallQuery] = useState('relationship evidence probe');
  const [recallResults, setRecallResults] = useState<MemoryRecallResult[]>([]);
  const [draftContent, setDraftContent] = useState('');
  const [draftType, setDraftType] = useState<MemoryType>('fact');
  const [importance, setImportance] = useState(0.6);
  const [evaluation, setEvaluation] = useState<MemoryEvaluation | null>(null);
  const [outcome, setOutcome] = useState<MemoryEvaluationOutcome | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const selectedAgent = useMemo(
    () => agents.find((agent) => agent.state.id === agentId) ?? null,
    [agentId, agents]
  );

  useEffect(() => {
    if (selectedAgentId) {
      setAgentId(selectedAgentId);
    }
  }, [selectedAgentId]);

  useEffect(() => {
    setEntityId((current) => current || playgroundUserId());
  }, []);

  const loadState = useCallback(async () => {
    setLoading(true);
    try {
      const trimmedEntityId = entityId.trim();
      const [recentMemories, entityList, relationshipList] = await Promise.all([
        memories.recent({
          agentId: agentId || undefined,
          limit: 8,
        }),
        memories.entities({ limit: 8 }),
        memories.relationships({
          agentId: agentId || undefined,
          entityId: trimmedEntityId || undefined,
          limit: 8,
        }),
      ]);
      setRecent(recentMemories);
      setEntities(entityList);
      setRelationships(relationshipList);
      setError(null);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setLoading(false);
    }
  }, [agentId, entityId]);

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
          {(['state', 'recall', 'evaluate'] as const).map((value) => (
            <button
              key={value}
              onClick={() => setTab(value)}
              className={`flex-1 rounded px-2 py-1 text-xs capitalize ${
                tab === value
                  ? 'bg-[var(--surface-3)] text-[var(--text)]'
                  : 'text-[var(--muted)] hover:text-[var(--text-2)]'
              }`}
            >
              {value}
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
          />
        )}

        {tab === 'recall' && (
          <RecallView
            query={recallQuery}
            results={recallResults}
            onQueryChange={setRecallQuery}
            onRun={runRecall}
          />
        )}

        {tab === 'evaluate' && (
          <EvaluateView
            selectedAgent={selectedAgent}
            content={draftContent}
            memoryType={draftType}
            importance={importance}
            evaluation={evaluation}
            outcome={outcome}
            loading={loading}
            onContentChange={setDraftContent}
            onMemoryTypeChange={setDraftType}
            onImportanceChange={setImportance}
            onEvaluate={() => evaluateDraft(false)}
            onStore={() => evaluateDraft(true)}
          />
        )}
      </div>
    </aside>
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
}: {
  recent: Memory[];
  entities: MemoryEntity[];
  relationships: AgentRelationship[];
}) {
  return (
    <div className="mt-4 grid gap-4">
      <SectionTitle title="Recent" count={recent.length} />
      <div className="grid gap-2">
        {recent.map((memory) => (
          <MemoryRow key={memory.id} memory={memory} />
        ))}
        {recent.length === 0 && <EmptyLine text="No memories." />}
      </div>

      <SectionTitle title="Relationships" count={relationships.length} />
      <div className="grid gap-2">
        {relationships.map((relationship) => (
          <RelationshipRow key={relationship.id} relationship={relationship} />
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

function RecallView({
  query,
  results,
  onQueryChange,
  onRun,
}: {
  query: string;
  results: MemoryRecallResult[];
  onQueryChange: (value: string) => void;
  onRun: () => void;
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
          <RecallRow key={result.memory.id} result={result} />
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
  importance,
  evaluation,
  outcome,
  loading,
  onContentChange,
  onMemoryTypeChange,
  onImportanceChange,
  onEvaluate,
  onStore,
}: {
  selectedAgent: AgentSnapshot | null;
  content: string;
  memoryType: MemoryType;
  importance: number;
  evaluation: MemoryEvaluation | null;
  outcome: MemoryEvaluationOutcome | null;
  loading: boolean;
  onContentChange: (value: string) => void;
  onMemoryTypeChange: (value: MemoryType) => void;
  onImportanceChange: (value: number) => void;
  onEvaluate: () => void;
  onStore: () => void;
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
      <div className="grid grid-cols-2 gap-2">
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
      </div>

      {evaluation && <EvaluationResult evaluation={evaluation} outcome={outcome} />}
    </div>
  );
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

function MemoryRow({ memory }: { memory: Memory }) {
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
        <span className="tabular-nums">{Math.round(memory.importance * 100)}%</span>
      </div>
    </div>
  );
}

function RelationshipRow({ relationship }: { relationship: AgentRelationship }) {
  return (
    <div className="rounded-md border border-[var(--border)] bg-[var(--surface-2)] px-3 py-2">
      <div className="truncate text-sm text-[var(--text)]">
        {relationship.sourceAgentName} {'->'} {relationship.targetAgentName}
      </div>
      <div className="mt-1 flex items-center justify-between gap-2 text-[11px] text-[var(--muted-2)]">
        <span className="truncate">
          {relationship.sourceKind}/{relationship.targetKind} / {relationship.relationshipType}
        </span>
        <span className="tabular-nums">
          {Math.round(relationship.strength * 100)}%
        </span>
      </div>
    </div>
  );
}

function RecallRow({ result }: { result: MemoryRecallResult }) {
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
      <div className="mt-2 grid grid-cols-5 gap-1 text-[10px] text-[var(--muted-2)]">
        <Score label="lex" value={result.lexicalScore} />
        <Score label="vec" value={result.vectorScore} />
        <Score label="rel" value={result.relationshipScore} />
        <Score label="new" value={result.recencyScore} />
        <Score label="imp" value={result.importanceScore} />
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