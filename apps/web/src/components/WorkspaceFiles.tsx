import { useEffect, useRef, useState } from 'react';
import type { WorkspaceFileResponse, WorkspaceFilesResponse } from '@animaOS-SWARM/sdk';
import { daemon } from '../lib/daemon-api';

const message = (error: unknown) => error instanceof Error ? error.message : String(error);
function sizeLabel(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export function WorkspaceFiles({ online }: { online: boolean }) {
  const [listing, setListing] = useState<WorkspaceFilesResponse | null>(null);
  const [listError, setListError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [refresh, setRefresh] = useState(0);
  const [query, setQuery] = useState('');
  const [selected, setSelected] = useState<string | null>(null);
  const [preview, setPreview] = useState<WorkspaceFileResponse | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [reading, setReading] = useState(false);
  const request = useRef(0);

  useEffect(() => {
    let active = true;
    request.current++;
    setSelected(null);
    setPreview(null);
    setPreviewError(null);
    setReading(false);
    if (!online) { setLoading(false); return; }
    setLoading(true);
    setListError(null);
    daemon.listWorkspaceFiles().then((result) => {
      if (active) setListing(result);
    }).catch((error) => {
      if (active) setListError(message(error));
    }).finally(() => { if (active) setLoading(false); });
    return () => { active = false; request.current++; };
  }, [online, refresh]);

  const read = async (path: string) => {
    if (!online) return;
    const id = ++request.current;
    setSelected(path);
    setPreview(null);
    setPreviewError(null);
    setReading(true);
    try {
      const result = await daemon.readWorkspaceFile(path);
      if (id === request.current) setPreview(result);
    } catch (error) {
      if (id === request.current) setPreviewError(message(error));
    } finally {
      if (id === request.current) setReading(false);
    }
  };
  const files = listing?.files.filter((file) => file.path.toLowerCase().includes(query.trim().toLowerCase())) ?? [];

  return <section className="mx-auto h-full w-full max-w-7xl space-y-6 overflow-y-auto p-4 pb-28 sm:p-7 md:pb-7" aria-labelledby="workspace-files-heading">
    <div className="flex flex-wrap items-start justify-between gap-4">
      <div><p className="text-xs font-medium uppercase tracking-widest text-ink-3">Workspace</p>
        <h1 id="workspace-files-heading" className="mt-2 font-display text-3xl font-semibold text-ink">Files</h1>
        <p className="mt-2 text-sm text-ink-2">Browse your workspace and preview text files.</p>
      </div>
      <button type="button" className="rounded-xl border border-line px-4 py-2 text-sm text-ink disabled:opacity-50"
        disabled={!online || loading} onClick={() => setRefresh((value) => value + 1)}>Refresh files</button>
    </div>
    {!online ? <p role="status" className="rounded-2xl border border-line p-6 text-sm text-ink-2">Connect to the daemon to browse workspace files.</p> : <>
      <label className="block text-sm text-ink-2">Search files
        <input type="search" className="field mt-2 w-full" value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Search by name or path" />
      </label>
      {loading && <p role="status" className="text-sm text-ink-3">Loading files…</p>}
      {listError && <p role="alert" className="rounded-xl border border-danger/30 p-4 text-sm text-danger">{listError}{listing ? ' Showing the last loaded file list.' : ''}</p>}
      {listing?.truncated && <p className="text-sm text-ink-3">The file list is truncated. Search filters only the files shown here.</p>}
      <div className="grid min-w-0 gap-5 lg:grid-cols-[minmax(240px,1fr)_minmax(0,2fr)]">
        <div className="min-w-0 overflow-hidden rounded-2xl border border-line">
          <div className="border-b border-line px-4 py-3 text-xs text-ink-3">{files.length} {files.length === 1 ? 'file' : 'files'} shown</div>
          <div className="max-h-[55vh] overflow-y-auto">
            {files.map((file) => <button key={file.path} type="button" aria-pressed={selected === file.path}
              onClick={() => void read(file.path)}
              className={`block w-full border-b border-line p-4 text-left last:border-0 ${selected === file.path ? 'bg-accent/10' : 'hover:bg-white/[0.04]'}`}>
              <span className="block break-words text-sm font-medium text-ink">{file.name}</span>
              <span className="mt-1 block break-all text-xs text-ink-3">{file.path}</span>
              <span className="mt-2 block text-xs text-ink-3">{sizeLabel(file.sizeBytes)} · {file.modifiedAtMs === null ? 'Date unavailable' : new Date(file.modifiedAtMs).toLocaleString()}</span>
            </button>)}
            {!loading && !listError && !files.length && <p className="p-6 text-sm text-ink-3">{listing?.files.length ? 'No files match your search.' : 'No workspace files yet.'}</p>}
          </div>
        </div>
        <section aria-label="File preview" className="min-w-0 rounded-2xl border border-line">
          <div className="flex flex-wrap justify-between gap-2 border-b border-line p-4">
            <h2 className="break-all text-sm font-medium text-ink">{selected ?? 'File preview'}</h2>
            <span className="text-xs text-ink-3">Read only</span>
          </div>
          {reading && <p role="status" className="p-6 text-sm text-ink-3">Loading preview…</p>}
          {previewError && <div className="space-y-3 p-6"><p role="alert" className="text-sm text-danger">{previewError}</p>
            <button type="button" className="text-sm text-accent" onClick={() => selected && void read(selected)}>Retry preview</button></div>}
          {!selected && <p className="p-6 text-sm text-ink-3">Select a file to read its contents.</p>}
          {preview && <>{preview.truncated && <p className="border-b border-line p-4 text-sm text-ink-3">This preview is truncated. Open the file locally to see its full contents.</p>}
            <pre className="max-h-[65vh] overflow-auto whitespace-pre-wrap break-words p-5 font-mono text-xs leading-relaxed text-ink-2">{preview.content || '(Empty file)'}</pre></>}
        </section>
      </div>
    </>}
  </section>;
}
