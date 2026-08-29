import { useEffect, useState } from 'react';
import type { TelegramConnector } from '../lib/daemon-api';
import { connectorStatusLabel } from '../lib/telegram';
import { AlertIcon, TrashIcon } from './icons';
import { ErrorBanner, SectionTitle, primaryBtnCls } from './ui-bits';

export function TelegramSettings({
  connector,
  busy,
  error,
  connect,
  replace,
  approve,
  restart,
  disconnect,
  refresh,
}: {
  connector: TelegramConnector | null;
  busy: string | null;
  error: string | null;
  connect: (token: string) => Promise<boolean>;
  replace: (connectorId: string, token: string) => Promise<boolean>;
  approve: (connectorId: string, chatId: string) => Promise<boolean>;
  restart: (connectorId: string) => Promise<boolean>;
  disconnect: (connectorId: string) => Promise<boolean>;
  refresh?: () => Promise<void>;
}) {
  const [token, setToken] = useState('');
  const [replacing, setReplacing] = useState(false);
  const [confirmDisconnect, setConfirmDisconnect] = useState(false);
  useEffect(() => {
    setToken('');
    setReplacing(false);
    setConfirmDisconnect(false);
  }, [connector?.id]);
  const submit = async () => {
    const value = token.trim();
    if (!value) return;
    try {
      if (connector) await replace(connector.id, value);
      else await connect(value);
    } finally {
      setToken('');
      setReplacing(false);
    }
  };
  const disabled = busy !== null;
  return (
    <section className="space-y-3">
      <SectionTitle>Telegram</SectionTitle>
      {connector ? (
        <div className="rounded-xl border border-line bg-white/[0.02] p-4">
          <div className="flex items-start justify-between gap-3">
            <div>
              <p className="text-sm font-semibold text-ink">
                {connector.bot.username
                  ? `@${connector.bot.username}`
                  : (connector.bot.displayName ?? 'Telegram bot')}
              </p>
              <p className="mt-1 font-mono text-[10px] uppercase tracking-wider text-ink-3">
                {connectorStatusLabel(connector.status)}
              </p>
            </div>
            <button
              type="button"
              disabled={disabled}
              onClick={() => void restart(connector.id)}
              className="rounded-lg border border-line px-2.5 py-1.5 text-xs text-ink-2 disabled:opacity-50"
            >
              Restart
            </button>
          </div>
          {connector.pendingPairing ? (
            <div className="mt-3 rounded-lg border border-accent/25 bg-accent/[0.06] p-3">
              <p className="text-xs text-ink">
                Pair with{' '}
                {connector.pendingPairing.chat.title ??
                  connector.pendingPairing.chat.username ??
                  connector.pendingPairing.chat.id}
              </p>
              <button
                type="button"
                disabled={disabled}
                onClick={() =>
                  void approve(connector.id, connector.pendingPairing!.chat.id)
                }
                className={`${primaryBtnCls} mt-2`}
              >
                Approve chat
              </button>
            </div>
          ) : null}
          {connector.approvedChat ? (
            <p className="mt-3 text-xs text-ink-2">
              Delivering to{' '}
              {connector.approvedChat.title ??
                connector.approvedChat.username ??
                connector.approvedChat.id}
            </p>
          ) : (
            <p className="mt-3 text-xs text-ink-3">
              Message the bot in Telegram, then approve the pending chat here.
            </p>
          )}
          <div className="mt-4 flex flex-wrap gap-2">
            {refresh ? (
              <button
                type="button"
                disabled={disabled}
                onClick={() => void refresh()}
                className="rounded-lg border border-line px-3 py-1.5 text-xs text-ink-2 disabled:opacity-50"
              >
                Refresh status
              </button>
            ) : null}
            <button
              type="button"
              disabled={disabled}
              onClick={() => setReplacing((value) => !value)}
              className="rounded-lg border border-line px-3 py-1.5 text-xs text-ink-2 disabled:opacity-50"
            >
              Replace token
            </button>
            <button
              type="button"
              disabled={disabled}
              onClick={() =>
                confirmDisconnect
                  ? void disconnect(connector.id)
                  : setConfirmDisconnect(true)
              }
              className="flex items-center gap-1.5 rounded-lg border border-danger/30 px-3 py-1.5 text-xs text-danger disabled:opacity-50"
            >
              <TrashIcon size={12} />
              {confirmDisconnect ? 'Confirm disconnect' : 'Disconnect'}
            </button>
          </div>
        </div>
      ) : (
        <p className="text-xs leading-relaxed text-ink-2">
          Paste a BotFather token. The daemon stores it in the operating-system
          credential vault and starts Telegram automatically.
        </p>
      )}
      {!connector || replacing ? (
        <div>
          <label
            htmlFor="telegram-bot-token"
            className="mb-1.5 block font-mono text-[10px] uppercase tracking-wider text-ink-3"
          >
            Bot token
          </label>
          <input
            id="telegram-bot-token"
            type="password"
            autoComplete="off"
            value={token}
            disabled={disabled}
            onChange={(event) => setToken(event.target.value)}
            className="field"
          />
          <button
            type="button"
            disabled={disabled || !token.trim()}
            onClick={() => void submit()}
            className={`${primaryBtnCls} mt-2 w-full`}
          >
            {busy === 'connect' || busy === 'replace'
              ? 'Connecting…'
              : connector
                ? 'Save replacement token'
                : 'Connect Telegram'}
          </button>
        </div>
      ) : null}
      {error ? (
        <div role="alert">
          <ErrorBanner message={error} icon={<AlertIcon size={14} />} />
        </div>
      ) : null}
    </section>
  );
}
