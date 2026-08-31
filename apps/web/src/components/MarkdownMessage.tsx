import { Component, type ReactNode, useEffect, useRef, useState } from 'react';
import { Highlight, Prism, themes } from 'prism-react-renderer';
import ReactMarkdown, {
  defaultUrlTransform,
  type Components,
} from 'react-markdown';
import remarkGfm from 'remark-gfm';

function isAbsoluteWebUrl(href: string | undefined): boolean {
  return href !== undefined && /^https?:\/\//i.test(href);
}

function sourceText(children: ReactNode): string {
  return Array.isArray(children)
    ? children.map(sourceText).join('')
    : String(children);
}

function PlainCode({ source }: { source: string }) {
  return (
    <code className="block font-mono text-xs leading-5 text-ink">{source}</code>
  );
}

class HighlightFallback extends Component<
  { children: ReactNode; source: string },
  { failed: boolean }
> {
  override state = { failed: false };

  static getDerivedStateFromError() {
    return { failed: true };
  }

  override render() {
    return this.state.failed ? (
      <PlainCode source={this.props.source} />
    ) : (
      this.props.children
    );
  }
}

function CodeBlock({
  className,
  children,
}: {
  className?: string;
  children: ReactNode;
}) {
  const source = sourceText(children);
  const language = className?.match(/language-([\w-]+)/)?.[1] ?? 'text';
  const [copyState, setCopyState] = useState<'idle' | 'copied' | 'failed'>(
    'idle',
  );
  const resetTimer = useRef<ReturnType<typeof setTimeout> | undefined>(
    undefined,
  );
  const mounted = useRef(true);
  const copyOperation = useRef(0);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      copyOperation.current += 1;
      clearTimeout(resetTimer.current);
    };
  }, []);

  useEffect(() => {
    copyOperation.current += 1;
    clearTimeout(resetTimer.current);
    setCopyState('idle');
  }, [language, source]);

  const copy = async () => {
    const operation = ++copyOperation.current;
    let nextState: 'copied' | 'failed';
    try {
      if (!navigator.clipboard?.writeText)
        throw new Error('Clipboard unavailable');
      await navigator.clipboard.writeText(source);
      nextState = 'copied';
    } catch {
      nextState = 'failed';
    }
    if (!mounted.current || operation !== copyOperation.current) return;
    setCopyState(nextState);
    clearTimeout(resetTimer.current);
    resetTimer.current = setTimeout(() => setCopyState('idle'), 2_000);
  };

  const highlighted = Prism.languages[language] ? (
    <Highlight code={source} language={language} theme={themes.vsDark}>
      {({ getLineProps, getTokenProps, tokens }) => (
        <code className="block font-mono text-xs leading-5">
          {tokens.map((line, index) => (
            <span key={index} {...getLineProps({ line })} className="block">
              {line.map((token, tokenIndex) => (
                <span key={tokenIndex} {...getTokenProps({ token })} />
              ))}
            </span>
          ))}
        </code>
      )}
    </Highlight>
  ) : (
    <PlainCode source={source} />
  );

  return (
    <div className="mt-2 overflow-hidden rounded-md border border-line bg-panel-2 text-ink">
      <div className="flex items-center justify-between gap-3 border-b border-line px-3 py-2">
        <span className="font-mono text-[11px] text-ink-3">{language}</span>
        <button
          className="rounded px-2 py-1 text-xs text-ink-2 hover:bg-panel hover:text-ink"
          onClick={copy}
          type="button"
        >
          {copyState === 'copied'
            ? 'Copied'
            : copyState === 'failed'
              ? 'Copy failed'
              : 'Copy'}
        </button>
        <span
          aria-atomic="true"
          aria-live="polite"
          className="sr-only"
          role="status"
        >
          {copyState === 'idle'
            ? ''
            : copyState === 'copied'
              ? 'Copied'
              : 'Copy failed'}
        </span>
      </div>
      <pre>
        <span
          className="block max-w-full overflow-x-auto p-3"
          data-markdown-overflow="code"
        >
          <HighlightFallback key={`${language}:${source}`} source={source}>
            {highlighted}
          </HighlightFallback>
        </span>
      </pre>
    </div>
  );
}

