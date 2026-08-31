import { fireEvent, render, screen } from '@testing-library/react';
import { useRef, useState } from 'react';
import { describe, expect, it, vi } from 'vitest';
import type { AgentDetail } from '../lib/types';

const markdownRenderProbe = vi.hoisted(() => vi.fn());

vi.mock('./MarkdownMessage', () => ({
  MarkdownMessage: ({ children }: { children: string }) => {
    markdownRenderProbe();
    return <div data-testid="markdown-message">{children}</div>;
  },
}));

import { MessageList } from './ChatScreen';

const onSuggestion = vi.fn();

const initialAgent: AgentDetail = {
  id: 'agent-1',
  name: 'Nova',
  provider: 'openai',
  model: 'gpt-5',
  toolNames: [],
  created_at_ms: 1_725_000_000_000,
  status: 'Idle',
  token_usage: { prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 },
  messages: [
    {
      id: 'assistant-message',
      role: 'Assistant',
      content: { text: 'Initial response' },
      created_at_ms: 1_725_000_000_000,
    },
  ],
};

const updatedAgent: AgentDetail = {
  ...initialAgent,
  messages: [
    ...initialAgent.messages,
    {
      id: 'next-assistant-message',
      role: 'Assistant',
      content: { text: 'Updated response' },
      created_at_ms: 1_725_000_060_000,
    },
  ],
};

function DraftHarness() {
  const [draft, setDraft] = useState('');
  const [agent, setAgent] = useState(initialAgent);
  const [sending, setSending] = useState(false);
  const scrollerRef = useRef<HTMLDivElement>(null);

  return (
    <>
      <input
        aria-label="Draft"
        onChange={(event) => setDraft(event.target.value)}
        value={draft}
      />
      <button onClick={() => setSending(true)} type="button">
        Start sending
      </button>
      <button onClick={() => setAgent(updatedAgent)} type="button">
        Update messages
      </button>
      <MessageList
        agent={agent}
        onSuggestion={onSuggestion}
        scrollerRef={scrollerRef}
        sending={sending}
      />
    </>
  );
}

describe('MessageList memoization', () => {
  it('skips Markdown work for draft updates but rerenders for sending and messages', () => {
    markdownRenderProbe.mockClear();
    render(<DraftHarness />);

    expect(markdownRenderProbe).toHaveBeenCalledTimes(1);

    fireEvent.change(screen.getByRole('textbox', { name: 'Draft' }), {
      target: { value: 'A draft update' },
    });

    expect(markdownRenderProbe).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole('button', { name: 'Start sending' }));
    expect(markdownRenderProbe).toHaveBeenCalledTimes(2);

    fireEvent.click(screen.getByRole('button', { name: 'Update messages' }));
    expect(markdownRenderProbe).toHaveBeenCalledTimes(4);
    expect(screen.getByText('Updated response')).toBeVisible();
  });
});
