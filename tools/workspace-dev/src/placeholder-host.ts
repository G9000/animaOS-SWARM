import { getHostDefinition } from './hosts.js';

export type PlaceholderHostMode = 'dev' | 'build' | 'test' | 'lint';

export function parseHostArg(argv: string[]): string {
  const hostFlagIndex = argv.findIndex((value) => value === '--host');
  const nextValue =
    hostFlagIndex >= 0 ? argv[hostFlagIndex + 1] : undefined;

  if (!nextValue) {
    throw new Error(
      "Missing required '--host <name>' argument for placeholder host."
    );
  }

  return nextValue;
}

export function parseModeArg(argv: string[]): PlaceholderHostMode {
  const modeFlagIndex = argv.findIndex((value) => value === '--mode');
  const nextValue =
    modeFlagIndex >= 0 ? argv[modeFlagIndex + 1] : undefined;

  if (!nextValue) {
    return 'dev';
  }

  if (
    nextValue !== 'dev' &&
    nextValue !== 'build' &&
    nextValue !== 'test' &&
    nextValue !== 'lint'
  ) {
    throw new Error(
      `Unsupported placeholder host mode '${nextValue}'. Expected one of: dev, build, test, lint.`
    );
  }

  return nextValue;
}

export function runPlaceholderHost(argv: string[]): void {
  const host = getHostDefinition(parseHostArg(argv));
  const mode = parseModeArg(argv);

  if (host.status !== 'placeholder') {
    throw new Error(
      `Host '${host.key}' is not a placeholder and should not use the placeholder entrypoint.`
    );
  }

  if (mode === 'dev') {
    throw new Error(
      `Host '${host.key}' is registered as a placeholder and is not implemented yet.`
    );
  }

  console.log(
    `Host '${host.key}' is registered as a placeholder; ${mode} is a no-op until the host is implemented.`
  );
}

if (import.meta.main) {
  runPlaceholderHost(process.argv.slice(2));
}
