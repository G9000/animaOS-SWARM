export interface SearchResult {
  id?: string;
  score?: number;
  task?: string;
  result?: string;
  content?: string;
  docId?: string;
  chunkId?: string;
  text?: string;
  [key: string]: unknown;
}

export interface SearchResultCardDisplay {
  key: string;
  label: string;
  title: string;
  preview: string;
}

function serializePayload(payload: unknown): string {
  if (typeof payload === 'string') {
    return payload;
  }

  if (payload === null || typeof payload === 'undefined') {
    return 'No payload returned.';
  }

  try {
    return JSON.stringify(payload, null, 2);
  } catch {
    return String(payload);
  }
}

function extractResultPreview(result: SearchResult): string {
  if (typeof result.content === 'string') {
    return result.content;
  }

  if (typeof result.result === 'string') {
    return result.result;
  }

  if (typeof result.text === 'string') {
    return result.text;
  }

  if (typeof result.task === 'string') {
    return result.task;
  }

  return serializePayload(result);
}

function getDocumentChunkTitle(result: SearchResult): string {
  if (typeof result.chunkId !== 'string') {
    return 'Indexed document excerpt';
  }

  if (
    typeof result.docId === 'string' &&
    result.chunkId.startsWith(`${result.docId}:`)
  ) {
    return `chunk ${result.chunkId.slice(result.docId.length + 1)}`;
  }

  return `chunk ${result.chunkId}`;
}

export function formatTaskHistoryResult(
  result: SearchResult,
  index: number
): SearchResultCardDisplay {
  return {
    key: typeof result.id === 'string' ? result.id : `history-${index + 1}`,
    label:
      typeof result.id === 'string' ? result.id : `result-${String(index + 1)}`,
    title: typeof result.task === 'string' ? result.task : 'Task history entry',
    preview: extractResultPreview(result),
  };
}

export function formatDocumentResult(
  result: SearchResult,
  index: number
): SearchResultCardDisplay {
  const label =
    typeof result.docId === 'string'
      ? result.docId
      : typeof result.id === 'string'
      ? result.id
      : `document-${String(index + 1)}`;

  return {
    key:
      typeof result.chunkId === 'string'
        ? result.chunkId
        : typeof result.docId === 'string'
        ? result.docId
        : typeof result.id === 'string'
        ? result.id
        : `document-${index + 1}`,
    label,
    title: getDocumentChunkTitle(result),
    preview:
      typeof result.text === 'string'
        ? result.text
        : extractResultPreview(result),
  };
}
