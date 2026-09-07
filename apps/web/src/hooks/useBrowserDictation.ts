import { useEffect, useRef, useState } from 'react';

// Speech recognition is not declared by every version of TypeScript's DOM lib.
interface RecognitionResult {
  isFinal: boolean;
  readonly [index: number]: { transcript: string };
}
interface Recognition {
  continuous: boolean;
  interimResults: boolean;
  lang: string;
  onstart: (() => void) | null;
  onend: (() => void) | null;
  onerror: ((event: { error: string }) => void) | null;
  onresult:
    | ((event: {
        resultIndex: number;
        results: ArrayLike<RecognitionResult>;
      }) => void)
    | null;
  start(): void;
  stop(): void;
  abort(): void;
}
type RecognitionConstructor = new () => Recognition;

function browserRecognition(): RecognitionConstructor | undefined {
  if (typeof window === 'undefined') return undefined;
  const browser = window as Window & {
    SpeechRecognition?: RecognitionConstructor;
    webkitSpeechRecognition?: RecognitionConstructor;
  };
  return browser.SpeechRecognition ?? browser.webkitSpeechRecognition;
}

function detach(recognition: Recognition) {
  recognition.onstart = null;
  recognition.onresult = null;
  recognition.onerror = null;
  recognition.onend = null;
}

function abort(recognition: Recognition) {
  detach(recognition);
  try {
    recognition.abort();
  } catch {
    /* Already ended by the browser. */
  }
}

function errorMessage(code: string): string {
  switch (code) {
    case 'not-allowed':
    case 'service-not-allowed':
      return 'Allow microphone access and speech recognition in your browser, then try again.';
    case 'audio-capture':
      return 'No microphone is available. Connect a microphone and try again.';
    case 'no-speech':
      return 'No speech was detected. Try again when you’re ready.';
    case 'network':
      return 'The browser’s speech service could not connect. Check your connection and try again.';
    case 'language-not-supported':
      return 'Your browser’s speech service does not support the current browser language.';
    default:
      return 'Dictation could not continue. You can try again or keep typing.';
  }
}

export function useBrowserDictation(onTranscript: (text: string) => void) {
  const [state, setState] = useState<
    'idle' | 'starting' | 'listening' | 'stopping'
  >('idle');
  const [interim, setInterim] = useState('');
  const [error, setError] = useState<string | null>(null);
  const current = useRef<Recognition | null>(null);
  const onTranscriptRef = useRef(onTranscript);
  onTranscriptRef.current = onTranscript;

  useEffect(
    () => () => {
      const recognition = current.current;
      current.current = null;
      if (recognition) abort(recognition);
    },
    [],
  );

  const start = () => {
    const Constructor = browserRecognition();
    if (!Constructor || current.current) return;
    setError(null);
    setInterim('');
    try {
      const recognition = new Constructor();
      current.current = recognition;
      recognition.continuous = true;
      recognition.interimResults = true;
      recognition.lang = navigator.language || 'en-US';
      const committed = new Set<number>();
      recognition.onstart = () => {
        if (current.current === recognition) setState('listening');
      };
      recognition.onresult = (event) => {
        if (current.current !== recognition) return;
        const final: string[] = [];
        const preview: string[] = [];
        for (let index = 0; index < event.results.length; index++) {
          const result = event.results[index];
          const text = result[0]?.transcript.trim();
          if (result.isFinal) {
            if (!committed.has(index)) {
              committed.add(index);
              if (text) final.push(text);
            }
          } else if (text) preview.push(text);
        }
        if (final.length) onTranscriptRef.current(final.join(' '));
        setInterim(preview.join(' '));
      };
      recognition.onend = () => {
        if (current.current !== recognition) return;
        current.current = null;
        detach(recognition);
        setState('idle');
        setInterim('');
      };
      recognition.onerror = (event) => {
        if (current.current !== recognition) return;
        current.current = null;
        abort(recognition);
        setState('idle');
        setInterim('');
        if (event.error !== 'aborted') setError(errorMessage(event.error));
      };
      setState('starting');
      recognition.start();
    } catch {
      const recognition = current.current;
      current.current = null;
      if (recognition) abort(recognition);
      setState('idle');
      setError(
        'Dictation could not start. Check microphone access and try again.',
      );
    }
  };

  const stop = () => {
    const recognition = current.current;
    if (!recognition) return;
    if (state === 'starting') {
      current.current = null;
      abort(recognition);
      setState('idle');
      return;
    }
    setState('stopping');
    try {
      recognition.stop();
    } catch {
      current.current = null;
      abort(recognition);
      setState('idle');
      setInterim('');
    }
  };

  return {
    supported: Boolean(browserRecognition()),
    state,
    interim,
    error,
    start,
    stop,
  };
}
