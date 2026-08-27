import { readFileSync, readdirSync } from 'node:fs';
import { dirname, extname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

const sourceRoot = dirname(fileURLToPath(import.meta.url));

function productionSources(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);

    if (entry.isDirectory()) return productionSources(path);
    if (!['.css', '.ts', '.tsx'].includes(extname(entry.name))) return [];
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

function cssHexToken(styles: string, token: string): string {
  const value = styles.match(
    new RegExp(`--color-${token}:\\s*(#[0-9a-f]{6})`, 'i'),
  )?.[1];
  expect(value).toBeDefined();
  return value ?? '#000000';
}

function relativeLuminance(hex: string): number {
  const channels = hex
    .slice(1)
    .match(/.{2}/g)
    ?.map((channel) => Number.parseInt(channel, 16) / 255)
    .map((channel) =>
      channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4,
    );
  if (!channels) throw new Error(`Invalid hex color: ${hex}`);
  return channels[0] * 0.2126 + channels[1] * 0.7152 + channels[2] * 0.0722;
}

function contrastRatio(foreground: string, background: string): number {
  const lighter = Math.max(
    relativeLuminance(foreground),
    relativeLuminance(background),
  );
  const darker = Math.min(
    relativeLuminance(foreground),
    relativeLuminance(background),
  );
  return (lighter + 0.05) / (darker + 0.05);
}

describe('Neon Rose spatial visual contract', () => {
  const styles = read('styles.css');
  const productionPaths = [
    ...productionSources(sourceRoot),
    ...productionSources(resolve(sourceRoot, '../../../packages/ui/src')),
    resolve(sourceRoot, '../index.html'),
  ];
  const production = productionPaths
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

  it('keeps core text and primary action pairs at AA contrast', () => {
    const abyss = cssHexToken(styles, 'abyss');
    const panel = cssHexToken(styles, 'panel');
    const accent = cssHexToken(styles, 'accent');

    for (const [foreground, background] of [
      [cssHexToken(styles, 'ink'), abyss],
      [cssHexToken(styles, 'ink-2'), abyss],
      [cssHexToken(styles, 'ink-3'), abyss],
      [cssHexToken(styles, 'ink-3'), panel],
      [abyss, accent],
    ]) {
      expect(contrastRatio(foreground, background)).toBeGreaterThanOrEqual(4.5);
    }
    expect(production).not.toMatch(/bg-accent[^'"\r\n]*text-white/);
    expect(read('components/ui-bits.tsx')).toMatch(
      /bg-accent[^'"\r\n]*text-abyss/,
    );
  });

  it('owns viewport and bottom safe areas without double-padding', () => {
    const index = readFileSync(resolve(sourceRoot, '../index.html'), 'utf8');
    const composer = between(styles, '.safe-composer {', '.safe-bottom-dock');
    const orbPulse = between(styles, '@keyframes orb-pulse', '.animate-orb');

    expect(index).toContain('viewport-fit=cover');
    expect(read('app/app.tsx')).toContain('app-viewport');
    expect(styles).toMatch(
      /\.app-viewport\s*{[^}]*height:\s*100vh;[^}]*height:\s*100dvh;/s,
    );
    expect(styles).toContain('.safe-settings-sheet');
    expect(read('components/SettingsPanel.tsx')).toContain(
      'safe-settings-sheet',
    );
    expect(styles.match(/env\(safe-area-inset-bottom/g)).toHaveLength(1);
    expect(composer).not.toContain('safe-area-inset-bottom');
    expect(composer).not.toContain('--safe-area-');
    expect(orbPulse).not.toContain('filter:');
  });

  it('uses one shared Neon Rose RGB token for authored glow values', () => {
    expect(styles).toContain('--color-accent-rgb: 255 57 127;');
    expect(production).not.toMatch(
      /rgba?\(\s*255\s*,\s*57\s*,\s*127(?:\s*,|\s*\))/i,
    );
  });

  it('exposes focus, safe-area, responsive shell, and motion semantics', () => {
    expect(styles).toContain(':focus-visible');
    expect(styles).toContain('--safe-area-top');
    expect(styles).toContain('--safe-area-bottom');
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

  it('keeps ordinary conversation surfaces and secondary hover neutral', () => {
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
    expect(suggestions).toContain('hover:border-line-strong');
    expect(suggestions).toContain('hover:bg-white/[0.035]');
    expect(suggestions).toContain('hover:shadow-black/30');
    expect(suggestions).toContain('group-hover:text-ink-2');
    expect(suggestions).not.toContain('accent');
    expect(suggestions).not.toMatch(/255,\s*57,\s*127/);
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
