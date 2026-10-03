/** One line of a draft's diff against the skill it would replace. */
export interface DiffLine {
  kind: 'same' | 'added' | 'removed';
  text: string;
}

/** The table a diff may fill (lines before × lines after); larger texts
 *  are shown side by side instead. */
export const MAX_DIFF_CELLS = 4_000_000;

/** A line diff by longest common subsequence, removals before additions;
 *  null when the texts are too large to compare. */
export function lineDiff(before: string, after: string): DiffLine[] | null {
  const a = before.split('\n');
  const b = after.split('\n');
  const width = b.length + 1;
  if ((a.length + 1) * width > MAX_DIFF_CELLS) return null;
  const common = new Uint32Array((a.length + 1) * width);
  for (let i = a.length - 1; i >= 0; i -= 1) {
    for (let j = b.length - 1; j >= 0; j -= 1) {
      common[i * width + j] =
        a[i] === b[j]
          ? common[(i + 1) * width + j + 1] + 1
          : Math.max(common[(i + 1) * width + j], common[i * width + j + 1]);
    }
  }
  const lines: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < a.length && j < b.length) {
    if (a[i] === b[j]) {
      lines.push({ kind: 'same', text: a[i] });
      i += 1;
      j += 1;
    } else if (common[(i + 1) * width + j] >= common[i * width + j + 1]) {
      lines.push({ kind: 'removed', text: a[i] });
      i += 1;
    } else {
      lines.push({ kind: 'added', text: b[j] });
      j += 1;
    }
  }
  for (; i < a.length; i += 1) lines.push({ kind: 'removed', text: a[i] });
  for (; j < b.length; j += 1) lines.push({ kind: 'added', text: b[j] });
  return lines;
}
