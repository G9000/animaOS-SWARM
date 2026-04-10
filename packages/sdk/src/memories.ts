import type {
  Memory,
  MemorySearchOptions,
  MemorySearchResult,
  MemoryType,
} from '@animaOS-SWARM/memory';

import type { DaemonClient } from './client.js';

export interface CreateMemoryInput {
  agentId: string;
  agentName: string;
  type: MemoryType;
  content: string;
  importance: number;
  tags?: string[] | null;
}

export interface RecentMemoriesOptions {
  agentId?: string;
  agentName?: string;
  limit?: number;
}

export class MemoriesClient {
  constructor(private readonly client: DaemonClient) {}

  async create(input: CreateMemoryInput): Promise<Memory> {
    return this.client.requestJson<Memory>('/api/memories', {
      method: 'POST',
      body: input,
    });
  }

  async search(
    query: string,
    options: MemorySearchOptions = {}
  ): Promise<MemorySearchResult[]> {
    const search = new URLSearchParams();
    search.set('q', query);

    if (options.agentId !== undefined) {
      search.set('agentId', options.agentId);
    }
    if (options.agentName !== undefined) {
      search.set('agentName', options.agentName);
    }
    if (options.type !== undefined) {
      search.set('type', options.type);
    }
    if (options.limit !== undefined) {
      search.set('limit', String(options.limit));
    }
    if (options.minImportance !== undefined) {
      search.set('minImportance', String(options.minImportance));
    }

    const response = await this.client.requestJson<{
      results: MemorySearchResult[];
    }>(`/api/memories/search?${search.toString()}`);

    return response.results;
  }

  async recent(options: RecentMemoriesOptions = {}): Promise<Memory[]> {
    const search = new URLSearchParams();

    if (options.agentId !== undefined) {
      search.set('agentId', options.agentId);
    }
    if (options.agentName !== undefined) {
      search.set('agentName', options.agentName);
    }
    if (options.limit !== undefined) {
      search.set('limit', String(options.limit));
    }

    const path = search.size
      ? `/api/memories/recent?${search.toString()}`
      : '/api/memories/recent';
    const response = await this.client.requestJson<{ memories: Memory[] }>(
      path
    );

    return response.memories;
  }
}
