// @animaOS-SWARM/tools — edit-hints.ts
// Helpers for the edit and multi-edit tools: line ending normalization,
// over-escape auto-fix, and diagnostic error messages.

/** Normalize \r\n to \n. Apply to both file content and search strings before matching. */
export function normalizeLineEndings(s: string): string {
  return s.replace(/\r\n/g, "\n");
}

/**
 * Fix common LLM over-escaping: \\n -> \n, \\t -> \t, etc.
 * Conservative — only handles patterns LLMs commonly produce.
 */
export function unescapeOverEscaped(s: string): string {
  return s
    .replace(/\\\\(?=[ntrfv'"`])/g, "__BACKSLASH_ESCAPE__")  // protect real \\n
    .replace(/\\n/g, "\n")
    .replace(/\\t/g, "\t")
    .replace(/\\r/g, "\r")
    .replace(/\\"/g, '"')
    .replace(/\\'/g, "'")
    .replace(/\\`/g, "`")
    .replace(/__BACKSLASH_ESCAPE__/g, "\\");
}

/**
 * Build a diagnostic error message when old_string isn't found in a file.
 * Checks for common causes in order: smart quotes, whitespace mismatch, then fallback.
 */
export function buildNotFoundError(
  filePath: string,
  oldString: string,
  fileContent: string,
): string {
  // 1. Smart quote mismatch
  if (hasSmartQuoteMismatch(oldString, fileContent)) {
    return `old_string not found in ${filePath}. The file uses smart/curly quotes but old_string has straight quotes. Re-read the file and copy the exact characters.`;
  }

  // 2. Whitespace mismatch
  if (hasWhitespaceMismatch(oldString, fileContent)) {
    return `old_string not found in ${filePath}. Found a near-match with different whitespace or indentation. Re-read the file for exact content.`;
  }

  // 3. Fallback
  return `old_string not found in ${filePath}. The file may have changed -- re-read it and try again.`;
}

/** Check if replacing straight quotes with curly equivalents produces a match. */
function hasSmartQuoteMismatch(search: string, content: string): boolean {
  // Only check if search contains straight quotes
  if (!/['"]/.test(search)) return false;

  const straightSingle = /'/g;
  const straightDouble = /"/g;

  const variants = [
    // Most common: right/closing curly quotes
    search.replace(straightSingle, "\u2019").replace(straightDouble, "\u201D"),
    // Opening curly quotes
    search.replace(straightSingle, "\u2018").replace(straightDouble, "\u201C"),
    // Mixed: opening " (left) and closing ' (right)
    search.replace(straightSingle, "\u2019").replace(straightDouble, "\u201C"),
    // Only single quote substitutions
    search.replace(straightSingle, "\u2019"),
    search.replace(straightSingle, "\u2018"),
    // Only double quote substitutions
    search.replace(straightDouble, "\u201C"),
    search.replace(straightDouble, "\u201D"),
  ];

  // Also try a fuzzy approach: normalize both to ASCII and check structural match
  const normalizeQuotes = (s: string) =>
    s
      .replace(/[\u2018\u2019]/g, "'")
      .replace(/[\u201C\u201D]/g, '"');

  if (normalizeQuotes(content).includes(normalizeQuotes(search))) {
    // There's a match after normalizing — content likely has curly quotes
    if (/[\u2018\u2019\u201C\u201D]/.test(content)) return true;
  }

  return variants.some((v) => content.includes(v));
}

/** Collapse all whitespace runs to single space, then compare. */
function hasWhitespaceMismatch(search: string, content: string): boolean {
  const collapse = (s: string) => s.replace(/\s+/g, " ").trim();
  const collapsedSearch = collapse(search);
  if (collapsedSearch.length < 10) return false; // too short to be meaningful
  return collapse(content).includes(collapsedSearch);
}
