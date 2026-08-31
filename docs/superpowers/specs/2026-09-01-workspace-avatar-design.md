# Workspace Avatar Design

## Goal

Let a user replace the animated orb at the top of the web client's workspace sidebar with a workspace-specific image. The image must survive browser reloads, remain available when the web client is closed, and travel with the workspace rather than living in browser storage.

## Scope

The existing avatar position in `AgentPresence` becomes an accessible change control on desktop and in the compact mobile presence bar. Desktop sidebar editing is the requested surface; matching mobile behavior was included in the approved design because both placements already share `AgentPresence`. Activating either control opens a local file picker for PNG, JPEG, or WebP images up to 5 MiB (5,242,880 bytes). A selected image is displayed immediately with a circular `object-fit: cover` treatment. The current animated orb remains the fallback when no valid image is configured.

This change does not add image cropping, generated avatars, remote image URLs, avatar removal, per-agent avatars, or onboarding fields. Those can be separate follow-ups if product needs justify them.

## Architecture

The Rust daemon owns avatar persistence and delivery. The web client never stores image bytes in local storage or global application state.

Add two workspace routes:

- `PUT /api/workspace/avatar` accepts raw image bytes with an image content type, validates them, atomically replaces the workspace avatar, and returns `204 No Content`.
- `GET /api/workspace/avatar` returns the current bytes with the detected content type and `Cache-Control: no-store`, or `404` when no valid avatar exists.

The daemon writes a fixed `assets/workspace-avatar` file below the configured workspace root. The fixed, extensionless name prevents user-controlled filenames and path traversal, avoids stale files when formats change, and makes the asset portable without adding binary data to `anima.yaml` or the control-plane snapshot. The daemon detects the format from the file signature whenever it serves the asset.

`WorkspaceConfigResponse` adds a required boolean `hasAvatar` field. A configured GET response therefore has the shape `{"configured":true,"workspace":{"rootPath":"...","companyName":"...","mission":"...","values":[],"hasAvatar":true},"defaultRoot":"..."}`. When the daemon is not configured, the existing `workspace: null` shape remains; there is no top-level `hasAvatar` field. Bootstrap, validation, and resume responses use the same extended nested workspace shape.

`hasAvatar` is true only when the conventional file exists, is readable, is within the size limit, and has a supported signature. It is derived instead of persisted separately, so copying or resuming a workspace naturally discovers a valid image. Missing, unreadable, oversized, or invalid conventional files report `hasAvatar: false`; `GET /api/workspace/avatar` treats the same cases as absent and returns `404`, preserving the orb fallback.

The TypeScript daemon client adds focused helpers for reading the avatar URL and uploading a `File`. `ViewHarness` owns the short-lived upload state and refreshes workspace state after a successful upload. `WorkspaceShell` passes avatar state and an upload callback to `AgentPresence`; the presence component owns only rendering, file selection, and local preview lifecycle.

## Upload and Display Flow

1. The user activates the current orb or image with a pointer or keyboard.
2. A hidden file input accepts `.png`, `.jpg`, `.jpeg`, and `.webp`.
3. The component rejects an unsupported browser-reported type or a file larger than 5 MiB before transmission.
4. The component creates an object URL and shows it immediately while the upload is pending.
5. The client sends the original bytes to `PUT /api/workspace/avatar` with the file's content type.
6. The daemon validates the byte signature and size, creates `assets/` if needed, writes a temporary file in that directory, and atomically replaces `workspace-avatar`.
7. On `204`, the web client refreshes workspace state, revokes the object URL, increments a component-owned revision value, and renders `/api/workspace/avatar?v=<revision>`. The revision is initialized for each page load; `Cache-Control: no-store` on GET prevents a stale image after reload or an external file replacement.
8. On failure, the component revokes the object URL, restores the prior image or orb, and exposes the daemon error beside the control.

The upload control remains usable after success so the avatar can be changed repeatedly. After every attempt, the component clears the hidden file input so selecting the same file triggers another change event. While an upload is active it is disabled and announces its busy state.

## Validation and Safety

The server accepts only PNG, JPEG, and WebP signatures and does not trust the request content type or original filename. Empty bodies, mismatched or unsupported bytes, oversized bodies, and uploads without a configured workspace return structured API errors. A route-specific `MAX_WORKSPACE_AVATAR_BYTES = 5 * 1024 * 1024` limit is passed to the existing `read_limited_body` helper; the daemon-wide `max_request_bytes` limit remains unchanged for ordinary JSON routes. The route uses the daemon's existing workspace/control-plane mutation serialization so two replacements cannot interleave.

The destination path is constructed solely from the validated configured workspace root and constant path segments. The write uses a temporary sibling plus atomic rename, ensuring a failed upload cannot corrupt the last valid avatar. GET revalidates the stored signature before responding and treats a missing asset as `404`.

## Responsive and Accessible UI

The desktop 44-pixel avatar and mobile 32-pixel avatar retain their existing footprint, so sidebar and mobile-bar layout do not shift. When an image exists it fills the circle and is clipped by the existing rounded shape. A subtle hover/focus overlay communicates that the image is editable without permanently covering it.

The control has the accessible name `Change workspace avatar`, supports normal button keyboard activation, exposes `aria-busy` during upload, and keeps the hidden input out of the tab order. Validation or upload failures use an adjacent live error message. The image is decorative because the workspace and main-agent names are already visible beside it.

## Error Handling

Client-side type and size failures do not call the daemon. Server rejection, network failure, or persistence failure restores the last confirmed avatar and leaves the picker available for retry. A failed atomic replacement preserves the previous file. If the configured file is removed outside the app, the next workspace refresh reports `hasAvatar: false` and the animated orb returns.

## Testing

Rust route tests will cover:

- upload and retrieval of representative PNG, JPEG, and WebP signatures;
- rejection of empty, unsupported, mismatched, and oversized bodies;
- rejection when no workspace is configured;
- replacement of an existing avatar without exposing partial bytes;
- correct response content type and `404` behavior; and
- `hasAvatar` discovery from the conventional workspace asset after state restoration or resume.

Web tests will cover:

- the existing orb fallback when `hasAvatar` is false;
- rendering the daemon avatar when `hasAvatar` is true;
- pointer and keyboard activation of the file picker;
- immediate preview and busy state;
- client-side type and size rejection without an API call;
- successful upload state refresh and repeat replacement; and
- failed upload rollback, object URL cleanup, and accessible error output.

Verification will run focused web tests during red-green development, then `bun x nx run @animaOS-SWARM/web:test --skipNxCache`, `bun x nx run @animaOS-SWARM/web:typecheck --skipNxCache`, and `bun x nx run rust-daemon:test --skipNxCache`. If Windows locks the normal Rust target directory, validation will use the repository's documented isolated `CARGO_TARGET_DIR` fallback. The live client at `http://localhost:4200/` will then be reloaded to verify the desktop sidebar and compact mobile presentation.

## Acceptance Criteria

1. The desktop sidebar avatar and compact mobile avatar can open a local image picker using pointer or keyboard input.
2. A valid PNG, JPEG, or WebP image up to 5 MiB replaces the orb immediately and remains after a page reload.
3. The image is stored under the configured workspace's `assets/` directory and is served by the daemon.
4. Invalid or failed uploads preserve the last confirmed avatar and show an accessible error.
5. A workspace without an avatar continues to render the current animated orb without layout changes.
6. Copying or resuming a workspace containing the conventional avatar asset discovers it without browser-local state.
7. Relevant web and Rust daemon tests pass in the current tree.
