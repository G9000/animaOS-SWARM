import { useEffect, useState } from 'react';

export function refreshAgentAvatar(id: string) {
  window.dispatchEvent(new CustomEvent('agent-avatar-changed', { detail: id }));
}

export function AgentAvatar({
  id,
  name,
  size = 28,
}: {
  id: string;
  name: string;
  size?: number;
}) {
  const [revision, setRevision] = useState(0);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    setFailed(false);
    const refresh = (event: Event) => {
      if ((event as CustomEvent<string>).detail === id) {
        setFailed(false);
        setRevision(Date.now());
      }
    };
    window.addEventListener('agent-avatar-changed', refresh);
    return () => window.removeEventListener('agent-avatar-changed', refresh);
  }, [id]);
  return (
    <span
      className="inline-flex shrink-0 items-center justify-center overflow-hidden rounded-lg bg-accent/15 font-semibold"
      style={{ width: size, height: size }}
    >
      {failed ? (
        name.slice(0, 1).toUpperCase()
      ) : (
        <img
          key={`${id}:${revision}`}
          src={`/api/agents/${encodeURIComponent(id)}/avatar?v=${revision}`}
          alt=""
          className="h-full w-full object-cover"
          onError={() => setFailed(true)}
        />
      )}
    </span>
  );
}
