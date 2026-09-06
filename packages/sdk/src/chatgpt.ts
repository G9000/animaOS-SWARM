import type { DaemonClient } from './client.js';

export interface ChatGptLogin {
  userCode: string;
  verificationUrl: string;
  expiresAtMs: number;
}

/** Redacted daemon-owned subscription state. Credentials never leave the host. */
export interface ChatGptStatus {
  connected: boolean;
  accountId: string | null;
  planType: string | null;
  login: ChatGptLogin | null;
  error: string | null;
}

export class ChatGptClient {
  constructor(private readonly client: DaemonClient) {}

  status(): Promise<ChatGptStatus> {
    return this.client.requestJson('/api/providers/chatgpt/status', {
      headers: { 'cache-control': 'no-store' },
    });
  }

  login(): Promise<ChatGptStatus> {
    return this.client.requestJson('/api/providers/chatgpt/login', {
      method: 'POST',
      headers: { 'cache-control': 'no-store' },
    });
  }

  cancel(): Promise<ChatGptStatus> {
    return this.client.requestJson('/api/providers/chatgpt/login', {
      method: 'DELETE',
      headers: { 'cache-control': 'no-store' },
    });
  }

  disconnect(): Promise<ChatGptStatus> {
    return this.client.requestJson('/api/providers/chatgpt', {
      method: 'DELETE',
      headers: { 'cache-control': 'no-store' },
    });
  }
}
