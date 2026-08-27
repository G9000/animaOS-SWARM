import { useEffect, useRef } from 'react';
import type { AgentDetail, ChatMessage } from '../lib/types';
import { AlertIcon, BoltIcon, PulseIcon, SendIcon } from './icons';
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
        className={`flex max-w-[85%] flex-col ${isUser ? 'items-end' : 'items-start'}`}
      >
        <div
          className={`whitespace-pre-wrap break-words px-4 py-2.5 text-sm leading-relaxed ${
            isUser
              ? 'rounded-2xl rounded-br-md border border-line-strong bg-panel-2/90 text-ink shadow-lg shadow-black/25'
              : 'glass rounded-2xl rounded-bl-md text-ink'
          }`}
        >
          {message.content.text}
        </div>
        <span className="mt-1 px-1 font-mono text-[10px] text-ink-3">
          {formatTime(message.created_at_ms)}
        </span>
      </div>
    </div>
  );
}

const SUGGESTIONS = [
  { icon: <Sparkle />, text: 'What can you do for me?' },
  { icon: <BoltIcon size={13} />, text: 'Help me plan my day' },
  { icon: <PulseIcon size={13} />, text: 'Check in on me every hour' },
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
    <div className="animate-rise-in flex flex-col items-center pt-[14vh]">
      <div className="relative mb-6 flex h-20 w-20 items-center justify-center">
        <span className="animate-ripple absolute inset-0 rounded-full border border-accent/30" />
        <span
          className="animate-ripple absolute inset-0 rounded-full border border-accent/20"
          style={{ animationDelay: '2.6s' }}
        />
        <div
          data-motion="agent-orb"
          className="agent-orb animate-orb flex h-16 w-16 items-center justify-center rounded-full"
        >
          <span className="font-display text-2xl font-bold text-white">
            {agentName.charAt(0).toUpperCase()}
          </span>
        </div>
      </div>
      <h2 className="font-display text-xl font-semibold tracking-tight text-ink">
        Say something to {agentName}
      </h2>
      <p className="mt-1.5 text-sm text-ink-3">or start with a suggestion</p>
      <div className="mt-6 flex flex-wrap items-center justify-center gap-2 px-6">
        {SUGGESTIONS.map((s) => (
          <button
            key={s.text}
            onClick={() => onPick(s.text)}
            className="glass group flex cursor-pointer items-center gap-2 rounded-full px-4 py-2 text-xs text-ink-2 transition-all duration-150 hover:border-line-strong hover:bg-white/[0.035] hover:text-ink hover:shadow-lg hover:shadow-black/30"
          >
            <span className="text-ink-3 transition group-hover:text-ink-2">
              {s.icon}
            </span>
            {s.text}
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

export function MessageList({
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
  return (
    <div
      ref={scrollerRef}
      className="relative z-[1] min-h-0 flex-1 overflow-y-auto"
      aria-label={`Conversation with ${agent.name}`}
    >
      {agent.messages.length === 0 && !sending ? (
        <EmptyState agentName={agent.name} onPick={onSuggestion} />
      ) : (
        <div className="mx-auto flex w-full max-w-3xl flex-col gap-4 px-4 py-6 sm:px-6">
          {agent.messages.map((m) => (
            <Bubble key={m.id} message={m} />
          ))}
          {sending && <ThinkingIndicator name={agent.name} />}
        </div>
      )}
    </div>
  );
}

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
}: {
  agentName: string;
  draft: string;
  setDraft: (v: string) => void;
  sending: boolean;
  disabled: boolean;
  onSend: () => void;
  error: string | null;
  onDismissError: () => void;
}) {
  const taRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    const el = taRef.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(el.scrollHeight, 192)}px`;
  }, [draft]);

  return (
    <div className="safe-composer sticky bottom-0 z-10 bg-gradient-to-t from-abyss via-abyss/95 to-transparent px-4 pt-3 sm:px-6">
      <div className="mx-auto w-full max-w-3xl">
        {error && (
          <div className="mb-2.5">
            <ErrorBanner
              message={error}
              onDismiss={onDismissError}
              icon={<AlertIcon size={14} />}
            />
          </div>
        )}
        <div className="glass-strong focus-glow flex items-end gap-2 rounded-2xl p-2 shadow-xl shadow-black/40 transition-all duration-200">
          <textarea
            ref={taRef}
            value={draft}
            disabled={disabled}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && !e.shiftKey) {
                e.preventDefault();
                onSend();
              }
            }}
            rows={1}
            placeholder={`Message ${agentName}…`}
            className="max-h-48 flex-1 resize-none bg-transparent px-3 py-2 text-sm leading-relaxed text-ink placeholder-ink-3 outline-none"
          />
          <button
            onClick={onSend}
            disabled={disabled || sending || !draft.trim()}
            aria-label="Send"
            className="flex h-9 w-9 shrink-0 cursor-pointer items-center justify-center rounded-xl bg-accent text-abyss shadow-lg shadow-accent/25 transition hover:bg-accent/90 active:scale-95 disabled:cursor-not-allowed disabled:opacity-25 disabled:shadow-none disabled:active:scale-100"
          >
            <SendIcon size={15} />
          </button>
        </div>
        <div className="mt-2 flex items-center justify-between px-2 font-mono text-[10px] text-ink-3">
          <span>⏎ send · ⇧⏎ new line</span>
          <span>anima-daemon</span>
        </div>
      </div>
    </div>
  );
}
