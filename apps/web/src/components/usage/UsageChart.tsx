import type { UsageGroup } from '@animaOS-SWARM/sdk';

import { formatTokens } from '../../lib/usage';

const WIDTH = 640;
const HEIGHT = 160;
const TOP = 8;
const LABEL_BAND = 22;
const PLOT = HEIGHT - TOP - LABEL_BAND;

/** Tokens per day as plain SVG bars (spec §15.4). The caller shows its
 *  empty state instead when the range has no calls. */
export function UsageChart({
  days,
  rangeDays,
}: {
  days: readonly UsageGroup[];
  rangeDays: number;
}) {
  const busiest = days.reduce<UsageGroup | null>(
    (best, day) =>
      day.totals.totalTokens > (best?.totals.totalTokens ?? 0) ? day : best,
    null,
  );
  const peak = busiest?.totals.totalTokens ?? 0;
  const slot = WIDTH / Math.max(days.length, 1);
  const barWidth = Math.max(slot - 2, 1);
  const label = busiest
    ? `Tokens per day, ${rangeDays} days, busiest day ${busiest.key} with ${formatTokens(peak)} tokens`
    : `Tokens per day, ${rangeDays} days, no tokens used`;
  const lastIndex = days.length - 1;
  const labelled = new Set([0, Math.floor(lastIndex / 2), lastIndex]);

  return (
    <svg
      className="usage-chart"
      viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
      role="img"
      aria-label={label}
    >
      <line
        className="usage-chart-axis"
        x1={0}
        x2={WIDTH}
        y1={TOP + PLOT}
        y2={TOP + PLOT}
      />
      {days.map((day, index) => {
        const tokens = day.totals.totalTokens;
        const height = peak > 0 ? (tokens / peak) * PLOT : 0;
        const x = index * slot + 1;
        return (
          <g key={day.key}>
            <rect
              className="usage-bar"
              x={x}
              y={TOP + PLOT - height}
              width={barWidth}
              height={height}
              rx={2}
            >
              <title>{`${day.key}: ${formatTokens(tokens)} tokens`}</title>
            </rect>
            {labelled.has(index) && (
              <text
                className="usage-chart-label"
                x={x + barWidth / 2}
                y={HEIGHT - 6}
                textAnchor={
                  index === 0 ? 'start' : index === lastIndex ? 'end' : 'middle'
                }
              >
                {day.key.slice(5)}
              </text>
            )}
          </g>
        );
      })}
    </svg>
  );
}
