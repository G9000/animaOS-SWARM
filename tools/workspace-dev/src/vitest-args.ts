export function normalizeVitestArgs(args: string[]): string[] {
  const normalized: string[] = [];

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (!arg) {
      continue;
    }

    if (arg === '--runInBand') {
      const nextArg = args[index + 1];
      if (nextArg === 'true' || nextArg === 'false') {
        index += 1;
      }
      continue;
    }

    if (arg.startsWith('--runInBand=')) {
      continue;
    }

    normalized.push(arg);
  }

  return normalized;
}
