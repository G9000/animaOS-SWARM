import { useEffect, useId, useRef, useState } from 'react';

export interface StudioCommand {
  id: string;
  title: string;
  description: string;
  group: string;
  run: () => void;
}

export function CommandMenu({
  commands,
  close,
}: {
  commands: StudioCommand[];
  close: () => void;
}) {
  const [query, setQuery] = useState('');
  const [active, setActive] = useState(0);
  const dialog = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const id = useId();
  const results = commands.filter((command) =>
    `${command.title} ${command.description} ${command.group}`
      .toLowerCase()
      .includes(query.trim().toLowerCase()),
  );
  const selected = Math.min(active, Math.max(results.length - 1, 0));

  useEffect(() => {
    const trigger =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    input.current?.focus();
    return () => {
      if (trigger?.isConnected) trigger.focus();
    };
  }, []);

  useEffect(() => {
    document
      .getElementById(`${id}-${selected}`)
      ?.scrollIntoView?.({ block: 'nearest' });
  }, [id, selected, query]);

  function choose(command: StudioCommand) {
    close();
    command.run();
  }

  return (
    <div
      className="studio-command-backdrop"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) close();
      }}
    >
      <div
        ref={dialog}
        role="dialog"
        aria-modal="true"
        aria-label="Command menu"
        className="studio-command-menu"
        onKeyDown={(event) => {
          if (event.key === 'Escape') {
            event.preventDefault();
            event.stopPropagation();
            close();
          }
          if (event.key === 'Tab') {
            const focusable = dialog.current?.querySelectorAll<HTMLElement>(
              'input, button:not([tabindex="-1"])',
            );
            if (!focusable?.length) return;
            const first = focusable[0];
            const last = focusable[focusable.length - 1];
            if (event.shiftKey && document.activeElement === first) {
              event.preventDefault();
              last.focus();
            } else if (!event.shiftKey && document.activeElement === last) {
              event.preventDefault();
              first.focus();
            }
          }
        }}
      >
        <div className="studio-command-search">
          <span aria-hidden>⌕</span>
          <input
            ref={input}
            role="combobox"
            aria-label="Search commands"
            aria-expanded="true"
            aria-autocomplete="list"
            aria-controls={`${id}-results`}
            aria-activedescendant={
              results.length ? `${id}-${selected}` : undefined
            }
            placeholder="Where do you want to go?"
            value={query}
            onChange={(event) => {
              setQuery(event.target.value);
              setActive(0);
            }}
            onKeyDown={(event) => {
              if (event.nativeEvent.isComposing) return;
              if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
                event.preventDefault();
                setActive(
                  results.length
                    ? (selected +
                        (event.key === 'ArrowDown' ? 1 : -1) +
                        results.length) %
                        results.length
                    : 0,
                );
              }
              if (event.key === 'Enter' && results[selected]) {
                event.preventDefault();
                choose(results[selected]);
              }
            }}
          />
          <button
            type="button"
            className="studio-tool-button"
            aria-label="Close command menu"
            onClick={close}
          >
            Esc
          </button>
        </div>
        <div
          className="studio-command-results"
          role="listbox"
          id={`${id}-results`}
          aria-label="Commands"
        >
          {results.map((command, index) => (
            <button
              key={command.id}
              type="button"
              role="option"
              tabIndex={-1}
              aria-selected={selected === index}
              id={`${id}-${index}`}
              className="studio-command-option"
              onMouseMove={() => setActive(index)}
              onClick={() => choose(command)}
            >
              <span className="studio-command-symbol" aria-hidden>
                {command.group === 'Navigate' ? '↗' : '✳'}
              </span>
              <span>
                <strong>{command.title}</strong>
                <small>{command.description}</small>
              </span>
              <span className="studio-command-group">{command.group}</span>
            </button>
          ))}
        </div>
        {!results.length && (
          <p role="status" className="studio-command-empty">
            No commands found. Try “workspace”, “plan”, or “settings”.
          </p>
        )}
        <footer>
          <span>↑ ↓ to explore · Enter to select</span>
          <span>Prompts fill your draft. You decide when to send.</span>
        </footer>
      </div>
    </div>
  );
}
