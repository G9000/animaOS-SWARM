import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { groupFixture } from '../../test/usage';
import { UsageChart } from './UsageChart';

const days = [
  groupFixture('2026-09-21', { totalTokens: 0 }),
  groupFixture('2026-09-22', { totalTokens: 500 }),
  groupFixture('2026-09-23', { totalTokens: 2_000 }),
];

describe('UsageChart', () => {
  it('draws a bar per day with a title and a label', () => {
    const { container } = render(<UsageChart days={days} rangeDays={3} />);

    expect(container.querySelectorAll('rect.usage-bar')).toHaveLength(3);
    expect(
      screen.getByRole('img', {
        name: 'Tokens per day, 3 days, busiest day 2026-09-23 with 2k tokens',
      }),
    ).toBeInTheDocument();
    expect(container.querySelector('title')?.textContent).toBe(
      '2026-09-21: 0 tokens',
    );
    expect(
      Array.from(container.querySelectorAll('title')).map(
        (title) => title.textContent,
      ),
    ).toContain('2026-09-23: 2k tokens');
  });

  it('scales bars to the busiest day', () => {
    const { container } = render(<UsageChart days={days} rangeDays={3} />);
    const heights = Array.from(
      container.querySelectorAll('rect.usage-bar'),
    ).map((bar) => Number(bar.getAttribute('height')));

    expect(heights[0]).toBe(0);
    expect(heights[2]).toBeGreaterThan(0);
    expect(heights[1]).toBeCloseTo(heights[2] / 4);
  });

  it('shows nothing for a range with no calls', () => {
    const { container } = render(
      <UsageChart
        days={[groupFixture('2026-09-22'), groupFixture('2026-09-23')]}
        rangeDays={2}
      />,
    );

    expect(
      Array.from(container.querySelectorAll('rect.usage-bar')).every(
        (bar) => bar.getAttribute('height') === '0',
      ),
    ).toBe(true);
    expect(
      screen.getByRole('img', {
        name: 'Tokens per day, 2 days, no tokens used',
      }),
    ).toBeInTheDocument();
  });
});