const components: Components = {
  h1: ({ children }) => (
    <h1 className="mt-3 text-base font-semibold text-ink first:mt-0">
      {children}
    </h1>
  ),
  h2: ({ children }) => (
    <h2 className="mt-3 text-sm font-semibold text-ink">{children}</h2>
  ),
  h3: ({ children }) => (
    <h3 className="mt-3 text-sm font-medium text-ink">{children}</h3>
  ),
  p: ({ children }) => <p className="mt-2 first:mt-0">{children}</p>,
  ul: ({ children, className, node: _node, ...props }) => {
    const taskList = className?.includes('contains-task-list');
    return (
      <ul
        {...props}
        className={`mt-2 space-y-1 ${taskList ? 'list-none pl-0' : 'list-disc pl-5'} ${className ?? ''}`}
      >
        {children}
      </ul>
    );
  },
  ol: ({ children, className, node: _node, ...props }) => (
    <ol
      {...props}
      className={`mt-2 list-decimal space-y-1 pl-5 ${className ?? ''}`}
    >
      {children}
    </ol>
  ),
  li: ({ children, className, node: _node, ...props }) => (
    <li {...props} className={`pl-1 ${className ?? ''}`}>
      {children}
    </li>
  ),
  blockquote: ({ children }) => (
    <blockquote className="mt-2 border-l-2 border-line-strong pl-3 text-ink-2">
      {children}
    </blockquote>
  ),
  a: ({ children, href, className, node: _node, ...props }) => (
    <a
      {...props}
      className={`break-words text-accent underline underline-offset-2 ${className ?? ''}`}
      href={href}
      {...(isAbsoluteWebUrl(href)
        ? { target: '_blank', rel: 'noopener noreferrer' }
        : {})}
    >
      {children}
    </a>
  ),
  img: ({ alt }) => (
    <span className="text-ink-3">
      {alt?.trim() ? `[Image: ${alt}]` : '[Image]'}
    </span>
  ),
  table: ({ children }) => (
    <div
      className="mt-2 max-w-full overflow-x-auto rounded-md border border-line"
      data-markdown-overflow="table"
    >
      <table className="w-full min-w-max border-collapse text-left text-xs">
        {children}
      </table>
    </div>
  ),
  thead: ({ children, className, node: _node, ...props }) => (
    <thead {...props} className={`bg-panel-2 ${className ?? ''}`}>
      {children}
    </thead>
  ),
  th: ({ children, className, node, style, ...props }) => {
    const textAlign =
      node?.properties.align === 'left'
        ? 'left'
        : node?.properties.align === 'center'
          ? 'center'
          : node?.properties.align === 'right'
            ? 'right'
            : undefined;
    return (
      <th
        {...props}
        style={{ ...style, ...(textAlign ? { textAlign } : {}) }}
        className={`border-b border-line px-3 py-2 font-semibold ${className ?? ''}`}
      >
        {children}
      </th>
    );
  },
  td: ({ children, className, node, style, ...props }) => {
    const textAlign =
      node?.properties.align === 'left'
        ? 'left'
        : node?.properties.align === 'center'
          ? 'center'
          : node?.properties.align === 'right'
            ? 'right'
            : undefined;
    return (
      <td
        {...props}
        style={{ ...style, ...(textAlign ? { textAlign } : {}) }}
        className={`border-b border-line px-3 py-2 align-top ${className ?? ''}`}
      >
        {children}
      </td>
    );
  },
  hr: () => <hr className="my-3 border-line" />,
  strong: ({ children }) => (
    <strong className="font-semibold text-ink">{children}</strong>
  ),
  del: ({ children }) => <del className="text-ink-3">{children}</del>,
  pre: ({ children }) => <>{children}</>,
  code: ({ children, className }) => {
    const source = sourceText(children);
    const fenced = className?.includes('language-') || source.includes('\n');
    return fenced ? (
      <CodeBlock className={className} children={children} />
    ) : (
      <code className="break-all rounded bg-panel-2 px-1 py-0.5 font-mono text-[0.9em]">
        {children}
      </code>
    );
  },
};

export function MarkdownMessage({ children }: { children: string }) {
  return (
    <div
      className="min-w-0 break-words text-sm leading-6 text-ink"
      data-testid="markdown-message"
    >
      <ReactMarkdown
        components={components}
        remarkPlugins={[remarkGfm]}
        urlTransform={defaultUrlTransform}
      >
        {children}
      </ReactMarkdown>
    </div>
  );
}
