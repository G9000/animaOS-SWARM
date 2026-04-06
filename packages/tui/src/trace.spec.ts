import { describe, expect, it } from 'vitest';
import {
  buildTraceEntries,
  traceEntryDetail,
  traceEntrySummary,
} from './trace.js';

describe('trace helpers', () => {
  it('merges messages and tools into a time-ordered activity list', () => {
    const entries = buildTraceEntries(
      [
        {
          id: 'msg-2',
          from: 'writer',
          to: 'user',
          content: 'Final answer',
          timestamp: 30,
        },
        {
          id: 'msg-1',
          from: 'user',
          to: 'manager',
          content: 'Research the topic',
          timestamp: 10,
        },
      ],
      [
        {
          id: 'tool-1',
          agentId: 'launch:manager',
          agentName: 'manager',
          toolName: 'memory_search',
          args: { query: 'topic' },
          status: 'success',
          durationMs: 42,
          timestamp: 20,
        },
      ]
    );

    expect(entries.map((entry) => entry.id)).toEqual([
      'msg-1',
      'tool-1',
      'msg-2',
    ]);
  });

  it('uses entry ids as a stable tiebreaker when timestamps match', () => {
    const entries = buildTraceEntries(
      [
        {
          id: 'msg-b',
          from: 'writer',
          to: 'user',
          content: 'Later by id',
          timestamp: 10,
        },
      ],
      [
        {
          id: 'tool-a',
          agentId: 'launch:manager',
          agentName: 'manager',
          toolName: 'memory_search',
          args: { query: 'topic' },
          status: 'success',
          durationMs: 42,
          timestamp: 10,
        },
      ]
    );

    expect(entries.map((entry) => entry.id)).toEqual(['msg-b', 'tool-a']);
  });

  it('formats summaries and details for tool entries', () => {
    const [entry] = buildTraceEntries(
      [],
      [
        {
          id: 'tool-1',
          agentId: 'launch:manager',
          agentName: 'manager',
          toolName: 'memory_search',
          args: { query: 'topic', limit: 3 },
          status: 'error',
          result: 'search backend unavailable',
          durationMs: 42,
          timestamp: 20,
        },
      ]
    );

    expect(traceEntrySummary(entry)).toContain('manager [err] memory_search');
    expect(traceEntryDetail(entry)).toContain('Args:');
    expect(traceEntryDetail(entry)).toContain('"query": "topic"');
    expect(traceEntryDetail(entry)).toContain('Result:');
    expect(traceEntryDetail(entry)).toContain('search backend unavailable');
  });

  it('truncates long message and tool summaries', () => {
    const [messageEntry, toolEntry] = buildTraceEntries(
      [
        {
          id: 'msg-1',
          from: 'user',
          to: 'manager',
          content: 'A'.repeat(80),
          timestamp: 10,
        },
      ],
      [
        {
          id: 'tool-1',
          agentId: 'launch:manager',
          agentName: 'manager',
          toolName: 'memory_search',
          args: { query: 'B'.repeat(80) },
          status: 'success',
          durationMs: 12,
          timestamp: 20,
        },
      ]
    );

    expect(traceEntrySummary(messageEntry)).toContain('user -> manager');
    expect(traceEntrySummary(messageEntry).endsWith('...')).toBe(true);
    expect(traceEntrySummary(toolEntry)).toContain(
      'manager [ok] memory_search('
    );
    expect(traceEntrySummary(toolEntry)).toContain('...');
    expect(traceEntrySummary(toolEntry)).toContain(' 12ms');
  });

  it('pretty-prints structured JSON tool results in details', () => {
    const [entry] = buildTraceEntries(
      [],
      [
        {
          id: 'tool-2',
          agentId: 'launch:manager',
          agentName: 'manager',
          toolName: 'memory_search',
          args: { query: 'topic' },
          status: 'success',
          result: '{"items":["alpha","beta"],"count":2}',
          durationMs: 21,
          timestamp: 40,
        },
      ]
    );

    expect(traceEntryDetail(entry)).toContain('"items": [');
    expect(traceEntryDetail(entry)).toContain('"alpha"');
    expect(traceEntryDetail(entry)).toContain('"count": 2');
  });

  it('pretty-prints JSON array tool results with surrounding whitespace', () => {
    const [entry] = buildTraceEntries(
      [],
      [
        {
          id: 'tool-3',
          agentId: 'launch:manager',
          agentName: 'manager',
          toolName: 'memory_search',
          args: { query: 'topic' },
          status: 'success',
          result: '  [ {"id":1}, {"id":2} ]  ',
          durationMs: 21,
          timestamp: 50,
        },
      ]
    );

    expect(traceEntryDetail(entry)).toContain('[\n  {');
    expect(traceEntryDetail(entry)).toContain('"id": 1');
    expect(traceEntryDetail(entry)).toContain('"id": 2');
  });

  it('falls back to raw text for invalid structured-looking tool results', () => {
    const invalid = '{"items": [1, 2,}';
    const [entry] = buildTraceEntries(
      [],
      [
        {
          id: 'tool-4',
          agentId: 'launch:manager',
          agentName: 'manager',
          toolName: 'memory_search',
          args: { query: 'topic' },
          status: 'error',
          result: invalid,
          durationMs: 21,
          timestamp: 60,
        },
      ]
    );

    expect(traceEntryDetail(entry)).toContain('Result:');
    expect(traceEntryDetail(entry)).toContain(invalid);
  });

  it('shows a placeholder when streamed tool results are unavailable', () => {
    const [entry] = buildTraceEntries(
      [],
      [
        {
          id: 'tool-5',
          agentId: 'launch:manager',
          agentName: 'manager',
          toolName: 'memory_search',
          args: { query: 'topic' },
          status: 'success',
          durationMs: 21,
          timestamp: 70,
        },
      ]
    );

    expect(traceEntryDetail(entry)).toContain(
      '(not available in the streamed event payload)'
    );
  });
});
