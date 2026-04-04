import React, { useState, useCallback } from 'react';
import { Box, Text, useInput } from 'ink';
import type { AgentProfile } from '../types.js';

type Phase = 'list' | 'detail' | 'edit';

const EDITABLE_FIELDS = [
  'bio',
  'lore',
  'adjectives',
  'topics',
  'knowledge',
  'style',
  'system',
] as const;

type EditableField = (typeof EDITABLE_FIELDS)[number];

const ARRAY_FIELDS = new Set<EditableField>([
  'adjectives',
  'topics',
  'knowledge',
]);

function getValue(profile: AgentProfile, field: EditableField): string {
  const val = profile[field];
  if (Array.isArray(val)) return val.join(', ');
  return val ?? '';
}

function applyValue(
  profile: AgentProfile,
  field: EditableField,
  value: string
): AgentProfile {
  if (ARRAY_FIELDS.has(field)) {
    return {
      ...profile,
      [field]: value
        .split(',')
        .map((s) => s.trim())
        .filter(Boolean),
    };
  }
  return { ...profile, [field]: value };
}

export interface AgentsPanelProps {
  profiles: AgentProfile[];
  onBack: () => void;
  onSave: (profile: AgentProfile) => void;
}

export function AgentsPanel({
  profiles,
  onBack,
  onSave,
}: AgentsPanelProps): React.ReactElement {
  const [phase, setPhase] = useState<Phase>('list');
  const [agentIdx, setAgentIdx] = useState(0);
  const [fieldIdx, setFieldIdx] = useState(0);
  const [editValue, setEditValue] = useState('');
  const [draft, setDraft] = useState<AgentProfile | null>(null);
  const [savedMsg, setSavedMsg] = useState(false);

  const selected = profiles[agentIdx] ?? profiles[0];

  const enterEdit = useCallback((profile: AgentProfile) => {
    const d = { ...profile };
    setDraft(d);
    setFieldIdx(0);
    setEditValue(getValue(d, EDITABLE_FIELDS[0]));
    setPhase('edit');
  }, []);

  // Saves current editValue into draft and switches to a new field index
  const switchField = useCallback(
    (newIdx: number) => {
      setDraft((prev) => {
        if (!prev) return prev;
        const field = EDITABLE_FIELDS[fieldIdx];
        const updated = applyValue(prev, field, editValue);
        setEditValue(getValue(updated, EDITABLE_FIELDS[newIdx]));
        return updated;
      });
      setFieldIdx(newIdx);
    },
    [fieldIdx, editValue]
  );

  const saveAll = useCallback(() => {
    setDraft((prev) => {
      if (!prev) return prev;
      const field = EDITABLE_FIELDS[fieldIdx];
      const updated = applyValue(prev, field, editValue);
      onSave(updated);
      setSavedMsg(true);
      setTimeout(() => setSavedMsg(false), 2500);
      return updated;
    });
  }, [fieldIdx, editValue, onSave]);

  useInput((input, key) => {
    if (phase === 'list') {
      if (key.upArrow) setAgentIdx((i) => Math.max(0, i - 1));
      else if (key.downArrow)
        setAgentIdx((i) => Math.min(profiles.length - 1, i + 1));
      else if (key.return) setPhase('detail');
      else if (input === 'e') enterEdit(selected);
      else if (input === 'q' || key.escape) onBack();
    } else if (phase === 'detail') {
      if (input === 'e') enterEdit(selected);
      else if (input === 'q' || key.escape) setPhase('list');
    } else if (phase === 'edit') {
      if (key.ctrl && input === 's') {
        saveAll();
      } else if (key.escape) {
        setPhase('detail');
      } else if (key.upArrow) {
        switchField(Math.max(0, fieldIdx - 1));
      } else if (key.downArrow) {
        switchField(Math.min(EDITABLE_FIELDS.length - 1, fieldIdx + 1));
      } else if (key.backspace || key.delete) {
        setEditValue((v) => v.slice(0, -1));
      } else if (!key.ctrl && !key.meta && !key.escape && !key.tab) {
        setEditValue((v) => v + input);
      }
    }
  });

  // ── List view ──
  if (phase === 'list') {
    return (
      <Box flexDirection="column" borderStyle="single" paddingX={1}>
        <Text bold color="white">
          Agents ({profiles.length})
        </Text>
        <Box flexDirection="column" marginTop={1}>
          {profiles.map((p, i) => {
            const active = i === agentIdx;
            const prefix = p.role === 'orchestrator' ? '★' : '•';
            return (
              <Box key={p.name}>
                <Text color={active ? 'cyan' : 'gray'} bold={active}>
                  {active ? '❯ ' : '  '}
                  {prefix} {p.name}
                </Text>
                <Text color="gray">
                  {'  '}
                  {p.role ?? 'worker'}
                </Text>
                {p.adjectives && p.adjectives.length > 0 ? (
                  <Text color="gray">
                    {'  ·  '}
                    {p.adjectives.slice(0, 3).join(', ')}
                  </Text>
                ) : null}
              </Box>
            );
          })}
        </Box>
        <Box marginTop={1}>
          <Text color="gray" dimColor>
            ↑↓ navigate{'  '}enter view{'  '}e edit{'  '}q back
          </Text>
        </Box>
      </Box>
    );
  }

  // ── Detail view ──
  if (phase === 'detail') {
    const p = selected;
    return (
      <Box flexDirection="column" borderStyle="single" paddingX={1}>
        <Box>
          <Text bold color="cyan">
            {p.name}
          </Text>
          <Text color="gray">
            {'  '}
            {p.role ?? 'worker'}
          </Text>
        </Box>
        {p.bio ? (
          <Box flexDirection="column" marginTop={1}>
            <Text bold>Bio</Text>
            <Text color="gray">
              {'  '}
              {p.bio}
            </Text>
          </Box>
        ) : null}
        {p.lore ? (
          <Box flexDirection="column" marginTop={1}>
            <Text bold>Backstory</Text>
            <Text color="gray">
              {'  '}
              {p.lore}
            </Text>
          </Box>
        ) : null}
        {p.adjectives && p.adjectives.length > 0 ? (
          <Box marginTop={1}>
            <Text bold>Personality{'  '}</Text>
            <Text color="magenta">{p.adjectives.join(', ')}</Text>
          </Box>
        ) : null}
        {p.topics && p.topics.length > 0 ? (
          <Box marginTop={1}>
            <Text bold>Topics{'  '}</Text>
            <Text color="cyan">{p.topics.join(', ')}</Text>
          </Box>
        ) : null}
        {p.knowledge && p.knowledge.length > 0 ? (
          <Box flexDirection="column" marginTop={1}>
            <Text bold>Knowledge</Text>
            {p.knowledge.map((k) => (
              <Text key={k} color="gray">
                {'  · '}
                {k}
              </Text>
            ))}
          </Box>
        ) : null}
        {p.style ? (
          <Box flexDirection="column" marginTop={1}>
            <Text bold>Style</Text>
            <Text color="gray">
              {'  '}
              {p.style}
            </Text>
          </Box>
        ) : null}
        {p.system ? (
          <Box flexDirection="column" marginTop={1}>
            <Text bold>System</Text>
            <Text color="gray">
              {'  '}
              {p.system}
            </Text>
          </Box>
        ) : null}
        <Box marginTop={1}>
          <Text color="gray" dimColor>
            e edit{'  '}q back to list
          </Text>
        </Box>
      </Box>
    );
  }

  // ── Edit view ──
  return (
    <Box flexDirection="column" borderStyle="single" paddingX={1}>
      <Box>
        <Text bold>Editing{'  '}</Text>
        <Text bold color="cyan">
          {selected.name}
        </Text>
        {savedMsg ? (
          <Text color="green">{'  '}✓ saved to anima.yaml</Text>
        ) : null}
      </Box>
      <Box flexDirection="column" marginTop={1}>
        {EDITABLE_FIELDS.map((field, i) => {
          const active = i === fieldIdx;
          const isArray = ARRAY_FIELDS.has(field);
          const displayVal = active
            ? editValue
            : draft
            ? getValue(draft, field)
            : '';
          return (
            <Box key={field}>
              <Text color={active ? 'cyan' : 'gray'} bold={active}>
                {active ? '❯ ' : '  '}
                {field.padEnd(11)}
              </Text>
              <Text color={active ? 'white' : 'gray'}>
                {displayVal || '(empty)'}
                {active ? <Text color="cyan">▌</Text> : null}
              </Text>
              {active && isArray ? (
                <Text color="gray" dimColor>
                  {'  '}comma-separated
                </Text>
              ) : null}
            </Box>
          );
        })}
      </Box>
      <Box marginTop={1}>
        <Text color="gray" dimColor>
          ↑↓ switch field{'  '}ctrl+s save{'  '}esc cancel
        </Text>
      </Box>
    </Box>
  );
}
