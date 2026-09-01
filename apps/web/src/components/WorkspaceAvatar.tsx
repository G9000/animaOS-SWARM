import { useEffect, useRef, useState } from 'react';

import { workspaceAvatarUrl } from '../lib/daemon-api';

const MAX_WORKSPACE_AVATAR_BYTES = 5 * 1024 * 1024;
const WORKSPACE_AVATAR_TYPES = new Set([
  'image/png',
  'image/jpeg',
  'image/webp',
]);

export interface WorkspaceAvatarProps {
  placement: 'sidebar' | 'mobile-bar';
  hasAvatar: boolean;
  uploadAvatar(file: File): Promise<void>;
}

function AgentOrb({ mobile }: { mobile: boolean }) {
  return (
    <>
      <span className="absolute inset-0 rounded-full border border-accent/20" />
      <span
        className={`absolute rounded-full border border-accent/25 bg-accent/[0.08] ${
          mobile ? 'inset-1' : 'inset-1.5'
        }`}
      />
      <span
        className={`agent-orb-core relative rounded-full ${
          mobile ? 'h-2 w-2' : 'h-3 w-3'
        }`}
      />
    </>
  );
}

export function WorkspaceAvatar({
  placement,
  hasAvatar,
  uploadAvatar,
}: WorkspaceAvatarProps) {
  const mobile = placement === 'mobile-bar';
  const inputRef = useRef<HTMLInputElement>(null);
  const previewRef = useRef<string | null>(null);
  const [previewUrl, setPreviewUrl] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);
  const [uploaded, setUploaded] = useState(false);
  const [uploading, setUploading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [imageFailed, setImageFailed] = useState(false);

  const releasePreview = () => {
    const current = previewRef.current;
    previewRef.current = null;
    if (current) URL.revokeObjectURL(current);
  };

  useEffect(
    () => () => {
      releasePreview();
    },
    [],
  );

  useEffect(() => {
    setImageFailed(false);
    if (!hasAvatar) setUploaded(false);
  }, [hasAvatar]);

  const chooseFile = async (file: File | undefined) => {
    if (!file) return;
    setError(null);

    if (!WORKSPACE_AVATAR_TYPES.has(file.type)) {
      setError('Choose a PNG, JPEG, or WebP image.');
      if (inputRef.current) inputRef.current.value = '';
      return;
    }
    if (file.size > MAX_WORKSPACE_AVATAR_BYTES) {
      setError('Choose an image no larger than 5 MiB.');
      if (inputRef.current) inputRef.current.value = '';
      return;
    }

    releasePreview();
    const nextPreview = URL.createObjectURL(file);
    previewRef.current = nextPreview;
    setPreviewUrl(nextPreview);
    setImageFailed(false);
    setUploading(true);
    try {
      await uploadAvatar(file);
      setUploaded(true);
      setRevision((current) => current + 1);
      setImageFailed(false);
    } catch (uploadError) {
      setError(
        uploadError instanceof Error
          ? uploadError.message
          : String(uploadError),
      );
    } finally {
      releasePreview();
      setPreviewUrl(null);
      setUploading(false);
      if (inputRef.current) inputRef.current.value = '';
    }
  };

  const showImage =
    previewUrl !== null || ((hasAvatar || uploaded) && !imageFailed);

  return (
    <div className="relative shrink-0">
      <button
        type="button"
        aria-label="Change workspace avatar"
        aria-busy={uploading}
        disabled={uploading}
        onClick={() => inputRef.current?.click()}
        className={`group relative flex shrink-0 items-center justify-center rounded-full focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent disabled:cursor-wait ${
          mobile ? 'h-8 w-8' : 'h-11 w-11'
        }`}
      >
        {showImage ? (
          <img
            src={previewUrl ?? workspaceAvatarUrl(revision)}
            alt=""
            onError={() => setImageFailed(true)}
            className="h-full w-full rounded-full object-cover"
          />
        ) : (
          <AgentOrb mobile={mobile} />
        )}
        <span
          aria-hidden
          className="pointer-events-none absolute inset-0 flex items-center justify-center rounded-full bg-black/45 text-white opacity-0 transition group-hover:opacity-100 group-focus-visible:opacity-100"
        >
          <svg
            width={mobile ? 12 : 14}
            height={mobile ? 12 : 14}
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <path d="M14.5 4 16 7h3a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V9a2 2 0 0 1 2-2h3l1.5-3h5Z" />
            <circle cx="12" cy="13" r="3" />
          </svg>
        </span>
      </button>
      <input
        ref={inputRef}
        type="file"
        accept=".png,.jpg,.jpeg,.webp,image/png,image/jpeg,image/webp"
        aria-label="Workspace avatar image file"
        tabIndex={-1}
        className="sr-only"
        onChange={(event) => void chooseFile(event.currentTarget.files?.[0])}
      />
      {error ? (
        <p
          role="alert"
          aria-live="polite"
          className="absolute left-0 top-full z-40 mt-2 w-56 rounded-lg border border-danger/25 bg-panel px-3 py-2 text-xs text-danger shadow-xl"
        >
          {error}
        </p>
      ) : null}
    </div>
  );
}
