import { getHostDefinition } from './hosts.js';

function parseHostArg(argv: string[]): string {
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

const host = getHostDefinition(parseHostArg(process.argv.slice(2)));

if (host.status !== 'placeholder') {
  throw new Error(
    `Host '${host.key}' is not a placeholder and should not use the placeholder entrypoint.`
  );
}

throw new Error(
  `Host '${host.key}' is registered as a placeholder and is not implemented yet.`
);
