import type { DaemonSchedule } from '../lib/daemon-api';
import type { AgentDetail } from '../lib/types';
import { CheckinsView } from './CheckinsView';
import { ChipIcon, PulseIcon, SparkIcon } from './icons';
import { formatTokens } from './ui-bits';

export interface ActivityViewProps {
  agent: AgentDetail;
  checkins: DaemonSchedule[];
  prompt: string;
  setPrompt: (value: string) => void;
  intervalMin: number;
  setIntervalMin: (value: number) => void;
  addCheckin: () => void;
  removeCheckin: (id: string) => void;
  error: string | null;
  target: 'workspace' | 'telegram';
  setTarget: (value: 'workspace' | 'telegram') => void;
  telegramAvailable: boolean;
  busy: boolean;
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
      <div className="studio-page mx-auto w-full max-w-5xl">
        <p className="font-mono text-[10px] uppercase tracking-[0.16em] text-ink-3">
          THE BIGGER PICTURE
        </p>
        <h2 id="activity-heading" className="studio-page-title mt-3 text-ink">
          Activity
        </h2>
        <p className="studio-page-intro">
          A pulse on your progress. See what’s happening, and give your next
          good habit a little momentum.
        </p>
        <div className="mt-7 grid grid-cols-3 gap-3">
          {summaries.map((summary) => (
            <div key={summary.label} className="studio-stat glass">
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
        <div
          className="studio-token-breakdown"
          aria-label="Token usage breakdown"
        >
          <div>
            <span className="studio-note-label">UNDER THE HOOD</span>
            <h3>Every idea has an exchange.</h3>
            <p>
              Actual token usage from this agent. Includes conversation context
              and generated responses.
            </p>
          </div>
          <dl>
            <div>
              <dt>Input tokens</dt>
              <dd>{agent.token_usage.prompt_tokens.toLocaleString()}</dd>
            </div>
            <div>
              <dt>Output tokens</dt>
              <dd>{agent.token_usage.completion_tokens.toLocaleString()}</dd>
            </div>
          </dl>
        </div>
      </div>
      <CheckinsView {...props} />
    </section>
  );
}
