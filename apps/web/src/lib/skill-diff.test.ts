import { describe, expect, it } from 'vitest';

import { MAX_DIFF_CELLS, lineDiff } from './skill-diff';

describe('lineDiff', () => {
  it('marks the lines kept, removed, and added', () => {
    expect(lineDiff('a\nb\nc', 'a\nB\nc\nd')).toEqual([
      { kind: 'same', text: 'a' },
      { kind: 'removed', text: 'b' },
      { kind: 'added', text: 'B' },
      { kind: 'same', text: 'c' },
      { kind: 'added', text: 'd' },
    ]);
    expect(lineDiff('same', 'same')).toEqual([{ kind: 'same', text: 'same' }]);
    expect(lineDiff('', 'new')).toEqual([
      { kind: 'removed', text: '' },
      { kind: 'added', text: 'new' },
    ]);
  });

  it('gives up on texts too large to compare', () => {
    const lines = Math.ceil(Math.sqrt(MAX_DIFF_CELLS));
    const big = Array.from(
      { length: lines },
      (_, index) => `line ${index}`,
    ).join('\n');
    expect(lineDiff(big, `${big}\nmore`)).toBeNull();
  });
});
