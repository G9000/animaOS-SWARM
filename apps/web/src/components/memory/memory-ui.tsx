import { useEffect, useRef } from 'react';

/** Characters as the daemon counts them (code points, not UTF-16 units). */
export function charCount(text: string): number {
  let count = 0;
  for (const _ of text) count += 1;
  return count;
}

export function formatCount(count: number, max: number): string {
  return `${count.toLocaleString('en-US')} / ${max.toLocaleString('en-US')}`;
}

/** An inline "are you sure": the safe choice has focus, Escape keeps. */
export function ConfirmRow({
  prompt,
  confirmLabel,
  onConfirm,
  onKeep,
}: {
  prompt: string;
  confirmLabel: string;
  onConfirm: () => void;
  onKeep: () => void;
}) {
  const keepRef = useRef<HTMLButtonElement>(null);
  useEffect(() => keepRef.current?.focus(), []);
  return (
    <div
      className="memory-confirm"
      role="group"
      aria-label="Confirm"
      onKeyDown={(event) => {
        if (event.key === 'Escape') {
          event.stopPropagation();
          onKeep();
        }
      }}
    >
      <span className="memory-confirm-text">{prompt}</span>
      <button type="button" className="studio-tool-button" onClick={onConfirm}>
        {confirmLabel}
      </button>
      <button
        type="button"
        ref={keepRef}
        className="studio-tool-button"
        onClick={onKeep}
      >
        Keep
      </button>
    </div>
  );
}
