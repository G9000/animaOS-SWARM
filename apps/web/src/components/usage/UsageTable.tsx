import type { ReactNode } from 'react';
import type { UsageGroup } from '@animaOS-SWARM/sdk';

import { costCell, formatTokens } from '../../lib/usage';

/** One breakdown of the Usage page. The group keys (provider and model
 *  names from config, session ids from the daemon) render as text nodes. */
export function UsageTable({
  title,
  keyHeading,
  rows,
  renderKey = (key) => key,
}: {
  title: string;
  keyHeading: string;
  rows: readonly UsageGroup[];
  renderKey?: (key: string) => ReactNode;
}) {
  if (rows.length === 0) return null;
  return (
    <section className="usage-table-section" aria-label={title}>
      <h3>{title}</h3>
      <table className="usage-table">
        <thead>
          <tr>
            <th scope="col">{keyHeading}</th>
            <th scope="col">Calls</th>
            <th scope="col">Tokens</th>
            <th scope="col">Cost</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((row) => (
            <tr key={row.key}>
              <th scope="row">{renderKey(row.key)}</th>
              <td>{row.totals.calls}</td>
              <td>{formatTokens(row.totals.totalTokens)}</td>
              <td>{costCell(row.totals)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  );
}
