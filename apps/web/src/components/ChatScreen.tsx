import { useEffect, useRef } from 'react';
import type { AgentDetail, ChatMessage } from '../lib/types';
import { AlertIcon, BoltIcon, ChipIcon, GearIcon, PulseIcon, SendIcon } from './icons';
import { ErrorBanner, formatTime, formatTokens, ghostBtnCls } from './ui-bits';

const STATUS_STYLE: Record<AgentDetail['status'], { dot: string; ping: boolean; label: string }> = {
  Idle: { dot: 'bg-mint', ping: false, label: 'idle' },
  Running: { dot: 'bg-sky-400', ping: true, label: 'thinking' },
  Completed: { dot: 'bg-violet-400', ping: false, label: 'done' },
  Failed: { dot: 'bg-red-400', ping: false, label: 'failed' },
  Terminated: { dot: 'bg-zinc-500', ping: false, label: 'stopped' },
};

function StatusDot({ status }: { status: AgentDetail['status'] }) {
  const s = STATUS_STYLE[status] ?? STATUS_STYLE.Idle;
  return (
    <span className="relative flex h-2 w-2">
      {s.ping && <span className={`animate-status-ping absolute inline-flex h-full w-full rounded-full ${s.dot}`} />}
      <span className={`relative inline-flex h-2 w-2 rounded-full ${s.dot}`} />
    </span>
  );
}

function AgentAvatar({ name, size = 'md' }: { name: string; size?: 'sm' | 'md' }) {
  const dims = size === 'sm' ? 'h-7 w-7 text-xs' : 'h-9 w-9 text-sm';
  return (
    <div
      className={`relative flex ${dims} shrink-0 items-center justify-center rounded-full bg-sky-500 font-display font-bold text-white shadow-lg shadow-sky-500/20`}
    >
      {name.charAt(0).toUpperCase()}
    </div>
  );
}

/* ── Header ── */
export function ChatHeader({
  agent,
  online,
  onOpenSettings,
}: {
  agent: AgentDetail;
  online: boolean | null;
  onOpenSettings: () => void;
}) {
  const status = STATUS_STYLE[agent.status] ?? STATUS_STYLE.Idle;
  const totalTokens = agent.token_usage?.total_tokens ?? 0;

  return (
    <header className="relative z-10 flex items-center justify-between gap-3 border-b border-line bg-panel/60 px-5 py-3 backdrop-blur-xl">
      <div className="flex min-w-0 items-center gap-3.5">
        <AgentAvatar name={agent.name} />
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <span className="truncate font-display text-[15px] font-semibold tracking-tight text-ink">
              {agent.name}
            </span>
            <StatusDot status={agent.status} />
            <span className={`font-mono text-[10px] uppercase tracking-wider ${agent.status === 'Running' ? 'text-shimmer' : 'text-ink-3'}`}>
              {online === false ? 'offline' : status.label}
            </span>
          </div>
          <div className="mt-0.5 flex items-center gap-2 font-mono text-[11px] text-ink-3">
            <span className="rounded border border-line bg-white/[0.03] px-1.5 py-px">
              {agent.provider}/{agent.model}
            </span>
            {totalTokens > 0 && (
              <span className="flex items-center gap-1" title="total tokens used">
                <ChipIcon size={11} />
                {formatTokens(totalTokens)}
              </span>
            )}
          </div>
        </div>
      </div>
      <button onClick={onOpenSettings} className={ghostBtnCls}>
        <GearIcon size={13} />
        Settings
      </button>
    </header>
  );
}

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
    <div className={`animate-msg-in flex ${isUser ? 'justify-end' : 'justify-start'}`}>
      <div className={`flex max-w-[85%] flex-col ${isUser ? 'items-end' : 'items-start'}`}>
        <div
          className={`whitespace-pre-wrap break-words px-4 py-2.5 text-sm leading-relaxed ${
            isUser
              ? 'rounded-2xl rounded-br-md bg-sky-600 text-white shadow-lg shadow-sky-600/15'
              : 'glass rounded-2xl rounded-bl-md text-ink'
          }`}
        >
          {message.content.text}
        </div>
        <span className="mt-1 px-1 font-mono text-[10px] text-ink-3/60">
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
        <span className="animate-ripple absolute inset-0 rounded-full border border-sky-400/30" />
        <span
          className="animate-ripple absolute inset-0 rounded-full border border-sky-400/20"
          style={{ animationDelay: '1.3s' }}
        />
        <div className="animate-orb flex h-16 w-16 items-center justify-center rounded-full bg-sky-500 shadow-2xl shadow-sky-500/30">
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
            className="glass flex cursor-pointer items-center gap-2 rounded-full px-4 py-2 text-xs text-ink-2 transition-all duration-150 hover:border-sky-400/40 hover:text-ink hover:shadow-[0_0_16px_-4px_rgba(56,189,248,0.4)]"
          >
            <span className="text-sky-400">{s.icon}</span>
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
            className="typing-dot h-1.5 w-1.5 rounded-full bg-sky-400"
            style={{ animationDelay: `${i * 150}ms` }}
          />
        ))}
      </div>
      <span className="text-shimmer font-mono text-[11px]">{name} is thinking</span>
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
    <div ref={scrollerRef} className="relative z-[1] flex-1 overflow-y-auto">
      {agent.messages.length === 0 && !sending ? (
        <EmptyState agentName={agent.name} onPick={onSuggestion} />
      ) : (
        <div className="mx-auto flex w-full max-w-3xl flex-col gap-4 px-6 py-6">
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
    <div className="relative z-10 px-6 pb-5 pt-2">
      <div className="mx-auto w-full max-w-3xl">
        {error && (
          <div className="mb-2.5">
            <ErrorBanner message={error} onDismiss={onDismissError} icon={<AlertIcon size={14} />} />
          </div>
        )}
        <div className="glass-strong focus-glow flex items-end gap-2 rounded-2xl p-2 shadow-xl shadow-black/30 transition-all duration-200">
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
            className="flex h-9 w-9 shrink-0 cursor-pointer items-center justify-center rounded-xl bg-sky-500 text-white shadow-lg shadow-sky-500/25 transition hover:bg-sky-400 active:scale-95 disabled:cursor-not-allowed disabled:opacity-25 disabled:shadow-none disabled:active:scale-100"
          >
            <SendIcon size={15} />
          </button>
        </div>
        <div className="mt-2 flex items-center justify-between px-2 font-mono text-[10px] text-ink-3/60">
          <span>⏎ send · ⇧⏎ new line</span>
          <span>anima-daemon</span>
        </div>
      </div>
    </div>
  );
}
