import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import type { AgentDetail } from '../lib/types';
import { formatTime } from './ui-bits';
import { MessageList } from './ChatScreen';

const messages: AgentDetail['messages'] = [
  {
    id: 'user-message',
    role: 'User',
    content: { text: '**bold**' },
    created_at_ms: 1_725_000_000_000,
  },
  {
    id: 'assistant-message',
    role: 'Assistant',
    content: { text: '## Heading' },
    created_at_ms: 1_725_000_060_000,
  },
  {
    id: 'system-message',
    role: 'System',
    content: { text: '**system marker**' },
    created_at_ms: 1_725_000_120_000,
  },
  {
    id: 'tool-message',
    role: 'Tool',
    content: { text: '## tool marker' },
    created_at_ms: 1_725_000_180_000,
  },
];

const agent: AgentDetail = {
  id: 'agent-1',
  name: 'Nova',
  provider: 'openai',
  model: 'gpt-5',
  toolNames: ['search'],
  created_at_ms: 1_725_000_000_000,
  status: 'Idle',
  token_usage: { prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 },
  messages,
};

describe('MessageList', () => {
  it('renders Markdown for user and assistant bubbles while keeping event pills literal', () => {
    const scrollerRef = { current: null };
    render(
      <MessageList
        agent={agent}
        sending={false}
        scrollerRef={scrollerRef}
        onSuggestion={vi.fn()}
      />,
    );

    expect(screen.getByText('bold').tagName).toBe('STRONG');
    expect(screen.getByRole('heading', { level: 2, name: 'Heading' })).toBeVisible();
    expect(screen.getByText('system · **system marker**')).toBeVisible();
    expect(screen.getByText('tool · ## tool marker')).toBeVisible();
    expect(screen.getByText('system · **system marker**').tagName).toBe('SPAN');
    expect(screen.queryByRole('heading', { name: 'tool marker' })).not.toBeInTheDocument();
  });

  it('retains the conversation label and message timestamps', () => {
    const scrollerRef = { current: null };
    render(
      <MessageList
        agent={agent}
        sending={false}
        scrollerRef={scrollerRef}
        onSuggestion={vi.fn()}
      />,
    );

    expect(screen.getByLabelText('Conversation with Nova')).toBeVisible();
    expect(screen.getByText(formatTime(messages[0].created_at_ms))).toBeVisible();
    expect(screen.getByText(formatTime(messages[1].created_at_ms))).toBeVisible();
  });
});
