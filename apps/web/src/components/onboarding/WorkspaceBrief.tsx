import { useRef, type RefObject } from 'react';
import { useBrowserDictation } from '../../hooks/useBrowserDictation';
import { labelCls } from '../ui-bits';

export function WorkspaceBrief({
  value,
  onChange,
  inputRef,
}: {
  value: string;
  onChange(value: string): void;
  inputRef?: RefObject<HTMLTextAreaElement | null>;
}) {
  const latestValue = useRef(value);
  latestValue.current = value;
  const change = (text: string) => {
    latestValue.current = text;
    onChange(text);
  };
  const dictation = useBrowserDictation((text) => {
    const previous = latestValue.current;
    change(`${previous}${previous && !/\s$/.test(previous) ? ' ' : ''}${text}`);
  });
  const active = dictation.state !== 'idle';

  return (
    <div>
      <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
        <label htmlFor="onboarding-mission" className={labelCls}>
          Workspace brief
        </label>
        <button
          type="button"
          onClick={active ? dictation.stop : dictation.start}
          disabled={!dictation.supported || dictation.state === 'stopping'}
          aria-pressed={active}
          className={`inline-flex items-center gap-2 rounded-xl border px-3 py-2 text-sm font-medium transition disabled:opacity-50 ${active ? 'border-accent bg-accent/10 text-accent' : 'border-line text-ink-2 hover:border-line-strong hover:text-ink'}`}
        >
          <svg
            aria-hidden="true"
            width="16"
            height="16"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.7"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <rect x="9" y="2" width="6" height="12" rx="3" />
            <path d="M5 10v2a7 7 0 0 0 14 0v-2M12 19v3M8 22h8" />
          </svg>
          {dictation.state === 'starting'
            ? 'Cancel dictation'
            : dictation.state === 'stopping'
              ? 'Finishing dictation…'
              : active
                ? 'Stop dictation'
                : 'Dictate brief'}
        </button>
      </div>
      <textarea
        ref={inputRef}
        id="onboarding-mission"
        className="field min-h-48 resize-y leading-relaxed"
        rows={7}
        aria-describedby="onboarding-brief-help onboarding-dictation-help"
        value={value}
        onChange={(event) => change(event.target.value)}
        autoComplete="off"
        placeholder="Describe what you do, who you serve, and what you want to achieve. Include the content or services you need, channels, workflows, brand voice, constraints, and examples."
      />
      <p id="onboarding-dictation-help" className="mt-2 text-xs text-ink-3">
        {dictation.supported
          ? 'Dictation uses your browser’s speech service and language. Confirmed speech is added to your brief.'
          : 'Voice dictation is not supported in this browser. You can still type your brief.'}
      </p>
      {active && (
        <div
          role="status"
          className="mt-2 rounded-xl border border-accent/25 bg-accent/5 p-3 text-sm text-ink-2"
        >
          {dictation.state === 'starting'
            ? 'Waiting for microphone access…'
            : dictation.state === 'stopping'
              ? 'Finishing the last words…'
              : 'Listening…'}
          {dictation.interim && (
            <p className="mt-1 break-words">{dictation.interim}</p>
          )}
        </div>
      )}
      {dictation.error && (
        <p role="alert" className="mt-2 text-sm text-danger">
          {dictation.error}
        </p>
      )}
      <p
        id="onboarding-brief-help"
        className="mt-3 text-sm leading-relaxed text-ink-3"
      >
        Describe the first useful result you want, who it is for, and what a good
        outcome looks like. Include constraints and context your team should keep
        in mind. Paragraphs and bullet points are welcome.
      </p>
    </div>
  );
}
