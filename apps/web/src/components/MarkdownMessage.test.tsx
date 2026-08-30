import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { MarkdownMessage } from './MarkdownMessage';

describe('MarkdownMessage', () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it('renders safe GitHub-flavored Markdown semantics', () => {
    render(
      <MarkdownMessage>
        {
          '# Heading\n\n**bold** and ~~removed~~\n\n- one\n- [x] done\n\n1. first\n2. second\n\n> quoted\n\n| Name | Value |\n| --- | --- |\n| alpha | beta |'
        }
      </MarkdownMessage>,
    );

    expect(screen.getByTestId('markdown-message')).toBeVisible();
    expect(screen.getByRole('heading', { name: 'Heading' })).toBeVisible();
    expect(screen.getByText('bold').tagName).toBe('STRONG');
    expect(screen.getByText('removed').tagName).toBe('DEL');
    expect(screen.getAllByRole('list')).toHaveLength(2);
    expect(screen.getByRole('checkbox')).toBeChecked();
    expect(screen.getByRole('blockquote')).toHaveTextContent('quoted');
    expect(screen.getByRole('table')).toHaveTextContent('alpha');
  });

  it('keeps raw HTML inert and unsafe link protocols non-executable', () => {
    render(
      <MarkdownMessage>
        {
          '<script data-testid="raw-script">alert(1)</script>\n\n[unsafe](javascript:alert(1))'
        }
      </MarkdownMessage>,
    );

    expect(screen.getByText(/alert\(1\)/)).toBeVisible();
    expect(screen.queryByTestId('raw-script')).not.toBeInTheDocument();
    const unsafe = screen.getByText('unsafe');
    expect(unsafe).not.toHaveAttribute('href', 'javascript:alert(1)');
  });

  it('opens absolute web links safely and retains relative links in the current tab', () => {
    render(
      <MarkdownMessage>
        {'[docs](https://example.com/docs) [settings](/settings)'}
      </MarkdownMessage>,
    );

    expect(screen.getByRole('link', { name: 'docs' })).toHaveAttribute(
      'target',
      '_blank',
    );
    expect(screen.getByRole('link', { name: 'docs' })).toHaveAttribute(
      'rel',
      'noopener noreferrer',
    );
    expect(screen.getByRole('link', { name: 'settings' })).toHaveAttribute(
      'href',
      '/settings',
    );
    expect(screen.getByRole('link', { name: 'settings' })).not.toHaveAttribute(
      'target',
    );
  });

  it('contains wide tables and wraps long inline code', () => {
    render(
      <MarkdownMessage>
        {
          '`a-very-long-inline-code-value`\n\n| A | B |\n| --- | --- |\n| 1 | 2 |'
        }
      </MarkdownMessage>,
    );

    expect(screen.getByRole('table').parentElement).toHaveAttribute(
      'data-markdown-overflow',
      'table',
    );
    expect(screen.getByText('a-very-long-inline-code-value')).toHaveClass(
      'break-all',
    );
  });

  it('keeps inline code free of a copy control', () => {
    render(<MarkdownMessage>{'Use `inlineValue` here.'}</MarkdownMessage>);

    expect(screen.getByText('inlineValue')).toBeVisible();
    expect(
      screen.queryByRole('button', { name: /copy/i }),
    ).not.toBeInTheDocument();
  });

  it('labels fenced TypeScript and copies its exact source', async () => {
    const writeText = vi.fn(async () => undefined);
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText },
    });
    render(
      <MarkdownMessage>
        {'```typescript\nconst answer = 42;\n```'}
      </MarkdownMessage>,
    );

    expect(screen.getByText('typescript')).toBeVisible();
    const copy = screen.getByRole('button', { name: 'Copy' });
    fireEvent.click(copy);

    expect(writeText).toHaveBeenCalledWith('const answer = 42;\n');
    expect(await screen.findByRole('button', { name: 'Copied' })).toBeVisible();
    expect(
      screen.getByTestId('markdown-message').querySelectorAll('pre'),
    ).toHaveLength(1);
    expect(
      screen
        .getByTestId('markdown-message')
        .querySelector('[data-markdown-overflow="code"]'),
    ).toBeVisible();
  });

  it('restores Copy after two seconds', async () => {
    vi.useFakeTimers();
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText: vi.fn(async () => undefined) },
    });
    render(
      <MarkdownMessage>{'```ts\nconst answer = 42;\n```'}</MarkdownMessage>,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Copy' }));
    await act(async () => {
      await Promise.resolve();
    });
    expect(screen.getByRole('button', { name: 'Copied' })).toBeVisible();
    act(() => vi.advanceTimersByTime(2_000));
    expect(screen.getByRole('button', { name: 'Copy' })).toBeVisible();
  });

  it('shows a failure label when Clipboard is unavailable or rejects', async () => {
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: undefined,
    });
    const { unmount } = render(
      <MarkdownMessage>{'```text\nhello\n```'}</MarkdownMessage>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Copy' }));
    expect(
      await screen.findByRole('button', { name: 'Copy failed' }),
    ).toBeVisible();
    unmount();

    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: {
        writeText: vi.fn(async () => Promise.reject(new Error('denied'))),
      },
    });
    render(<MarkdownMessage>{'```text\nhello\n```'}</MarkdownMessage>);
    fireEvent.click(screen.getByRole('button', { name: 'Copy' }));
    expect(
      await screen.findByRole('button', { name: 'Copy failed' }),
    ).toBeVisible();
  });

  it('falls back to original source for an unknown fenced language', () => {
    render(
      <MarkdownMessage>
        {'```not-a-real-language\nkeep <this> source\n```'}
      </MarkdownMessage>,
    );

    expect(screen.getByText('not-a-real-language')).toBeVisible();
    expect(screen.getByText('keep <this> source')).toBeVisible();
  });
});
