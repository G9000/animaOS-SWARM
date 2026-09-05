import { useEffect, useMemo, useRef, useState } from 'react';
import type { AgentDetail } from '../lib/types';
import {
  conversationFilename,
  conversationMarkdown,
} from '../lib/conversation';

export function ConversationTools({
  agent,
  onJump,
  onHighlight,
}: {
  agent: AgentDetail;
  onJump: (id: string) => void;
  onHighlight: (id: string | null) => void;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [index, setIndex] = useState(0);
  const [exportError, setExportError] = useState(false);
  const search = useRef<HTMLInputElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const term = query.trim().toLowerCase();
  const matches = useMemo(
    () =>
      term
        ? agent.messages.filter((message) =>
            message.content.text.toLowerCase().includes(term),
          )
        : [],
    [agent.messages, term],
  );
  const selected = matches[Math.min(index, Math.max(0, matches.length - 1))];
  useEffect(() => {
    onHighlight(open ? (selected?.id ?? null) : null);
  }, [selected?.id, open, onHighlight]);
  useEffect(() => {
    if (open) search.current?.focus();
  }, [open]);

  function move(direction: number) {
    if (!matches.length) return;
    const next =
      (Math.min(index, matches.length - 1) + direction + matches.length) %
      matches.length;
    setIndex(next);
    onJump(matches[next].id);
  }

  function close() {
    setOpen(false);
    setQuery('');
    setIndex(0);
    trigger.current?.focus();
  }

  function download() {
    let url: string | undefined;
    try {
      url = URL.createObjectURL(
        new Blob([conversationMarkdown(agent)], {
          type: 'text/markdown;charset=utf-8',
        }),
      );
      const link = document.createElement('a');
      link.href = url;
      link.download = conversationFilename(agent.name);
      document.body.appendChild(link);
      link.click();
      link.remove();
      setExportError(false);
    } catch {
      setExportError(true);
    } finally {
      if (url) {
        const revoke = url;
        window.setTimeout(() => URL.revokeObjectURL(revoke), 1000);
      }
    }
  }

  return (
    <div className="studio-conversation-tools">
      <div className="studio-conversation-toolbar">
        <span>
          <strong>Conversation</strong>
          <small>{agent.messages.length} loaded messages</small>
        </span>
        <div>
          <button
            ref={trigger}
            type="button"
            className="studio-tool-button"
            aria-label="Search conversation"
            aria-expanded={open}
            onClick={() => setOpen((value) => !value)}
          >
            <span aria-hidden>⌕</span> Search
          </button>
          <button
            type="button"
            className="studio-tool-button"
            onClick={download}
            disabled={!agent.messages.length}
            title="Download all loaded messages as Markdown"
          >
            ↓ <span>Export</span>
          </button>
        </div>
      </div>
      {open && (
        <div className="studio-conversation-search">
          <input
            ref={search}
            type="search"
            className="field"
            aria-label="Search messages"
            placeholder="Find something in this conversation…"
            value={query}
            onChange={(event) => {
              setQuery(event.target.value);
              setIndex(0);
            }}
            onKeyDown={(event) => {
              if (event.nativeEvent.isComposing) return;
              if (event.key === 'Escape') {
                event.preventDefault();
                close();
              }
              if (event.key === 'Enter') {
                event.preventDefault();
                if (selected) onJump(selected.id);
              }
            }}
          />
          <span role="status" aria-live="polite">
            {!term
              ? 'Search loaded messages'
              : matches.length
                ? `${Math.min(index + 1, matches.length)} of ${matches.length}`
                : 'No matches'}
          </span>
          <button
            type="button"
            className="studio-tool-button"
            disabled={!matches.length}
            onClick={() => move(-1)}
            aria-label="Previous match"
          >
            ↑
          </button>
          <button
            type="button"
            className="studio-tool-button"
            disabled={!matches.length}
            onClick={() => move(1)}
            aria-label="Next match"
          >
            ↓
          </button>
          <button
            type="button"
            className="studio-tool-button"
            onClick={close}
            aria-label="Close conversation search"
          >
            ×
          </button>
        </div>
      )}
      {exportError && (
        <p role="alert">Export could not start. Please try again.</p>
      )}
    </div>
  );
}
