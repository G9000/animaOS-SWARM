import { afterEach, describe, expect, it, vi } from 'vitest';

import { downloadText } from './download';

const originalCreate = URL.createObjectURL;
const originalRevoke = URL.revokeObjectURL;

afterEach(() => {
  URL.createObjectURL = originalCreate;
  URL.revokeObjectURL = originalRevoke;
  vi.restoreAllMocks();
  vi.useRealTimers();
});

describe('downloadText', () => {
  it('downloads text through a temporary link and revokes the url', async () => {
    vi.useFakeTimers();
    const create = vi.fn().mockReturnValue('blob:usage');
    const revoke = vi.fn();
    URL.createObjectURL = create;
    URL.revokeObjectURL = revoke;
    const clicked: HTMLAnchorElement[] = [];
    vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function (
      this: HTMLAnchorElement,
    ) {
      clicked.push(this);
    });

    downloadText('usage.csv', 'a,b\n', 'text/csv');

    expect(create).toHaveBeenCalledTimes(1);
    const blob = create.mock.calls[0][0] as Blob;
    expect(blob.type).toBe('text/csv;charset=utf-8');
    expect(await blob.text()).toBe('a,b\n');
    expect(clicked).toHaveLength(1);
    expect(clicked[0].download).toBe('usage.csv');
    expect(clicked[0].getAttribute('href')).toBe('blob:usage');
    expect(document.querySelector('a[download]')).toBeNull();
    expect(revoke).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1000);
    expect(revoke).toHaveBeenCalledWith('blob:usage');
  });
});
