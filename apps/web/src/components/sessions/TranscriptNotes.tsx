import { MarkdownMessage } from '../MarkdownMessage';

/** Where the companion's view of the session begins (spec §5.3). */
export function TrimmedDivider({ onCompact }: { onCompact?: () => void }) {
  return (
    <div className="context-trimmed">
      {/* The separator role belongs on a decorative element, never one
       *  wrapping the interactive Compact button (ARIA forbids focusable
       *  descendants of role="separator"). */}
      <span className="context-trimmed-rule" role="separator" />
      <span>Earlier messages are outside the companion’s view</span>
      {onCompact && (
        <button
          type="button"
          className="studio-tool-button"
          onClick={onCompact}
        >
          Compact
        </button>
      )}
    </div>
  );
}

/** A helper session's user turn, credited to the agent that wrote it
 *  (the delegating companion, or the peer that sent it). */
export function DelegatedTurn({ from, text }: { from: string; text: string }) {
  return (
    <div className="delegated-turn">
      <p className="delegated-turn-from">From {from}</p>
      <MarkdownMessage>{text}</MarkdownMessage>
    </div>
  );
}
