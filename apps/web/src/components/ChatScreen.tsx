import {
  memo,
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import { ConversationTools } from './ConversationTools';
import { CopyMessage } from './CopyMessage';
import type { AgentDetail, ChatMessage } from '../lib/types';
import { AlertIcon, BoltIcon, PulseIcon, SendIcon, StopIcon } from './icons';
import { MarkdownMessage } from './MarkdownMessage';
import { ErrorBanner, formatTime } from './ui-bits';
import { isMacPlatform } from '../lib/platform';
import {
  buildTranscript,
  type TranscriptActions,
  type TranscriptItem,
} from '../lib/transcript';
import { PendingMessage, RunActivity, ToolBlock } from './sessions/RunActivity';
import { RunOutcomeCard } from './sessions/RunOutcomeCard';
import { DelegatedTurn, TrimmedDivider } from './sessions/TranscriptNotes';
import { slashSuggestions, type SlashCommand } from '../lib/slash-commands';
import { SlashCommandMenu } from './sessions/SlashCommandMenu';

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

/** Why a reply is partial (spec §4.5, §4.6). */
function messageFlag(message: ChatMessage): string | null {
  const metadata = message.content.metadata;
  if (metadata?.stopped === true) return 'Stopped';
  if (metadata?.incomplete === true) return 'Incomplete';
  return null;
}

/** Memoized on its message: a committed message renders once, however
 *  often the transcript around it changes (a streamed reply's deltas). */
const Bubble = memo(function Bubble({ message }: { message: ChatMessage }) {
  if (message.role !== 'User' && message.role !== 'Assistant') {
    return <EventPill message={message} />;
  }
  const isUser = message.role === 'User';
  const flag = messageFlag(message);
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
          {flag && <span className="message-flag">{flag}</span>}
          <CopyMessage text={message.content.text} />
        </div>
      </div>
    </div>
  );
});

