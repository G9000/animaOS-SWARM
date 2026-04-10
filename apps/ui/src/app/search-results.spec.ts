import { describe, expect, it } from 'vitest';
import {
  formatDocumentResult,
  formatTaskHistoryResult,
} from './search-results';

describe('ui search result formatting', () => {
  it('formats task history cards with task titles and result previews', () => {
    expect(
      formatTaskHistoryResult(
        {
          id: 'history-1',
          task: 'Trace launch backlog',
          result: 'Mock completion for: Trace launch backlog',
        },
        0
      )
    ).toEqual({
      key: 'history-1',
      label: 'history-1',
      title: 'Trace launch backlog',
      preview: 'Mock completion for: Trace launch backlog',
    });
  });

  it('formats document search cards with document and chunk labels', () => {
    expect(
      formatDocumentResult(
        {
          docId: 'ops-playbook',
          chunkId: 'ops-playbook:0',
          text: 'Escalate the overnight incident before paging a reviewer.',
          score: 0.58,
        },
        0
      )
    ).toEqual({
      key: 'ops-playbook:0',
      label: 'ops-playbook',
      title: 'chunk 0',
      preview: 'Escalate the overnight incident before paging a reviewer.',
    });
  });

  it('falls back to serialized payloads for unknown result shapes', () => {
    const formatted = formatDocumentResult(
      {
        id: 'document-9',
        chunkId: 'chunk-9',
        unexpected: true,
      },
      8
    );

    expect(formatted.label).toBe('document-9');
    expect(formatted.title).toBe('chunk chunk-9');
    expect(formatted.preview).toContain('"unexpected": true');
  });
});
