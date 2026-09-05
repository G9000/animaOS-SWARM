import {
  memo,
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from 'react';
import { ConversationTools } from './ConversationTools';
import { CopyMessage } from './CopyMessage';
import type { AgentDetail, ChatMessage } from '../lib/types';
import { AlertIcon, BoltIcon, PulseIcon, SendIcon } from './icons';
import { MarkdownMessage } from './MarkdownMessage';
import { ErrorBanner, formatTime } from './ui-bits';

/* ── Messages ── */
function EventPill({ message }: { message: ChatMessage }) {
  const text = message.content.text;
  return (
    <div className="animate-fade-in flex justify-center">
      <span
        className="max-w-md truncate rounded-full border border-line bg-white/[0.02] px-3 py-1 font-mono text-[10px] text-ink-3"
        title={text}
      >
        {message.role.toLowerCase()} · {text}
      </span>
    </div>
  );
}

function Bubble({ message }: { message: ChatMessage }) {
  if (message.role !== 'User' && message.role !== 'Assistant') {
    return <EventPill message={message} />;
  }
  const isUser = message.role === 'User';
  return (
    <div
      className={`animate-msg-in flex ${isUser ? 'justify-end' : 'justify-start'}`}
    >
      <div
        className={`flex min-w-0 max-w-[85%] flex-col ${isUser ? 'items-end' : 'items-start'}`}
      >
        <div
          className={`min-w-0 max-w-full break-words px-4 py-2.5 text-sm leading-relaxed ${
            isUser
              ? 'rounded-2xl rounded-br-md border border-line-strong bg-panel-2/90 text-ink shadow-lg shadow-black/25'
              : 'glass rounded-2xl rounded-bl-md text-ink'
          }`}
        >
          <MarkdownMessage>{message.content.text}</MarkdownMessage>
        </div>
        <div className="studio-message-meta mt-1 px-1 font-mono text-[10px] text-ink-3">
          <span>{formatTime(message.created_at_ms)}</span>
          <CopyMessage text={message.content.text} />
        </div>
      </div>
    </div>
  );
}

const SUGGESTIONS = [
  {
    icon: <Sparkle />,
    text: 'What can you do for me?',
    title: 'Explore the possibilities',
    label: 'DISCOVER',
    detail: 'Meet your new thinking partner.',
  },
  {
    icon: <BoltIcon size={13} />,
    text: 'Help me plan my day',
    title: 'Make room for what matters',
    label: 'MAKE A PLAN',
    detail: 'Turn a busy day into a clear one.',
  },
  {
    icon: <PulseIcon size={13} />,
    text: 'Check in on me every hour',
    title: 'Keep the momentum',
    label: 'STAY IN SYNC',
    detail: 'A gentle nudge, right on time.',
  },
];

function Sparkle() {
  return <BoltIcon size={13} />;
}

function EmptyState({
  agentName,
  onPick,
}: {
  agentName: string;
  onPick: (text: string) => void;
}) {
  return (
    <div className="studio-welcome animate-rise-in">
      <div className="studio-hero">
        <div className="studio-hero-copy">
          <p className="studio-eyebrow">
            <span aria-hidden /> A SPACE FOR YOUR NEXT BIG THING
          </p>
          <h2
            aria-label={`Say something to ${agentName}`}
            className="studio-hero-title"
          >
            Less busy.
            <br />
            <span>More possibility.</span>
          </h2>
          <p className="studio-hero-description">
            Think out loud. Find your focus. Make something happen.
            <br className="hidden lg:block" /> {agentName} is here to help you
            move things forward.
          </p>
          <div className="studio-hero-signature">
            <span aria-hidden>↗</span> Human ambition. A little extra
            intelligence.
          </div>
        </div>
        <div className="studio-sculpture" aria-hidden data-motion="agent-orb">
          <div className="studio-orbit orbit-one" />
          <div className="studio-orbit orbit-two" />
          <div className="studio-orbit orbit-three" />
          <div className="studio-sphere">
            <span>✳</span>
          </div>
          <span className="studio-sculpture-label">POSSIBILITY, IN ORBIT</span>
          <span className="studio-satellite satellite-one" />
          <span className="studio-satellite satellite-two" />
        </div>
      </div>
      <div className="studio-section-rule">
        <span>A FEW PLACES TO START</span>
        <span aria-hidden>01 — 03</span>
      </div>
      <div className="studio-suggestions">
        {SUGGESTIONS.map((s) => (
          <button
            key={s.text}
            aria-label={`${s.title}. ${s.text}`}
            onClick={() => onPick(s.text)}
            className="studio-suggestion group text-left"
          >
            <span className="studio-suggestion-top">
              <span className="studio-suggestion-icon">{s.icon}</span>
              <span>{s.label}</span>
              <span className="studio-suggestion-arrow" aria-hidden>
                ↗
              </span>
            </span>
            <strong>{s.title}</strong>
            <span className="studio-suggestion-detail">{s.detail}</span>
          </button>
        ))}
      </div>
    </div>
  );
}

function ThinkingIndicator({ name }: { name: string }) {
  return (
    <div className="animate-fade-in flex items-center gap-3 px-1 py-1">
      <div className="flex items-center gap-1.5">
        {[0, 1, 2].map((i) => (
          <span
            key={i}
            className="typing-dot h-1.5 w-1.5 rounded-full bg-accent"
            style={{ animationDelay: `${i * 150}ms` }}
          />
        ))}
      </div>
      <span className="text-shimmer font-mono text-[11px]">
        {name} is thinking
      </span>
    </div>
  );
}

export const MessageList = memo(function MessageList({
  agent,
  sending,
  scrollerRef,
  onSuggestion,
}: {
  agent: AgentDetail;
  sending: boolean;
  scrollerRef: React.RefObject<HTMLDivElement | null>;
  onSuggestion: (text: string) => void;
}) {
  const [awayFromBottom, setAwayFromBottom] = useState(false);
  const [highlight, setHighlight] = useState<string | null>(null);
  const atBottom = useRef(true);
  const messageElements = useRef(new Map<string, HTMLDivElement>());
  const jumpToMessage = useCallback((id: string) => {
    atBottom.current = false;
    setAwayFromBottom(true);
    messageElements.current.get(id)?.scrollIntoView?.({ block: 'center' });
  }, []);
  const jumpToLatest = () => {
    const element = scrollerRef.current;
    if (element) element.scrollTop = element.scrollHeight;
    atBottom.current = true;
    setAwayFromBottom(false);
  };
  useLayoutEffect(() => {
    if (atBottom.current) {
      const element = scrollerRef.current;
      if (element) element.scrollTop = element.scrollHeight;
    }
  }, [agent.messages, sending, scrollerRef]);

  return (
    <>
      {agent.messages.length > 0 && (
        <ConversationTools
          agent={agent}
          onJump={jumpToMessage}
          onHighlight={setHighlight}
        />
      )}
      <div className="studio-conversation-body">
        <div
          ref={scrollerRef}
          className="studio-message-scroller relative z-[1] min-h-0 flex-1 overflow-y-auto"
          aria-label={`Conversation with ${agent.name}`}
          onScroll={(event) => {
            const element = event.currentTarget;
            atBottom.current =
              element.scrollHeight - element.scrollTop - element.clientHeight <
              80;
            setAwayFromBottom(!atBottom.current);
          }}
        >
          {agent.messages.length === 0 && !sending ? (
            <EmptyState agentName={agent.name} onPick={onSuggestion} />
          ) : (
            <div className="studio-messages mx-auto flex w-full max-w-3xl flex-col gap-4 px-4 py-6 sm:px-6">
              {agent.messages.map((m) => (
                <div
                  key={m.id}
                  ref={(element) => {
                    if (element) messageElements.current.set(m.id, element);
                    else messageElements.current.delete(m.id);
                  }}
                  data-search-match={highlight === m.id || undefined}
                  className="studio-message-anchor"
                >
                  <Bubble message={m} />
                </div>
              ))}
              {sending && <ThinkingIndicator name={agent.name} />}
            </div>
          )}
        </div>
        {awayFromBottom && (
          <button
            type="button"
            className="studio-jump-latest"
            onClick={jumpToLatest}
          >
            ↓ Jump to latest
          </button>
        )}
      </div>
    </>
  );
});

/* ── Composer ── */
export function Composer({
  agentName,
  draft,
  setDraft,
  sending,
  disabled,
  onSend,
  error,
  onDismissError,
  offline = false,
  recovery,
}: {
  agentName: string;
  draft: string;
  setDraft: (v: string) => void;
  sending: boolean;
  disabled: boolean;
  onSend: () => void;
  error: string | null;
  onDismissError: () => void;
  offline?: boolean;
  recovery?: {
    count: number;
    text: string;
    restore: () => void;
    dismiss: () => void;
  };
}) {
  const taRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    const el = taRef.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(el.scrollHeight, 192)}px`;
  }, [draft]);

  return (
    <div className="studio-composer safe-composer sticky bottom-0 z-10 bg-gradient-to-t from-abyss via-abyss/95 to-transparent px-4 pt-3 sm:px-6">
      <div className="mx-auto w-full max-w-3xl">
        {recovery && (
          <div className="studio-draft-recovery">
            <div>
              <p>
                {recovery.count} recoverable{' '}
                {recovery.count === 1 ? 'message' : 'messages'}. Check the
                conversation before retrying—it may have reached the daemon.
              </p>
              <blockquote>{recovery.text}</blockquote>
            </div>
            <div>
              <button
                type="button"
                className="studio-tool-button"
                onClick={() => {
                  recovery.restore();
                  taRef.current?.focus();
                }}
              >
                Restore message
              </button>
              <button
                type="button"
                className="studio-tool-button"
                onClick={recovery.dismiss}
                aria-label="Dismiss recoverable message"
              >
                ×
              </button>
            </div>
          </div>
        )}
        {error && (
          <div className="mb-2.5">
            <ErrorBanner
              message={error}
              onDismiss={onDismissError}
              icon={<AlertIcon size={14} />}
            />
          </div>
        )}
        <div className="studio-composer-box glass-strong focus-glow flex items-end gap-2 rounded-2xl p-2 transition-all duration-200">
          <textarea
            data-workspace-composer
            ref={taRef}
            value={draft}
            disabled={disabled}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (
                e.key === 'Enter' &&
                !e.shiftKey &&
                !e.nativeEvent.isComposing &&
                e.keyCode !== 229
              ) {
                e.preventDefault();
                if (!disabled && !sending && !offline && draft.trim()) onSend();
              }
            }}
            rows={1}
            aria-label={`Message ${agentName}`}
            placeholder={`Message ${agentName}…`}
            className="max-h-48 flex-1 resize-none bg-transparent px-3 py-2 text-sm leading-relaxed text-ink placeholder-ink-3 outline-none"
          />
          <button
            onClick={onSend}
            disabled={disabled || sending || offline || !draft.trim()}
            aria-label="Send"
            className="flex h-9 w-9 shrink-0 cursor-pointer items-center justify-center rounded-xl bg-accent text-abyss shadow-lg shadow-accent/25 transition hover:bg-accent/90 active:scale-95 disabled:cursor-not-allowed disabled:opacity-25 disabled:shadow-none disabled:active:scale-100"
          >
            <SendIcon size={15} />
          </button>
        </div>
        <div className="mt-2 flex items-center justify-between px-2 font-mono text-[10px] text-ink-3">
          <span>⏎ send · ⇧⏎ new line</span>
          <span>
            {offline
              ? 'Offline · your draft stays here'
              : sending
                ? 'Working on your message…'
                : 'Your space. Your pace.'}
          </span>
        </div>
      </div>
    </div>
  );
}
