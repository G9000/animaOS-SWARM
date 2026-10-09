import type { CardState, HealthCard } from '../../lib/status';
import { RevealedText } from '../memory/RevealedText';

/** State is a word and a shape, never color alone. */
const STATE_MARK: Record<CardState, { shape: string; label: string }> = {
  ok: { shape: '●', label: 'OK' },
  warn: { shape: '▲', label: 'Needs attention' },
  bad: { shape: '■', label: 'Problem' },
};

/** One Health card. Every string can hold daemon text (an error, a
 *  connector's name), so each renders through `RevealedText`. */
export function StatusCard({ card }: { card: HealthCard }) {
  const mark = STATE_MARK[card.state];
  return (
    <section
      className={`system-card system-card-${card.state}`}
      aria-label={card.title}
    >
      <div className="system-card-head">
        <h3>{card.title}</h3>
        <span className="system-card-state">
          <span aria-hidden>{mark.shape}</span> {mark.label}
        </span>
      </div>
      <p className="system-card-summary">
        <RevealedText text={card.summary} />
      </p>
      {card.details.length > 0 && (
        <ul className="system-card-details">
          {card.details.map((detail, index) => (
            <li key={`${index}:${detail}`}>
              <RevealedText text={detail} />
            </li>
          ))}
        </ul>
      )}
      {card.link && (
        <a className="studio-tool-button" href={card.link.hash}>
          {card.link.label}
        </a>
      )}
    </section>
  );
}
