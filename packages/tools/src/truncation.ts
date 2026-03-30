// @animaOS-SWARM/tools — truncation.ts
// Smart output truncation to prevent context window blowups.
//
// Features:
//  - Context-aware middle truncation: keeps error-relevant lines (stderr markers)
//  - Per-tool limits instead of one-size-fits-all
//  - Overflow files auto-expire (no separate cleanup command needed)
//  - Composable: truncateOutput() wraps chars + lines in one call

import { mkdirSync, writeFileSync, readdirSync, statSync, unlinkSync } from "node:fs";
import { join } from "node:path";
import { homedir } from "node:os";
import { randomUUID } from "node:crypto";

// ---------- Per-tool limits ----------

export const LIMITS = {
  bash: { chars: 30_000, lines: 500 },
  read_file: { chars: 200_000, lines: 2_000, charsPerLine: 2_000 },
  grep: { chars: 10_000, lines: 200 },
  glob: { items: 2_000 },
  list_dir: { items: 1_000 },
  default: { chars: 30_000, lines: 500 },
} as const;

export type ToolLimitKey = keyof typeof LIMITS;

// ---------- Overflow files ----------

const OVERFLOW_MAX_AGE_MS = 4 * 60 * 60 * 1000; // 4 hours

function overflowDir(): string {
  return join(homedir(), ".animaos-swarm", "overflow");
}

/** Write full output to disk, return the path. */
export function writeOverflow(content: string, toolName: string): string {
  const dir = overflowDir();
  mkdirSync(dir, { recursive: true });

  // Auto-expire old files opportunistically
  try { cleanupOverflow(); } catch { /* best-effort */ }

  const file = join(dir, `${toolName}-${randomUUID().slice(0, 8)}.txt`);
  writeFileSync(file, content, "utf-8");
  return file;
}

/** Remove overflow files older than OVERFLOW_MAX_AGE_MS. */
export function cleanupOverflow(): number {
  const dir = overflowDir();
  let deleted = 0;
  let entries: string[];
  try { entries = readdirSync(dir); } catch { return 0; }

  const now = Date.now();
  for (const f of entries) {
    try {
      const p = join(dir, f);
      if (now - statSync(p).mtimeMs > OVERFLOW_MAX_AGE_MS) {
        unlinkSync(p);
        deleted++;
      }
    } catch { /* skip */ }
  }
  return deleted;
}

// ---------- Core truncation ----------

export interface TruncateResult {
  content: string;
  truncated: boolean;
  overflowPath?: string;
  originalChars: number;
  originalLines: number;
}

export interface TruncateOpts {
  /** Max characters. */
  maxChars?: number;
  /** Max lines. */
  maxLines?: number;
  /** Max chars per individual line. */
  maxCharsPerLine?: number;
  /** Tool name (for overflow filename + notice). */
  toolName?: string;
  /** Write full output to overflow file on truncation. Default true. */
  overflow?: boolean;
  /** Error-relevant patterns to preserve during middle-truncation. */
  errorPatterns?: RegExp[];
}

const DEFAULT_ERROR_PATTERNS = [
  /error/i,
  /ERR!/,
  /failed/i,
  /exception/i,
  /panic/i,
  /FAIL/,
  /TypeError/,
  /SyntaxError/,
  /ReferenceError/,
  /^\s*at\s+/,   // stack trace lines
  /^\s*\^/,      // caret pointers
];

/**
 * Smart truncation with context-aware middle omission.
 *
 * Instead of naively keeping head+tail, we scan the omitted middle for
 * lines matching error patterns and preserve those too. This means
 * stack traces buried in verbose output still show up.
 */
export function truncateOutput(text: string, opts: TruncateOpts = {}): TruncateResult {
  const {
    maxChars = LIMITS.default.chars,
    maxLines = LIMITS.default.lines,
    maxCharsPerLine,
    toolName = "output",
    overflow = true,
    errorPatterns = DEFAULT_ERROR_PATTERNS,
  } = opts;

  const originalChars = text.length;
  const allLines = text.split("\n");
  const originalLines = allLines.length;

  let truncated = false;
  let overflowPath: string | undefined;

  // 1) Per-line char truncation
  let lines = allLines;
  if (maxCharsPerLine) {
    lines = lines.map((line) => {
      if (line.length > maxCharsPerLine) {
        truncated = true;
        return line.slice(0, maxCharsPerLine) + "... [line truncated]";
      }
      return line;
    });
  }

  // 2) Line count truncation — context-aware middle omission
  if (lines.length > maxLines) {
    truncated = true;
    const headCount = Math.floor(maxLines * 0.4);
    const tailCount = Math.floor(maxLines * 0.4);
    const errorBudget = maxLines - headCount - tailCount; // ~20% for error lines

    const head = lines.slice(0, headCount);
    const tail = lines.slice(-tailCount);
    const middle = lines.slice(headCount, lines.length - tailCount);

    // Scan middle for error-relevant lines
    const errorLines: string[] = [];
    for (const line of middle) {
      if (errorLines.length >= errorBudget) break;
      if (errorPatterns.some((pat) => pat.test(line))) {
        errorLines.push(line);
      }
    }

    const omitted = middle.length - errorLines.length;
    const marker = `\n... [${omitted.toLocaleString()} lines omitted] ...\n`;

    lines = errorLines.length > 0
      ? [...head, marker, `  -- error-relevant lines from omitted section --`, ...errorLines, `  ------------------------------------------------`, ...tail]
      : [...head, marker, ...tail];
  }

  let content = lines.join("\n");

  // 3) Final char cap (safety net after line truncation)
  if (content.length > maxChars) {
    truncated = true;
    const half = Math.floor(maxChars / 2);
    const charOmitted = content.length - maxChars;
    content = content.slice(0, half) + `\n... [${charOmitted.toLocaleString()} chars omitted] ...\n` + content.slice(-half);
  }

  // 4) Overflow file
  if (truncated && overflow) {
    try {
      overflowPath = writeOverflow(text, toolName);
    } catch { /* best-effort */ }
  }

  // 5) Truncation notice
  if (truncated) {
    const notices: string[] = [];
    notices.push(`[Truncated: ${originalChars.toLocaleString()} chars / ${originalLines.toLocaleString()} lines -> ${content.length.toLocaleString()} chars]`);
    if (overflowPath) {
      notices.push(`[Full output: ${overflowPath}]`);
    }
    content += `\n\n${notices.join(" ")}`;
  }

  return { content, truncated, overflowPath, originalChars, originalLines };
}

/**
 * Truncate an array of items (file paths, dir entries, etc.)
 */
export function truncateItems<T>(
  items: T[],
  maxItems: number,
  format: (items: T[]) => string,
  itemType = "items",
  toolName = "output",
): TruncateResult {
  const originalChars = 0; // not meaningful for arrays
  const originalLines = items.length;

  if (items.length <= maxItems) {
    return { content: format(items), truncated: false, originalChars, originalLines };
  }

  const half = Math.floor(maxItems / 2);
  const selected = [...items.slice(0, half), ...items.slice(-half)];
  const omitted = items.length - maxItems;

  let content = format(selected);
  content += `\n... [${omitted.toLocaleString()} ${itemType} omitted] ...`;

  let overflowPath: string | undefined;
  try {
    overflowPath = writeOverflow(format(items), toolName);
  } catch { /* best-effort */ }

  const notices = [`[Truncated: ${maxItems} of ${items.length} ${itemType}]`];
  if (overflowPath) notices.push(`[Full output: ${overflowPath}]`);
  content += `\n${notices.join(" ")}`;

  return { content, truncated: true, overflowPath, originalChars, originalLines };
}
