import { fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import type { AgentDetail } from '../lib/types';
import { formatTime } from './ui-bits';
import { Composer, MessageList } from './ChatScreen';

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
  it('preserves the reading position when new messages arrive and offers an explicit jump', async () => {
    const user = userEvent.setup();
    const scrollerRef = { current: null as HTMLDivElement | null };
    const view = render(
      <MessageList
        agent={agent}
        sending={false}
        scrollerRef={scrollerRef}
        onSuggestion={vi.fn()}
      />,
    );
    const scroller = screen.getByLabelText('Conversation with Nova');
    Object.defineProperties(scroller, {
      scrollHeight: { configurable: true, value: 1200 },
      clientHeight: { configurable: true, value: 400 },
    });
    scroller.scrollTop = 100;
    fireEvent.scroll(scroller);
    view.rerender(
      <MessageList
        agent={{
          ...agent,
          messages: [...messages, { ...messages[0], id: 'new-message' }],
        }}
        sending={false}
        scrollerRef={scrollerRef}
        onSuggestion={vi.fn()}
      />,
    );
    expect(scroller.scrollTop).toBe(100);
    await user.click(screen.getByRole('button', { name: '↓ Jump to latest' }));
    expect(scroller.scrollTop).toBe(1200);
    expect(
      screen.queryByRole('button', { name: '↓ Jump to latest' }),
    ).not.toBeInTheDocument();
  });
  it('searches literal conversation text and reports no matches', async () => {
    const user = userEvent.setup();
    render(
      <MessageList
        agent={agent}
        sending={false}
        scrollerRef={{ current: null }}
        onSuggestion={vi.fn()}
      />,
    );
    await user.click(
      screen.getByRole('button', { name: 'Search conversation' }),
    );
    await user.type(
      screen.getByRole('searchbox', { name: 'Search messages' }),
      'Heading',
    );
    expect(screen.getByRole('status')).toHaveTextContent('1 of 1');
    await user.clear(
      screen.getByRole('searchbox', { name: 'Search messages' }),
    );
    fireEvent.change(
      screen.getByRole('searchbox', { name: 'Search messages' }),
      { target: { value: '[missing]' } },
    );
    expect(screen.getByRole('status')).toHaveTextContent('No matches');
  });

  it('copies original message Markdown', async () => {
    const user = userEvent.setup();
    render(
      <MessageList
        agent={agent}
        sending={false}
        scrollerRef={{ current: null }}
        onSuggestion={vi.fn()}
      />,
    );
    await user.click(
      screen.getAllByRole('button', { name: 'Copy message' })[0],
    );
    expect(await navigator.clipboard.readText()).toBe('**bold**');
    expect(screen.getByRole('button', { name: 'Copied' })).toBeVisible();
  });
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
    expect(
      screen.getByRole('heading', { level: 2, name: 'Heading' }),
    ).toBeVisible();
    expect(screen.getByText('system · **system marker**')).toBeVisible();
    expect(screen.getByText('tool · ## tool marker')).toBeVisible();
    expect(screen.getByText('system · **system marker**').tagName).toBe('SPAN');
    expect(
      screen.queryByRole('heading', { name: 'tool marker' }),
    ).not.toBeInTheDocument();
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
    expect(
      screen.getByText(formatTime(messages[0].created_at_ms)),
    ).toBeVisible();
    expect(
      screen.getByText(formatTime(messages[1].created_at_ms)),
    ).toBeVisible();
  });

  it('allows rich-content bubbles to shrink within their width cap', () => {
    const scrollerRef = { current: null };
    render(
      <MessageList
        agent={agent}
        sending={false}
        scrollerRef={scrollerRef}
        onSuggestion={vi.fn()}
      />,
    );

    for (const message of screen.getAllByTestId('markdown-message')) {
      expect(message.parentElement).toHaveClass('min-w-0', 'max-w-full');
      expect(message.parentElement?.parentElement).toHaveClass(
        'max-w-[85%]',
        'min-w-0',
      );
    }
  });
});

describe('Composer keyboard safety', () => {
  it('does not send while composing IME text or while another send is pending', () => {
    const onSend = vi.fn();
    const props = {
      agentName: 'Nova',
      draft: 'Hello',
      setDraft: vi.fn(),
      sending: false,
      disabled: false,
      onSend,
      error: null,
      onDismissError: vi.fn(),
    };
    const view = render(<Composer {...props} />);
    const input = screen.getByRole('textbox', { name: 'Message Nova' });
    fireEvent.keyDown(input, { key: 'Enter', isComposing: true });
    expect(onSend).not.toHaveBeenCalled();
    view.rerender(<Composer {...props} sending />);
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(onSend).not.toHaveBeenCalled();
    view.rerender(<Composer {...props} />);
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(onSend).toHaveBeenCalledTimes(1);
  });
});
