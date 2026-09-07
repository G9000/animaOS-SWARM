import { useState } from 'react';
import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { WorkspaceBrief } from './WorkspaceBrief';

class Recognition {
  static instances: Recognition[] = [];
  continuous = false;
  interimResults = false;
  lang = '';
  onstart: (() => void) | null = null;
  onend: (() => void) | null = null;
  onerror: ((event: { error: string }) => void) | null = null;
  onresult:
    | ((event: {
        resultIndex: number;
        results: Array<{ isFinal: boolean; 0: { transcript: string } }>;
      }) => void)
    | null = null;
  start = vi.fn();
  stop = vi.fn();
  abort = vi.fn();
  constructor() {
    Recognition.instances.push(this);
  }
  result(parts: Array<[string, boolean]>, resultIndex = 0) {
    this.onresult?.({
      resultIndex,
      results: parts.map(([transcript, isFinal]) => ({
        isFinal,
        0: { transcript },
      })),
    });
  }
}

function Brief() {
  const [value, setValue] = useState('Build useful tools.');
  return <WorkspaceBrief value={value} onChange={setValue} />;
}

beforeEach(() => {
  Recognition.instances = [];
  vi.stubGlobal('SpeechRecognition', Recognition);
  vi.stubGlobal('webkitSpeechRecognition', undefined);
});
afterEach(() => vi.unstubAllGlobals());

it('appends each final result once and previews interim speech without overwriting typing', async () => {
  const user = userEvent.setup();
  render(<Brief />);
  await user.click(screen.getByRole('button', { name: 'Dictate brief' }));
  const recognition = Recognition.instances[0];
  expect(recognition.start).toHaveBeenCalledOnce();
  expect(recognition.continuous).toBe(true);
  expect(recognition.interimResults).toBe(true);
  expect(recognition.lang).toBe(navigator.language);
  act(() => recognition.onstart?.());
  act(() => recognition.result([['For small teams', false]]));
  expect(screen.getByText('For small teams')).toBeVisible();
  const field = screen.getByRole('textbox', { name: 'Workspace brief' });
  expect(field).toHaveValue('Build useful tools.');
  await user.type(field, ' Keep it simple.');
  act(() => recognition.result([['For small teams.', true]]));
  act(() =>
    recognition.result(
      [
        ['For small teams.', true],
        ['With clear goals.', true],
      ],
      0,
    ),
  );
  expect(field).toHaveValue(
    'Build useful tools. Keep it simple. For small teams. With clear goals.',
  );
});

it('stops listening and accepts the final result before returning to idle', async () => {
  const user = userEvent.setup();
  render(<Brief />);
  await user.click(screen.getByRole('button', { name: 'Dictate brief' }));
  const recognition = Recognition.instances[0];
  act(() => recognition.onstart?.());
  await user.click(screen.getByRole('button', { name: 'Stop dictation' }));
  expect(recognition.stop).toHaveBeenCalledOnce();
  act(() => recognition.result([['For creators.', true]]));
  act(() => recognition.onend?.());
  expect(screen.getByRole('textbox')).toHaveValue(
    'Build useful tools. For creators.',
  );
  expect(screen.getByRole('button', { name: 'Dictate brief' })).toBeEnabled();
});

it('handles denied microphone access and allows a retry', async () => {
  const user = userEvent.setup();
  render(<Brief />);
  await user.click(screen.getByRole('button', { name: 'Dictate brief' }));
  act(() => Recognition.instances[0].onerror?.({ error: 'not-allowed' }));
  expect(screen.getByRole('alert')).toHaveTextContent('microphone access');
  expect(screen.getByRole('textbox')).toHaveValue('Build useful tools.');
  await user.click(screen.getByRole('button', { name: 'Dictate brief' }));
  expect(Recognition.instances).toHaveLength(2);
});

it('aborts on unmount and ignores late speech from the old step', async () => {
  const user = userEvent.setup();
  const onChange = vi.fn();
  const { unmount } = render(
    <WorkspaceBrief value="Original" onChange={onChange} />,
  );
  await user.click(screen.getByRole('button', { name: 'Dictate brief' }));
  const recognition = Recognition.instances[0];
  const lateResult = recognition.onresult;
  unmount();
  expect(recognition.abort).toHaveBeenCalledOnce();
  act(() =>
    lateResult?.({
      resultIndex: 0,
      results: [{ isFinal: true, 0: { transcript: 'Late' } }],
    }),
  );
  expect(onChange).not.toHaveBeenCalled();
});

it('uses the prefixed browser API when available', async () => {
  vi.stubGlobal('SpeechRecognition', undefined);
  vi.stubGlobal('webkitSpeechRecognition', Recognition);
  render(<Brief />);
  await userEvent.click(screen.getByRole('button', { name: 'Dictate brief' }));
  expect(Recognition.instances).toHaveLength(1);
});

it('keeps typing available when the browser does not support dictation', async () => {
  vi.stubGlobal('SpeechRecognition', undefined);
  render(<Brief />);
  expect(screen.getByRole('button', { name: 'Dictate brief' })).toBeDisabled();
  expect(screen.getByText(/not supported in this browser/)).toBeVisible();
  await userEvent.type(screen.getByRole('textbox'), ' More detail.');
  expect(screen.getByRole('textbox')).toHaveValue(
    'Build useful tools. More detail.',
  );
});

it('cancels while microphone permission is pending and ignores a late start', async () => {
  render(<Brief />);
  await userEvent.click(screen.getByRole('button', { name: 'Dictate brief' }));
  const recognition = Recognition.instances[0];
  const lateStart = recognition.onstart;
  await userEvent.click(
    screen.getByRole('button', { name: 'Cancel dictation' }),
  );
  expect(recognition.abort).toHaveBeenCalledOnce();
  act(() => lateStart?.());
  expect(screen.getByRole('button', { name: 'Dictate brief' })).toBeEnabled();
});

it('recovers when the browser throws on start', async () => {
  class BrokenRecognition extends Recognition {
    override start = vi.fn(() => {
      throw new Error('not available');
    });
  }
  vi.stubGlobal('SpeechRecognition', BrokenRecognition);
  render(<Brief />);
  await userEvent.click(screen.getByRole('button', { name: 'Dictate brief' }));
  expect(screen.getByRole('alert')).toHaveTextContent('could not start');
  expect(screen.getByRole('button', { name: 'Dictate brief' })).toBeEnabled();
  expect(Recognition.instances[0].abort).toHaveBeenCalledOnce();
});

it('does not restart automatically after the browser ends recognition', async () => {
  render(<Brief />);
  await userEvent.click(screen.getByRole('button', { name: 'Dictate brief' }));
  const recognition = Recognition.instances[0];
  act(() => recognition.onstart?.());
  act(() => recognition.result([['Unconfirmed words', false]]));
  act(() => recognition.onend?.());
  expect(screen.queryByText('Unconfirmed words')).not.toBeInTheDocument();
  expect(screen.getByRole('textbox')).toHaveValue('Build useful tools.');
  expect(recognition.start).toHaveBeenCalledOnce();
  expect(screen.getByRole('button', { name: 'Dictate brief' })).toBeEnabled();
});
