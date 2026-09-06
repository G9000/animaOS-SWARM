import { useEffect, useRef, useState } from 'react';
import { createDaemonClient, type ChatGptStatus } from '@animaOS-SWARM/sdk';

const client = createDaemonClient({ baseUrl: '' }).chatgpt;
const verificationUrl = 'https://auth.openai.com/codex/device';
const buttonClass =
  'rounded-xl border border-line px-3 py-2 text-sm text-ink disabled:opacity-50';

export function ChatGptConnection({
  onConnectionChange,
}: {
  onConnectionChange?: () => void | Promise<void>;
}) {
  const [status, setStatus] = useState<ChatGptStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(true);
  const [now, setNow] = useState(Date.now);
  const [poll, setPoll] = useState(0);
  const generation = useRef(0);
  const connected = useRef<boolean | null>(null);
  const onChange = useRef(onConnectionChange);
  onChange.current = onConnectionChange;

  async function request(
    action: 'status' | 'login' | 'cancel' | 'disconnect',
    foreground = true,
  ) {
    const version = ++generation.current;
    if (foreground) setBusy(true);
    setError(null);
    try {
      const next = await client[action]();
      if (version !== generation.current) return;
      setStatus(next);
      if (
        connected.current !== next.connected &&
        (connected.current !== null || next.connected)
      ) {
        void Promise.resolve(onChange.current?.()).catch(() => {
          if (version === generation.current)
            setError(
              'Account updated, but the provider catalog could not refresh. Retry the provider catalog.',
            );
        });
      }
      connected.current = next.connected;
    } catch (cause) {
      if (version === generation.current)
        setError(
          cause instanceof Error
            ? cause.message
            : 'Unable to update ChatGPT connection.',
        );
    } finally {
      if (version === generation.current) {
        setBusy(false);
        setNow(Date.now());
        setPoll((value) => value + 1);
      }
    }
  }

  useEffect(() => {
    void request('status');
    return () => {
      generation.current += 1;
    };
  }, []);

  const login = status?.login;
  const expired = Boolean(login && now >= login.expiresAtMs);
  useEffect(() => {
    if (!login || expired || busy) return;
    const timer = window.setTimeout(
      () => {
        setNow(Date.now());
        void request('status', false);
      },
      Math.min(3000, Math.max(0, login.expiresAtMs - Date.now())),
    );
    return () => window.clearTimeout(timer);
  }, [login, expired, busy, poll]);

  return (
    <section
      aria-label="ChatGPT subscription"
      className="space-y-3 rounded-xl border border-line bg-white/[0.02] p-4"
    >
      <h3 className="text-sm font-semibold text-ink">ChatGPT subscription</h3>
      <p className="text-xs leading-relaxed text-ink-2">
        Use your ChatGPT plan. Subscription limits apply; model availability
        depends on your account. This connection does not use API-key billing.
      </p>
      <p className="text-xs leading-relaxed text-ink-3">
        Enable device-code login in your ChatGPT security settings, or ask your
        workspace administrator to allow it, before connecting.
      </p>
      <p role="status" className="text-sm text-ink-2">
        {busy
          ? 'Updating ChatGPT connection…'
          : !status
            ? 'ChatGPT connection status is unavailable.'
            : status.connected
              ? `Connected${status.planType ? ` · ${status.planType}` : ''}`
              : expired
                ? 'Sign-in expired. Start again for a new code.'
                : login
                  ? 'Waiting for you to finish signing in…'
                  : 'ChatGPT is not connected.'}
      </p>
      {status?.connected && status.accountId && (
        <p className="break-all text-xs text-ink-3">
          Account: {status.accountId}
        </p>
      )}
      {(error || status?.error) && (
        <p role="alert" className="text-sm text-danger">
          {error || status?.error}
        </p>
      )}
      {login && !expired && (
        <div className="space-y-2 text-sm text-ink">
          <p>
            Enter this code on OpenAI:{' '}
            <strong className="select-all font-mono">{login.userCode}</strong>
          </p>
          <p className="text-xs text-ink-3">
            Expires at {new Date(login.expiresAtMs).toLocaleTimeString()}.
          </p>
          {login.verificationUrl === verificationUrl ? (
            <a
              href={verificationUrl}
              target="_blank"
              rel="noopener noreferrer"
              className="inline-block text-accent underline"
            >
              Continue on OpenAI
            </a>
          ) : (
            <p role="alert" className="text-danger">
              The daemon returned an unexpected verification address. Cancel and
              try again.
            </p>
          )}
        </div>
      )}
      <div className="flex flex-wrap gap-2">
        {status?.connected ? (
          <button
            type="button"
            className={buttonClass}
            disabled={busy}
            onClick={() => void request('disconnect')}
          >
            Disconnect ChatGPT
          </button>
        ) : !login || expired ? (
          <button
            type="button"
            className={buttonClass}
            disabled={busy}
            onClick={() => void request('login')}
          >
            Connect ChatGPT
          </button>
        ) : null}
        {login && (
          <button
            type="button"
            className={buttonClass}
            disabled={busy}
            onClick={() => void request('cancel')}
          >
            Cancel sign-in
          </button>
        )}
        {error && (
          <button
            type="button"
            className={buttonClass}
            disabled={busy}
            onClick={() => void request('status')}
          >
            Refresh connection
          </button>
        )}
        {!status && error && (
          <button
            type="button"
            className={buttonClass}
            disabled={busy}
            onClick={() => void request('disconnect')}
          >
            Clear saved ChatGPT connection
          </button>
        )}
      </div>
    </section>
  );
}
