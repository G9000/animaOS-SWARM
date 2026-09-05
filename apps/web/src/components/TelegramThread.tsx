import { useState } from 'react';
import type { ConnectorMessage } from '../lib/daemon-api';
import { AlertIcon, SendIcon } from './icons';
import { ErrorBanner, formatTime } from './ui-bits';

export function TelegramThread({
  agentName,
  messages,
  hasOlder,
  busy,
  error,
  deliveryQueued,
  loadOlder,
  send,
}: {
  agentName: string;
  messages: ConnectorMessage[];
  hasOlder: boolean;
  busy: string | null;
  error: string | null;
  deliveryQueued: boolean;
  loadOlder: () => Promise<boolean>;
  send: (text: string) => Promise<boolean>;
}) {
  const [draft, setDraft] = useState('');
  const submit = async () => {
    const text = draft.trim();
    if (!text) return;
    if (await send(text)) setDraft('');
  };
  return (
    <section
      className="studio-telegram flex h-full min-h-0 flex-col"
      aria-label="Telegram thread"
    >
      <header className="border-b border-line bg-panel/60 px-6 py-3">
        <h2 className="font-display text-[15px] font-semibold text-ink">
          Telegram
        </h2>
        <p className="font-mono text-[11px] text-ink-3">
          dedicated room · {agentName}
        </p>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto px-4 py-5 sm:px-6">
        {hasOlder ? (
          <div className="mb-4 text-center">
            <button
              type="button"
              disabled={busy !== null}
              onClick={() => void loadOlder()}
              className="rounded-lg border border-line px-3 py-1.5 text-xs text-ink-2"
            >
              Load older messages
            </button>
          </div>
        ) : null}
        {messages.length ? (
          <div className="mx-auto flex max-w-3xl flex-col gap-3">
            {messages.map((message) => (
              <div
                key={message.id}
                className={`flex ${message.role === 'user' ? 'justify-end' : 'justify-start'}`}
              >
                <div className="max-w-[85%]">
                  <div className="glass whitespace-pre-wrap rounded-2xl px-4 py-2.5 text-sm text-ink">
                    {message.content.text}
                  </div>
                  <div className="mt-1 px-1 font-mono text-[10px] text-ink-3">
                    {formatTime(message.createdAtMs)}
                  </div>
                </div>
              </div>
            ))}
          </div>
        ) : (
          <p className="pt-[14vh] text-center text-sm text-ink-3">
            Telegram messages stay separate from the workspace conversation.
          </p>
        )}
      </div>
      <div className="safe-composer px-4 pb-2 sm:px-6">
        {deliveryQueued ? (
          <p
            role="status"
            className="mb-2 text-center font-mono text-[10px] text-mint"
          >
            Queued for Telegram delivery
          </p>
        ) : null}
        {error ? (
          <div className="mb-2">
            <ErrorBanner message={error} icon={<AlertIcon size={14} />} />
          </div>
        ) : null}
        <div className="glass-strong mx-auto flex max-w-3xl items-end gap-2 rounded-2xl p-2">
          <textarea
            aria-label={`Message ${agentName} on Telegram`}
            value={draft}
            disabled={busy !== null}
            onChange={(event) => setDraft(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter' && !event.shiftKey) {
                event.preventDefault();
                void submit();
              }
            }}
            rows={1}
            placeholder={`Message ${agentName} on Telegram…`}
            className="flex-1 resize-none bg-transparent px-3 py-2 text-sm text-ink outline-none"
          />
          <button
            type="button"
            aria-label="Send to Telegram"
            disabled={busy !== null || !draft.trim()}
            onClick={() => void submit()}
            className="flex h-9 w-9 items-center justify-center rounded-xl bg-accent text-abyss disabled:opacity-25"
          >
            <SendIcon size={15} />
          </button>
        </div>
      </div>
    </section>
  );
}
