import { fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import type { AgentDetail } from '../lib/types';
import { formatTime } from './ui-bits';
import { Composer, MessageList } from './ChatScreen';
import { SLASH_COMMANDS } from '../lib/slash-commands';

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
  it('renders Markdown bubbles, literal event pills, and tool results as cards', async () => {
    const user = userEvent.setup();
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
    expect(screen.getByText('system · **system marker**').tagName).toBe('SPAN');
    // Tool messages are no longer grey pills (spec §15.1).
    expect(screen.queryByText(/^tool · /)).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Used 1 tool · <1s' }));
    await user.click(screen.getByRole('button', { name: /^tool\b/ }));
    expect(screen.getByText('## tool marker').tagName).toBe('PRE');
    expect(
      screen.queryByRole('heading', { name: 'tool marker' }),
    ).not.toBeInTheDocument();
  });

  it('labels a stopped reply', () => {
    render(
      <MessageList
        agent={{
          ...agent,
          messages: [
            {
              id: 'stopped',
              role: 'Assistant',
              content: { text: 'Half an answer', metadata: { stopped: true } },
              created_at_ms: 1_725_000_000_000,
            },
          ],
        }}
        sending={false}
        scrollerRef={{ current: null }}
        onSuggestion={vi.fn()}
      />,
    );

    expect(screen.getByText('Stopped')).toBeVisible();
  });

  it('labels an incomplete reply', () => {
    render(
      <MessageList
        agent={{
          ...agent,
          messages: [
            {
              id: 'incomplete',
              role: 'Assistant',
              content: {
                text: 'Half an answer',
                metadata: { incomplete: true },
              },
              created_at_ms: 1_725_000_000_000,
            },
          ],
        }}
        sending={false}
        scrollerRef={{ current: null }}
        onSuggestion={vi.fn()}
      />,
    );

    expect(screen.getByText('Incomplete')).toBeVisible();
  });

  it('anchors a message id shared by a bubble and its tool block to the bubble alone', async () => {
    // An assistant message with both text and tool calls produces two
    // transcript items that both cite its id; only one may own it, or
    // jump/highlight are ambiguous and unmounting either item deletes the
    // other's registration.
    const calledOn: Element[] = [];
    const original = Element.prototype.scrollIntoView;
    Element.prototype.scrollIntoView = function (this: Element) {
      calledOn.push(this);
    };
    try {
      const user = userEvent.setup();
      render(
        <MessageList
          agent={{
            ...agent,
            messages: [
              {
                id: 'a1',
                role: 'Assistant',
                content: {
                  text: 'Let me check.',
                  metadata: {
                    toolCalls: [{ id: 'call_1', name: 'calculate', args: {} }],
                  },
                },
                created_at_ms: 1_725_000_000_000,
              },
            ],
          }}
          sending={false}
          scrollerRef={{ current: null }}
          onSuggestion={vi.fn()}
        />,
      );

      const bubbleAnchor = screen
        .getByText('Let me check.')
        .closest('.studio-message-anchor');
      const toolAnchor = screen
        .getByRole('button', { name: /Used 1 tool/ })
        .closest('.studio-message-anchor');
      expect(bubbleAnchor).not.toBeNull();
      expect(bubbleAnchor).not.toBe(toolAnchor);

      await user.click(
        screen.getByRole('button', { name: 'Search conversation' }),
      );
      await user.type(
        screen.getByRole('searchbox', { name: 'Search messages' }),
        'Let me check',
      );
      expect(bubbleAnchor).toHaveAttribute('data-search-match', 'true');
      expect(toolAnchor).not.toHaveAttribute('data-search-match');

      fireEvent.keyDown(
        screen.getByRole('searchbox', { name: 'Search messages' }),
        { key: 'Enter' },
      );
      expect(calledOn).toContain(bubbleAnchor);
      expect(calledOn).not.toContain(toolAnchor);
    } finally {
      Element.prototype.scrollIntoView = original;
    }
  });

  it('renders the context-trimmed divider without nesting its Compact button inside the separator', () => {
    const onCompact = vi.fn();
    render(
      <MessageList
        agent={agent}
        sending={false}
        scrollerRef={{ current: null }}
        onSuggestion={vi.fn()}
        items={[{ kind: 'trimmed', key: 'trimmed' }]}
        actions={{ onCompact }}
      />,
    );

    const separator = screen.getByRole('separator');
    const button = screen.getByRole('button', { name: 'Compact' });
    expect(separator.contains(button)).toBe(false);
  });

  it('renders a helper session’s delegated turn from the delegating companion, not the owner', () => {
    render(
      <MessageList
        agent={agent}
        sending={false}
        scrollerRef={{ current: null }}
        onSuggestion={vi.fn()}
        items={[
          {
            kind: 'delegated',
            key: 'delegated-1',
            from: 'Nova',
            message: {
              id: 'delegated-1',
              role: 'User',
              content: { text: 'Compare vendors' },
              created_at_ms: 1_725_000_000_000,
            },
          },
        ]}
      />,
    );

    expect(screen.getByText('From Nova')).toBeVisible();
    expect(screen.getByText('Compare vendors')).toBeVisible();
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

describe('Composer commands and live replies', () => {
  function composerProps(
    overrides: Partial<Parameters<typeof Composer>[0]> = {},
  ) {
    return {
      agentName: 'Nova',
      draft: '',
      setDraft: vi.fn(),
      sending: false,
      disabled: false,
      onSend: vi.fn(),
      error: null,
      onDismissError: vi.fn(),
      commands: SLASH_COMMANDS,
      ...overrides,
    };
  }

  it('offers the matching commands and runs one with Enter', () => {
    const props = composerProps({ draft: '/co' });
    render(<Composer {...props} />);

    const menu = screen.getByRole('listbox', { name: 'Commands' });
    expect(within(menu).getAllByRole('option')).toHaveLength(1);
    expect(
      within(menu).getByRole('option', { selected: true }),
    ).toHaveTextContent('/compact');
    fireEvent.keyDown(screen.getByRole('textbox', { name: 'Message Nova' }), {
      key: 'Enter',
    });
    expect(props.onSend).toHaveBeenCalledWith('/compact');
  });

  it('completes a command that needs more text instead of running it', () => {
    const props = composerProps({ draft: '/re' });
    const view = render(<Composer {...props} />);
    const input = screen.getByRole('textbox', { name: 'Message Nova' });

    fireEvent.keyDown(input, { key: 'Enter' });
    expect(props.setDraft).toHaveBeenCalledWith('/rename ');
    expect(props.onSend).not.toHaveBeenCalled();

    view.rerender(<Composer {...props} draft="/n" />);
    fireEvent.keyDown(input, { key: 'Tab' });
    expect(props.setDraft).toHaveBeenLastCalledWith('/new');
  });

  it('moves through commands with the arrow keys, picks by click, and closes with Escape', async () => {
    const props = composerProps({ draft: '/' });
    render(<Composer {...props} />);
    const input = screen.getByRole('textbox', { name: 'Message Nova' });

    fireEvent.keyDown(input, { key: 'ArrowDown' });
    expect(screen.getByRole('option', { selected: true })).toHaveTextContent(
      '/stop',
    );
    expect(input).toHaveAttribute(
      'aria-activedescendant',
      screen.getByRole('option', { selected: true }).id,
    );
    await userEvent.click(screen.getByRole('option', { name: /\/export/ }));
    expect(props.onSend).toHaveBeenCalledWith('/export');
    fireEvent.keyDown(input, { key: 'Escape' });
    expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
  });

  it('shows no command menu without commands', () => {
    render(
      <Composer {...composerProps({ draft: '/', commands: undefined })} />,
    );
    expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
  });

  it('turns Send into Stop and steers with Ctrl+Enter while a reply runs', async () => {
    const props = composerProps({
      draft: 'also check flights',
      runActive: true,
      onStop: vi.fn(),
      onSteer: vi.fn(),
    });
    render(<Composer {...props} />);
    const input = screen.getByRole('textbox', { name: 'Message Nova' });

    expect(
      screen.queryByRole('button', { name: 'Send' }),
    ).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Stop' }));
    expect(props.onStop).toHaveBeenCalled();
    fireEvent.keyDown(input, { key: 'Enter', ctrlKey: true });
    expect(props.onSteer).toHaveBeenCalledTimes(1);
    expect(props.onSend).not.toHaveBeenCalled();
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(props.onSend).toHaveBeenCalledTimes(1);
    expect(screen.getByText('⏎ queue · ⌘⏎ steer · ⇧⏎ new line')).toBeVisible();
  });

  it('does not steer or send while composing IME text', () => {
    const props = composerProps({
      draft: 'more context',
      commands: undefined,
      runActive: true,
      onStop: vi.fn(),
      onSteer: vi.fn(),
    });
    render(<Composer {...props} />);
    const input = screen.getByRole('textbox', { name: 'Message Nova' });

    fireEvent.keyDown(input, {
      key: 'Enter',
      ctrlKey: true,
      isComposing: true,
    });
    expect(props.onSteer).not.toHaveBeenCalled();
    expect(props.onSend).not.toHaveBeenCalled();
    fireEvent.keyDown(input, { key: 'Enter', isComposing: true });
    expect(props.onSend).not.toHaveBeenCalled();
  });

  it('wires the input to the open command menu as a combobox', () => {
    const props = composerProps({ draft: '/co' });
    render(<Composer {...props} />);
    const input = screen.getByRole('textbox', { name: 'Message Nova' });
    const listbox = screen.getByRole('listbox');
    const option = screen.getByRole('option', { selected: true });

    expect(input).toHaveAttribute('aria-expanded', 'true');
    expect(input).toHaveAttribute('aria-controls', listbox.id);
    expect(input).toHaveAttribute('aria-activedescendant', option.id);
    expect(document.activeElement).not.toBe(listbox);
  });

  it('closes the combobox wiring once no command matches', () => {
    const props = composerProps({ draft: 'hello' });
    render(<Composer {...props} />);
    const input = screen.getByRole('textbox', { name: 'Message Nova' });

    expect(input).toHaveAttribute('aria-expanded', 'false');
    expect(input).not.toHaveAttribute('aria-controls');
    expect(input).not.toHaveAttribute('aria-activedescendant');
  });
});
