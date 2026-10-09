/** Saves `text` as a file through a temporary link; the object URL is
 *  revoked after the click. */
export function downloadText(
  filename: string,
  text: string,
  mime: string,
): void {
  let url: string | undefined;
  try {
    url = URL.createObjectURL(
      new Blob([text], { type: `${mime};charset=utf-8` }),
    );
    const link = document.createElement('a');
    link.href = url;
    link.download = filename;
    document.body.appendChild(link);
    link.click();
    link.remove();
  } finally {
    if (url) {
      const revoke = url;
      window.setTimeout(() => URL.revokeObjectURL(revoke), 1000);
    }
  }
}
