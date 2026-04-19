import { afterEach, describe, expect, it, vi } from 'vitest';

import {
  parseHostArg,
  parseModeArg,
  runPlaceholderHost,
} from './placeholder-host.js';

describe('placeholder host entrypoint', () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('defaults to dev mode', () => {
    expect(parseModeArg(['--host', 'elixir'])).toBe('dev');
  });

  it('parses explicit placeholder modes', () => {
    expect(parseModeArg(['--host', 'python', '--mode', 'build'])).toBe(
      'build'
    );
    expect(parseModeArg(['--host', 'python', '--mode', 'test'])).toBe('test');
    expect(parseModeArg(['--host', 'python', '--mode', 'lint'])).toBe('lint');
  });

  it('parses the host flag', () => {
    expect(parseHostArg(['--host', 'elixir'])).toBe('elixir');
  });

  it('fails dev mode with the placeholder message', () => {
    expect(() => runPlaceholderHost(['--host', 'elixir'])).toThrowError(
      "Host 'elixir' is registered as a placeholder and is not implemented yet."
    );
  });

  it('treats build/test/lint as intentional no-ops', () => {
    const log = vi.spyOn(console, 'log').mockImplementation(() => {});

    expect(() =>
      runPlaceholderHost(['--host', 'elixir', '--mode', 'build'])
    ).not.toThrow();
    expect(() =>
      runPlaceholderHost(['--host', 'python', '--mode', 'test'])
    ).not.toThrow();
    expect(() =>
      runPlaceholderHost(['--host', 'python', '--mode', 'lint'])
    ).not.toThrow();

    expect(log).toHaveBeenCalledTimes(3);
    expect(log).toHaveBeenNthCalledWith(
      1,
      "Host 'elixir' is registered as a placeholder; build is a no-op until the host is implemented."
    );
    expect(log).toHaveBeenNthCalledWith(
      2,
      "Host 'python' is registered as a placeholder; test is a no-op until the host is implemented."
    );
    expect(log).toHaveBeenNthCalledWith(
      3,
      "Host 'python' is registered as a placeholder; lint is a no-op until the host is implemented."
    );
  });
});
