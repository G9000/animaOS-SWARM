/** Whether the current platform is a Mac, so a keyboard hint can show its
 *  ⌘ symbol; every other platform (including when `navigator` gives
 *  nothing useful, as in some test environments) shows Ctrl instead
 *  (spec §15.3, S3b-E). */
export function isMacPlatform(): boolean {
  if (typeof navigator === 'undefined') return false;
  const platform =
    (navigator as { userAgentData?: { platform?: string } }).userAgentData
      ?.platform ||
    navigator.platform ||
    navigator.userAgent ||
    '';
  return /mac|iphone|ipad|ipod/i.test(platform);
}
