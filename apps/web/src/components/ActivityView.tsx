import type { Checkin } from '../lib/checkins';
import type { AgentDetail } from '../lib/types';
import { CheckinsView } from './CheckinsView';
import { ChipIcon, PulseIcon, SparkIcon } from './icons';
import { formatTokens } from './ui-bits';

export interface ActivityViewProps {
  agent: AgentDetail;
  checkins: Checkin[];
  prompt: string;
  setPrompt: (value: string) => void;
  intervalMin: number;
  setIntervalMin: (value: number) => void;
  addCheckin: () => void;
  removeCheckin: (id: string) => void;
  error: string | null;
}

export function ActivityView(props: ActivityViewProps) {
  const { agent, checkins } = props;
  const summaries = [
    {
      label: 'Messages',
      value: agent.messages.length,
      icon: <SparkIcon size={14} />,
    },
    {
      label: 'Tokens',
      value: formatTokens(agent.token_usage.total_tokens),
      icon: <ChipIcon size={14} />,
    },
    {
      label: 'Check-ins',
      value: checkins.length,
      icon: <PulseIcon size={14} />,
    },
  ];

  return (
    <section
      className="h-full overflow-y-auto"
      aria-labelledby="activity-heading"
    >
      <div className="mx-auto w-full max-w-4xl px-4 pt-7 sm:px-6">
        <p className="font-mono text-[10px] uppercase tracking-[0.16em] text-ink-3">
          Existing workspace history
        </p>
        <h2
          id="activity-heading"
          className="mt-1 font-display text-2xl font-semibold tracking-tight text-ink"
        >
          Activity
        </h2>
        <div className="mt-5 grid grid-cols-3 gap-2">
          {summaries.map((summary) => (
            <div
              key={summary.label}
              className="glass rounded-xl px-3 py-3 sm:px-4"
            >
              <div className="flex items-center gap-1.5 text-mint">
                {summary.icon}
                <span className="font-mono text-[9px] uppercase tracking-wider text-ink-3">
                  {summary.label}
                </span>
              </div>
              <p className="mt-1 font-mono text-sm font-semibold text-ink">
                {summary.value}
              </p>
            </div>
          ))}
        </div>
      </div>
      <CheckinsView {...props} />
    </section>
  );
}
