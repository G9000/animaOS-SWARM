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
  return <code className="block font-mono text-xs leading-5">{source}</code>;
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

  useEffect(() => () => clearTimeout(resetTimer.current), []);

  const copy = async () => {
    try {
      if (!navigator.clipboard?.writeText)
        throw new Error('Clipboard unavailable');
      await navigator.clipboard.writeText(source);
      setCopyState('copied');
    } catch {
      setCopyState('failed');
    }
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
    <section className="mt-2 overflow-hidden rounded-md border border-slate-700 bg-slate-950 text-slate-100">
      <header className="flex items-center justify-between gap-3 border-b border-slate-800 px-3 py-2">
        <span className="font-mono text-[11px] text-slate-400">{language}</span>
        <button
          className="rounded px-2 py-1 text-xs text-slate-300 hover:bg-slate-800 hover:text-white"
          onClick={copy}
          type="button"
        >
          {copyState === 'copied'
            ? 'Copied'
            : copyState === 'failed'
              ? 'Copy failed'
              : 'Copy'}
        </button>
      </header>
      <pre>
        <span
          className="block overflow-x-auto p-3"
          data-markdown-overflow="code"
        >
          <HighlightFallback key={`${language}:${source}`} source={source}>
            {highlighted}
          </HighlightFallback>
        </span>
      </pre>
    </section>
  );
}

const components: Components = {
  h1: ({ children }) => (
    <h1 className="mt-3 text-base font-semibold text-slate-100 first:mt-0">
      {children}
    </h1>
  ),
  h2: ({ children }) => (
    <h2 className="mt-3 text-sm font-semibold text-slate-100">{children}</h2>
  ),
  h3: ({ children }) => (
    <h3 className="mt-3 text-sm font-medium text-slate-100">{children}</h3>
  ),
  p: ({ children }) => <p className="mt-2 first:mt-0">{children}</p>,
  ul: ({ children }) => (
    <ul className="mt-2 list-disc space-y-1 pl-5">{children}</ul>
  ),
  ol: ({ children }) => (
    <ol className="mt-2 list-decimal space-y-1 pl-5">{children}</ol>
  ),
  li: ({ children }) => <li className="pl-1">{children}</li>,
  blockquote: ({ children }) => (
    <blockquote className="mt-2 border-l-2 border-slate-500 pl-3 text-slate-300">
      {children}
    </blockquote>
  ),
  a: ({ children, href }) => (
    <a
      className="break-words text-sky-300 underline underline-offset-2"
      href={href}
      {...(isAbsoluteWebUrl(href)
        ? { target: '_blank', rel: 'noopener noreferrer' }
        : {})}
    >
      {children}
    </a>
  ),
  table: ({ children }) => (
    <div
      className="mt-2 max-w-full overflow-x-auto rounded-md border border-slate-700"
      data-markdown-overflow="table"
    >
      <table className="w-full min-w-max border-collapse text-left text-xs">
        {children}
      </table>
    </div>
  ),
  thead: ({ children }) => (
    <thead className="bg-slate-800/70">{children}</thead>
  ),
  th: ({ children }) => (
    <th className="border-b border-slate-700 px-3 py-2 font-semibold">
      {children}
    </th>
  ),
  td: ({ children }) => (
    <td className="border-b border-slate-800 px-3 py-2 align-top">
      {children}
    </td>
  ),
  hr: () => <hr className="my-3 border-slate-700" />,
  strong: ({ children }) => (
    <strong className="font-semibold text-slate-100">{children}</strong>
  ),
  del: ({ children }) => <del className="text-slate-400">{children}</del>,
  pre: ({ children }) => <>{children}</>,
  code: ({ children, className }) => {
    const source = sourceText(children);
    const fenced = className?.includes('language-') || source.includes('\n');
    return fenced ? (
      <CodeBlock className={className} children={children} />
    ) : (
      <code className="break-all rounded bg-slate-800 px-1 py-0.5 font-mono text-[0.9em]">
        {children}
      </code>
    );
  },
};

export function MarkdownMessage({ children }: { children: string }) {
  return (
    <div
      className="min-w-0 break-words text-sm leading-6 text-slate-200"
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
