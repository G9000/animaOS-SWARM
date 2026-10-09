import { memo } from 'react';
import type { LogLine as LogLineData } from '@animaOS-SWARM/sdk';

import { LOG_LEVEL_LABELS, formatLogTime } from '../../lib/logs';
import { RevealedText } from '../memory/RevealedText';

/** One daemon log line. The target and message can carry text a model or
 *  provider wrote, so both render as text through `RevealedText`; hidden
 *  characters show as markers (spec §14). Never markup. */
export const LogLine = memo(function LogLine({ line }: { line: LogLineData }) {
  return (
    <div className="system-log-row" data-level={line.level}>
      <time className="system-log-time">{formatLogTime(line.at)}</time>
      <span className={`system-log-level system-log-level-${line.level}`}>
        {LOG_LEVEL_LABELS[line.level] ?? line.level}
      </span>
      <RevealedText text={line.target} className="system-log-target" />
      <RevealedText text={line.message} className="system-log-message" />
    </div>
  );
});
