import { readFileSync, readdirSync } from 'node:fs';
import { dirname, extname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

const sourceRoot = dirname(fileURLToPath(import.meta.url));

function productionSources(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);

    if (entry.isDirectory()) return productionSources(path);
    if (!['.css', '.tsx'].includes(extname(entry.name))) return [];
    if (entry.name.includes('.test.')) return [];

    return [path];
  });
}

function read(relativePath: string): string {
  return readFileSync(join(sourceRoot, relativePath), 'utf8');
}

function between(source: string, start: string, end: string): string {
  const startIndex = source.indexOf(start);
  const endIndex = source.indexOf(end, startIndex + start.length);

  expect(startIndex).toBeGreaterThanOrEqual(0);
  expect(endIndex).toBeGreaterThan(startIndex);

  return source.slice(startIndex, endIndex);
}

describe('Neon Rose spatial visual contract', () => {
  const styles = read('styles.css');
  const production = productionSources(sourceRoot)
    .map((path) => `${path}\n${readFileSync(path, 'utf8')}`)
    .join('\n');

  it('defines the approved palette anchors exactly', () => {
    expect(styles).toContain('#090A0F');
    expect(styles).toContain('#17171D');
    expect(styles).toContain('#FF397F');
    expect(styles).toContain('#64DFAD');
  });

  it('contains no legacy blue or purple accent tokens in production sources', () => {
    expect(production).not.toMatch(/\b(?:sky|purple|violet)-/i);
    expect(production).not.toMatch(/#(?:38bdf8|a78bfa|7dd3fc|0ea5e9)/i);
    expect(production).not.toMatch(
      /rgba?\(\s*(?:56\s*,\s*189\s*,\s*248|167\s*,\s*139\s*,\s*250|125\s*,\s*211\s*,\s*252)/i,
    );
  });

  it('exposes focus, safe-area, responsive shell, and motion semantics', () => {
    expect(styles).toContain(':focus-visible');
    expect(styles).toContain('--safe-area-composer');
    expect(styles).toContain('--safe-area-navigation');
    expect(styles).toContain('@media (prefers-reduced-motion: reduce)');
    expect(styles).toContain('.agent-orb');

    expect(read('components/WorkspaceShell.tsx')).toContain(
      'data-placement={placement}',
    );
    expect(read('components/WorkspaceShell.tsx')).toContain("'top-shell'");
    expect(read('components/WorkspaceShell.tsx')).toContain("'bottom-dock'");
    expect(read('components/SettingsPanel.tsx')).toContain(
      'data-surface="settings-sheet-drawer"',
    );
    expect(production).toContain('data-motion="agent-orb"');
  });

  it('keeps unknown connection feedback neutral', () => {
    const connecting = between(
      read('ViewHarness.tsx'),
      'function ConnectingState()',
      'function OfflineRetry',
    );

    expect(connecting).toContain('bg-ink-3');
    expect(connecting).not.toMatch(
      /(?:bg|text|shadow)-accent|255,\s*57,\s*127/,
    );
    expect(connecting).not.toMatch(/(?:bg|text|shadow)-mint/);
  });

  it('keeps ordinary conversation surfaces neutral until interaction', () => {
    const chat = read('components/ChatScreen.tsx');
    const bubble = between(chat, 'function Bubble', 'const SUGGESTIONS');
    const suggestions = between(
      chat,
      '{SUGGESTIONS.map',
      'function ThinkingIndicator',
    );

    expect(bubble).toContain('bg-panel-2/90');
    expect(bubble).not.toContain('accent');
    expect(suggestions).toContain('text-ink-3');
    expect(suggestions).not.toContain('className="text-accent"');
    expect(suggestions).toContain('group-hover:text-accent');
    expect(suggestions).toContain('group-focus-visible:text-accent');
  });

  it('keeps onboarding decorative copy neutral', () => {
    const onboarding = read('components/onboarding/OnboardingFlow.tsx');
    const header = between(
      onboarding,
      '<header className="space-y-2 text-center">',
      '<OnboardingProgress',
    );

    expect(header).toContain('text-ink-3');
    expect(header).not.toContain('text-accent');
  });
});