const SUGGESTIONS = [
  {
    icon: <Sparkle />,
    text: 'What can you do for me?',
    title: 'Explore what I can do',
    label: 'DISCOVER',
    detail: 'Understand the agent’s tools and access.',
  },
  {
    icon: <BoltIcon size={13} />,
    text: 'Help me plan my day',
    title: 'Make space for your day',
    label: 'MAKE A PLAN',
    detail: 'Define the objective and next steps.',
  },
  {
    icon: <PulseIcon size={13} />,
    text: 'Check in on me every hour',
    title: 'Set a check-in',
    label: 'STAY IN SYNC',
    detail: 'Agree on a schedule for progress updates.',
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
            <span aria-hidden /> YOUR PERSONAL COMPANION
          </p>
          <h2
            aria-label={`Say something to ${agentName}`}
            className="studio-hero-title"
          >
            What’s on your mind?
          </h2>
          <p className="studio-hero-description">
            I’m {agentName}. Let’s make a plan, work through an idea, or take
            something off your list.
          </p>
          <div className="studio-hero-signature">
            One companion. A little less to carry.
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

function candidateAnchorIds(item: TranscriptItem): string[] {
  switch (item.kind) {
    case 'message':
    case 'delegated':
    case 'revised':
      return [item.message.id];
    case 'tools':
      return item.messageIds;
    default:
      return [];
  }
}

/**
 * The ids each transcript item owns for scroll-jump and search-highlight
 * (spec §15.3), one array per item in `transcript` order. A message with
 * both text and tool calls produces two items that both cite its id (its
 * bubble and its tools block); exactly one may claim it, or the jump
 * target and the highlighted item become ambiguous and unmounting either
 * item deletes the id mapping the other still needs. The text bubble
 * (pushed first, see `buildTranscript`) wins.
 */
function anchorsFor(transcript: readonly TranscriptItem[]): string[][] {
  const claimed = new Set<string>();
  return transcript.map((item) => {
    const owned = candidateAnchorIds(item).filter((id) => !claimed.has(id));
    for (const id of owned) claimed.add(id);
    return owned;
  });
}

const renderBubble = (message: ChatMessage) => <Bubble message={message} />;

/** Memoized on its item: history items keep their identity while a run
 *  streams, so only the items that changed render again. */
const TranscriptEntry = memo(function TranscriptEntry({
  item,
  agentName,
  actions,
}: {
  item: TranscriptItem;
  agentName: string;
  actions?: TranscriptActions;
}) {
  switch (item.kind) {
    case 'message':
      return <Bubble message={item.message} />;
    case 'revised':
      return (
        <details className="revised-draft">
          <summary>Earlier draft (revised)</summary>
          <Bubble message={item.message} />
        </details>
      );
    case 'delegated':
      return (
        <DelegatedTurn from={item.from} text={item.message.content.text} />
      );
    case 'tools':
      return <ToolBlock steps={item.steps} active={false} actions={actions} />;
    case 'run':
      return (
        <RunActivity
          live={item.live}
          agentName={agentName}
          actions={actions}
          renderMessage={renderBubble}
        />
      );
    case 'outcome':
      return (
        <RunOutcomeCard
          run={item.run}
          onSendAgain={actions?.onSendAgain}
          resent={actions?.resentRunIds?.has(item.run.id) ?? false}
        />
      );
    case 'pending':
      return (
        <PendingMessage pending={item.pending} renderMessage={renderBubble} />
      );
    case 'trimmed':
      return (
        <TrimmedDivider
          onCompact={actions?.onCompact}
          compacting={actions?.compacting}
        />
      );
  }
});

export const MessageList = memo(function MessageList({
  agent,
  sending,
  scrollerRef,
  onSuggestion,
  hasOlder = false,
  loadingOlder = false,
  onLoadOlder,
  emptyState,
  items,
  actions,
}: {
  agent: AgentDetail;
  sending: boolean;
  scrollerRef: React.RefObject<HTMLDivElement | null>;
  onSuggestion: (text: string) => void;
  /** Older history exists: show a control and load it on scroll to the top. */
  hasOlder?: boolean;
  loadingOlder?: boolean;
  onLoadOlder?: () => void;
  /** Replaces the welcome screen for sessions that are not new chats. */
  emptyState?: ReactNode;
  /** The session's transcript with its live runs and sends (spec §15.2);
   *  built from `agent.messages` when absent. */
  items?: readonly TranscriptItem[];
  actions?: TranscriptActions;
}) {
  const [awayFromBottom, setAwayFromBottom] = useState(false);
  const [highlight, setHighlight] = useState<string | null>(null);
  const atBottom = useRef(true);
  const messageElements = useRef(new Map<string, HTMLDivElement>());
  const firstMessageId = agent.messages[0]?.id;
  const anchor = useRef<{ firstId: string | undefined; height: number }>({
    firstId: firstMessageId,
    height: 0,
  });
  const transcript = useMemo(
    () => items ?? buildTranscript({ messages: agent.messages }),
    [items, agent.messages],
  );
  const anchorsByItem = useMemo(() => anchorsFor(transcript), [transcript]);
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
  }, [transcript, sending, scrollerRef]);
  // Keep the reading position when older messages are prepended.
  useLayoutEffect(() => {
    const element = scrollerRef.current;
    if (!element) return;
    const previous = anchor.current;
    if (
      previous.firstId !== undefined &&
      previous.firstId !== firstMessageId &&
      !atBottom.current
    ) {
      element.scrollTop += element.scrollHeight - previous.height;
    }
    anchor.current = { firstId: firstMessageId, height: element.scrollHeight };
  }, [firstMessageId, agent.messages, scrollerRef]);

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
            if (element.scrollTop < 40 && hasOlder && !loadingOlder) {
              onLoadOlder?.();
            }
          }}
        >
          {transcript.length === 0 && !sending ? (
            (emptyState ?? (
              <EmptyState agentName={agent.name} onPick={onSuggestion} />
            ))
          ) : (
            <div className="studio-messages mx-auto flex w-full max-w-3xl flex-col gap-4 px-4 py-6 sm:px-6">
              {hasOlder && onLoadOlder && (
                <button
                  type="button"
                  className="studio-tool-button session-load-older"
                  onClick={onLoadOlder}
                  disabled={loadingOlder}
                >
                  {loadingOlder
                    ? 'Loading older messages…'
                    : 'Load older messages'}
                </button>
              )}
              {transcript.map((item, index) => {
                const ids = anchorsByItem[index];
                // This item's own last-registered element, so cleanup
                // only ever removes what THIS ref put there — never a
                // different item's registration for the same id.
                let ownElement: HTMLDivElement | null = null;
                return (
                  <div
                    key={item.key}
                    ref={(element) => {
                      if (element) {
                        ownElement = element;
                        for (const id of ids)
                          messageElements.current.set(id, element);
                      } else {
                        for (const id of ids)
                          if (messageElements.current.get(id) === ownElement)
                            messageElements.current.delete(id);
                        ownElement = null;
                      }
                    }}
                    data-search-match={
                      (highlight !== null && ids.includes(highlight)) ||
                      undefined
                    }
                    className="studio-message-anchor"
                  >
                    <TranscriptEntry
                      item={item}
                      agentName={agent.name}
                      actions={actions}
                    />
                  </div>
                );
              })}
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
  label,
  draft,
  setDraft,
  sending,
  disabled,
  onSend,
  error,
  onDismissError,
  offline = false,
  recovery,
  commands,
  runActive = false,
  onStop,
  onSteer,
}: {
  agentName: string;
  /** The textarea's name and placeholder; defaults to "Message <agent>". */
  label?: string;
  draft: string;
  setDraft: (v: string) => void;
  sending: boolean;
  disabled: boolean;
  /** Sends the draft, or `text` — a command picked from the menu. */
  onSend: (text?: string) => void;
  error: string | null;
  onDismissError: () => void;
  offline?: boolean;
  recovery?: {
    count: number;
    text: string;
    restore: () => void;
    dismiss: () => void;
  };
  /** The slash commands the menu offers (spec §15.3); none without them. */
  commands?: readonly SlashCommand[];
  /** This session's reply is in progress: Send becomes Stop and
   *  ⌘/Ctrl+Enter steers it (spec §15.3). */
  runActive?: boolean;
  onStop?: () => void;
  onSteer?: () => void;
}) {
  const taRef = useRef<HTMLTextAreaElement>(null);
  const menuId = useId();
  const [activeIndex, setActiveIndex] = useState(0);
  const [dismissedFor, setDismissedFor] = useState<string | null>(null);
  const [blurred, setBlurred] = useState(false);
  const inputLabel = label ?? `Message ${agentName}`;
  const suggestions = commands ? slashSuggestions(draft, commands) : [];
  const menuOpen = suggestions.length > 0 && dismissedFor !== draft && !blurred;
  const selected = menuOpen
    ? suggestions[Math.min(activeIndex, suggestions.length - 1)]
    : null;
  const canSend = !disabled && !sending && !offline && draft.trim().length > 0;
  const canPick = !disabled && !sending && !offline;
  const steerable = runActive && onSteer !== undefined;

  useEffect(() => {
    setActiveIndex(0);
  }, [draft]);

  useEffect(() => {
    const el = taRef.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(el.scrollHeight, 192)}px`;
  }, [draft]);

  /** Runs a command that needs nothing more; completes one that does. */
  const pick = (command: SlashCommand, complete = false) => {
    if (command.needs || complete) {
      setDraft(`/${command.name}${command.needs ? ' ' : ''}`);
      taRef.current?.focus();
      return;
    }
    if (canPick) onSend(`/${command.name}`);
  };

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
        {menuOpen && (
          <SlashCommandMenu
            id={menuId}
            commands={suggestions}
            activeName={selected?.name ?? null}
            onPick={(command) => pick(command)}
          />
        )}
        <div className="studio-composer-box glass-strong focus-glow flex items-end gap-2 rounded-2xl p-2 transition-all duration-200">
          <textarea
            data-workspace-composer
            ref={taRef}
            value={draft}
            disabled={disabled}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              const composing = e.nativeEvent.isComposing || e.keyCode === 229;
              if (selected) {
                if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
                  e.preventDefault();
                  const step = e.key === 'ArrowDown' ? 1 : -1;
                  setActiveIndex(
                    (index) =>
                      (Math.min(index, suggestions.length - 1) +
                        step +
                        suggestions.length) %
                      suggestions.length,
                  );
                  return;
                }
                if (e.key === 'Escape') {
                  e.preventDefault();
                  e.stopPropagation();
                  setDismissedFor(draft);
                  return;
                }
                if (
                  (e.key === 'Tab' && !e.shiftKey) ||
                  (e.key === 'Enter' && !e.shiftKey && !composing)
                ) {
                  e.preventDefault();
                  pick(selected, e.key === 'Tab');
                  return;
                }
              }
              if (e.key === 'Enter' && !e.shiftKey && !composing) {
                e.preventDefault();
                if (!canSend) return;
                if ((e.metaKey || e.ctrlKey) && steerable) onSteer?.();
                else onSend();
              }
            }}
            onFocus={() => setBlurred(false)}
            onBlur={() => setBlurred(true)}
            rows={1}
            aria-label={inputLabel}
            aria-autocomplete={commands ? 'list' : undefined}
            aria-controls={menuOpen ? menuId : undefined}
            aria-activedescendant={
              selected ? `${menuId}-${selected.name}` : undefined
            }
            placeholder={`${inputLabel}…`}
            className="max-h-48 flex-1 resize-none bg-transparent px-3 py-2 text-sm leading-relaxed text-ink placeholder-ink-3 outline-none"
          />
          {runActive && onStop ? (
            <button
              type="button"
              onClick={onStop}
              aria-label="Stop"
              className="flex h-9 w-9 shrink-0 cursor-pointer items-center justify-center rounded-xl border border-line-strong bg-panel-2 text-ink transition hover:bg-panel active:scale-95"
            >
              <StopIcon size={15} />
            </button>
          ) : (
            <button
              type="button"
              onClick={() => onSend()}
              disabled={!canSend}
              aria-label="Send"
              className="flex h-9 w-9 shrink-0 cursor-pointer items-center justify-center rounded-xl bg-accent text-accent-fg shadow-lg shadow-accent/25 transition hover:bg-accent/90 active:scale-95 disabled:cursor-not-allowed disabled:opacity-25 disabled:shadow-none disabled:active:scale-100"
            >
              <SendIcon size={15} />
            </button>
          )}
        </div>
        <div className="mt-2 flex items-center justify-between px-2 font-mono text-[10px] text-ink-3">
          <span>
            {steerable
              ? // Ctrl+Enter on every non-Mac platform, never ⌘⏎ (S3b-E).
                `⏎ queue · ${isMacPlatform() ? '⌘⏎' : 'Ctrl+Enter'} steer · ⇧⏎ new line`
              : '⏎ send · ⇧⏎ new line'}
          </span>
          <span>
            {offline
              ? 'Offline · your draft stays here'
              : sending
                ? 'Working on your message…'
                : runActive
                  ? 'Replying · you can keep writing'
                  : 'Your space. Your pace.'}
          </span>
        </div>
      </div>
    </div>
  );
}
