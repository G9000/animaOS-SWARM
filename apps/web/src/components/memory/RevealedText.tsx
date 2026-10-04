import { invisibleNote, revealInvisible } from '../../lib/skills';

/** Model-written text, as text: hidden characters are shown as ⟨U+XXXX⟩
 *  markers and a note counts them (spec §14). Never markup. */
export function RevealedText({
  text,
  className,
}: {
  text: string;
  className?: string;
}) {
  const revealed = revealInvisible(text);
  return (
    <>
      <span className={className}>{revealed.text}</span>
      {revealed.count > 0 && (
        <small className="memory-hidden-note">
          {invisibleNote(revealed.count)}
        </small>
      )}
    </>
  );
}
