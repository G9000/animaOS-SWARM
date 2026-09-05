import { describe, expect, it } from 'vitest';
import { conversationMarkdown, conversationFilename } from './conversation';
import type { AgentDetail } from './types';

describe('conversation export', () => {
  it('exports every loaded message with its role, timestamp and original Markdown', () => {
    const agent = {
      name: 'Nova',
      messages: [
        {
          id: '1',
          role: 'User',
          content: { text: '**My idea**' },
          created_at_ms: 0,
        },
        {
          id: '2',
          role: 'Tool',
          content: { text: 'result' },
          created_at_ms: 1000,
        },
      ],
    } as AgentDetail;
    const output = conversationMarkdown(agent);
    expect(output).toContain('# Nova — conversation');
    expect(output).toContain('1970-01-01T00:00:00.000Z');
    expect(output).toContain('**My idea**');
    expect(output).toContain('## Tool');
    expect(output).toContain('result');
  });
  it('creates a safe filename even for punctuation-only names', () => {
    expect(conversationFilename('../Nova: work')).toBe(
      'nova-work-conversation.md',
    );
    expect(conversationFilename('///')).toBe('workspace-conversation.md');
  });
});
