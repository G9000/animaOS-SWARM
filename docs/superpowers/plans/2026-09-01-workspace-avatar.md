# Workspace Avatar Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let users replace the web sidebar's animated orb with a validated, daemon-persisted workspace avatar.

**Architecture:** The Rust daemon validates and atomically stores one conventional `assets/workspace-avatar` file and serves it through dedicated workspace routes. The web client adds a focused `WorkspaceAvatar` presentation/upload component, while existing shell/controller layers pass only `hasAvatar` and an upload callback.

**Tech Stack:** Rust, Axum, atomicwrites, React, TypeScript, Vitest, Testing Library, Nx/Bun.

---

## Execution constraint

This plan is executed inline in the current checkout at the user's request. Several target files already contain unrelated uncommitted sidebar and calendar work, including `AgentPresence.tsx`, `WorkspaceShell.tsx`, their tests, and `routes/mod.rs`. Preserve those edits exactly. Use focused diffs and test checkpoints; do not create implementation commits that would capture unrelated hunks.

## File map

- Modify `hosts/rust-daemon/src/routes/workspace.rs`: image signature validation, conventional path inspection, atomic storage, GET/PUT handlers, and unit tests.
- Modify `hosts/rust-daemon/src/routes/contracts/workspace.rs`: add `hasAvatar` to the nested workspace response contract.
- Modify `hosts/rust-daemon/src/routes/mod.rs`: register/document raw avatar routes, enforce the route-specific body cap, build binary responses, and add router tests.
- Modify `apps/web/src/lib/daemon-api.ts`: expose `hasAvatar`, avatar URL creation, and raw-file upload.
- Modify `apps/web/src/lib/daemon-api.test.ts`: prove exact upload transport and 204 handling.
- Create `apps/web/src/components/WorkspaceAvatar.tsx`: accessible picker, preview, validation, fallback, cache revision, and error lifecycle.
- Create `apps/web/src/components/WorkspaceAvatar.test.tsx`: focused behavior tests for the component.
- Modify `apps/web/src/components/AgentPresence.tsx`: replace the private orb renderer with `WorkspaceAvatar` in both placements.
- Modify `apps/web/src/components/WorkspaceShell.tsx`: derive avatar availability and pass upload behavior.
- Modify `apps/web/src/components/WorkspaceShell.test.tsx`: integration assertions without disturbing the in-progress sidebar tests.
- Modify `apps/web/src/ViewHarness.tsx`: connect the daemon upload method to the shell.
- Modify `apps/web/src/ViewHarness.test.tsx`: verify the controller wiring through a successful selection.

### Task 1: Daemon avatar storage and workspace discovery

**Files:**
- Modify: `hosts/rust-daemon/src/routes/workspace.rs`
- Modify: `hosts/rust-daemon/src/routes/contracts/workspace.rs`

- [ ] **Step 1: Write failing Rust unit tests for supported signatures and discovery**

Extend `workspace.rs` tests with temporary workspace helpers and cases equivalent to:

```rust
#[test]
fn detects_supported_workspace_avatar_formats() {
    assert_eq!(detect_avatar_media_type(PNG_BYTES), Some("image/png"));
    assert_eq!(detect_avatar_media_type(JPEG_BYTES), Some("image/jpeg"));
    assert_eq!(detect_avatar_media_type(WEBP_BYTES), Some("image/webp"));
    assert_eq!(detect_avatar_media_type(b"not-an-image"), None);
}

#[test]
fn config_response_reports_only_valid_conventional_avatar() {
    let root = unique_temp_workspace("avatar-discovery");
    let config = workspace_config(&root);
    assert!(!config_response(&config).has_avatar);

    write_avatar(&root, PNG_BYTES);
    assert!(config_response(&config).has_avatar);

    write_avatar(&root, b"invalid");
    assert!(!config_response(&config).has_avatar);
}
```

Also cover an oversized conventional file and ensure test cleanup removes only the unique temp directory it created.

- [ ] **Step 2: Run the focused Rust tests and verify RED**

Name both tests with the common `workspace_avatar_` prefix, then run:

```powershell
bun x nx run rust-daemon:test --skipNxCache -- workspace_avatar_
```

Expected: FAIL because the detection helpers and `has_avatar` field do not exist.

- [ ] **Step 3: Add the response field and minimal avatar helpers**

In `contracts/workspace.rs`, extend the serialized nested response:

```rust
pub(crate) struct WorkspaceConfigResponse {
    pub(crate) root_path: String,
    pub(crate) company_name: String,
    pub(crate) mission: String,
    pub(crate) values: Vec<String>,
    pub(crate) has_avatar: bool,
}
```

In `workspace.rs`, add constants and focused helpers:

```rust
pub(super) const MAX_WORKSPACE_AVATAR_BYTES: usize = 5 * 1024 * 1024;
const WORKSPACE_AVATAR_RELATIVE_PATH: [&str; 2] = ["assets", "workspace-avatar"];

fn workspace_avatar_path(root: &Path) -> PathBuf {
    root.join(WORKSPACE_AVATAR_RELATIVE_PATH[0])
        .join(WORKSPACE_AVATAR_RELATIVE_PATH[1])
}

fn detect_avatar_media_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}
```

Implement `inspect_workspace_avatar(root)` so it rejects missing, unreadable, empty, oversized, or unsupported files. Check `metadata.len()` against the 5 MiB cap before opening the file and read only enough header bytes for discovery. Set `has_avatar` from this helper inside `config_response`. Prove discovery after restoration by creating a second `WorkspaceConfig` for the same temp root after the avatar is written and asserting its response also reports `has_avatar: true`; this models the control-plane restore/resume path because availability is deliberately derived rather than persisted.

- [ ] **Step 4: Run the focused Rust tests and verify GREEN**

Run the same focused Nx command. Expected: PASS.

- [ ] **Step 5: Write failing unit tests for write/read handlers**

Add async tests proving:

```rust
// Valid bytes + matching declared type create assets/workspace-avatar.
handle_put_workspace_avatar(PNG_BYTES.to_vec(), Some("image/png"), &state)
    .await
    .unwrap();
let avatar = handle_get_workspace_avatar(&state).await.unwrap();
assert_eq!(avatar.content_type, "image/png");
assert_eq!(avatar.bytes, PNG_BYTES);

// Repeat equivalent PUT/GET round trips for JPEG and WebP.
// A second successful upload replaces the first and GET returns only new bytes.
// Invalid bytes, mismatched type, empty bytes, no configured workspace,
// and >5 MiB input fail without replacing a previously valid file.
// Externally placed invalid and oversized conventional files make GET return 404.
```

- [ ] **Step 6: Run the handler tests and verify RED**

Expected: FAIL because the handlers and `WorkspaceAvatar` result type do not exist.

- [ ] **Step 7: Implement atomic put/get handlers**

Add:

```rust
pub(crate) struct WorkspaceAvatar {
    pub(crate) bytes: Vec<u8>,
    pub(crate) content_type: &'static str,
}

pub(crate) async fn handle_put_workspace_avatar(
    body: Vec<u8>,
    declared_content_type: Option<&str>,
    state: &SharedDaemonState,
) -> Result<(), ApiError> { /* validate, create assets, AtomicFile AllowOverwrite */ }

pub(crate) async fn handle_get_workspace_avatar(
    state: &SharedDaemonState,
) -> Result<WorkspaceAvatar, ApiError> { /* read, validate, return bytes/type */ }
```

Copy the configured root while holding the read lock, then release it before filesystem work. If no workspace exists, return `409 Conflict` with `workspace is not configured`. Validate the detected type against `image/png`, `image/jpeg`, or `image/webp`; do not use a request filename. Use `AtomicFile::new(path, AllowOverwrite).write(...)` so replacement is atomic on Windows. GET must check file metadata before allocation, then read through `File::take((MAX_WORKSPACE_AVATAR_BYTES + 1) as u64)` and reject if the bounded result exceeds the cap. Missing, invalid, unreadable, and oversized conventional files return `404`; other persistence failures return service unavailable.

- [ ] **Step 8: Run all `workspace.rs` tests and verify GREEN**

Run:

```powershell
bun x nx run rust-daemon:test --skipNxCache -- routes::workspace::tests
```

Expected: PASS.

- [ ] **Step 9: Record a clean task checkpoint**

Run `git diff --check` and inspect only the two task files. Do not commit because the checkout contains unrelated overlapping work.

### Task 2: Daemon HTTP contract and route-specific upload limit

**Files:**
- Modify: `hosts/rust-daemon/src/routes/mod.rs`
- Test: `hosts/rust-daemon/src/routes/mod.rs`

- [ ] **Step 1: Write failing router tests**

Add tests using the existing `custom_router` helper and a uniquely named temp workspace. Cover:

```rust
// GET /api/workspace returns workspace.hasAvatar false, then true after upload.
// PUT accepts a valid image larger than DaemonConfig::default().max_request_bytes.
// PUT returns 204 and stores exact bytes.
// GET returns exact bytes, detected Content-Type, and Cache-Control: no-store.
// Invalid content type/signature and >5 MiB return JSON errors.
// GET with missing, invalid, or oversized conventional assets returns 404.
```

Use a syntactically valid PNG-signature payload of roughly 65 KiB to prove the route does not inherit the 64 KiB JSON limit.

- [ ] **Step 2: Run the focused router tests and verify RED**

Run:

```powershell
bun x nx run rust-daemon:test --skipNxCache -- workspace_avatar
```

Expected: FAIL with 404/missing route and missing `hasAvatar` JSON.

- [ ] **Step 3: Register and document the routes**

Add `get_workspace_avatar_entry` and `put_workspace_avatar_entry` to the OpenAPI path list and register:

```rust
.route(
    "/api/workspace/avatar",
    get(get_workspace_avatar_entry).put(put_workspace_avatar_entry),
)
```

The PUT entry must:

```rust
let content_type = request.headers()
    .get(header::CONTENT_TYPE)
    .and_then(|value| value.to_str().ok())
    .map(str::to_owned);
let body = match read_limited_body(
    request,
    workspace::MAX_WORKSPACE_AVATAR_BYTES + 1,
).await {
    Ok(body) => body,
    Err(response) => return response,
};
if body.len() > workspace::MAX_WORKSPACE_AVATAR_BYTES {
    return ApiError::bad_request_static("workspace avatar exceeds 5 MiB")
        .into_response();
}
let _transaction = state.agent_runs.control_plane_transaction().await;
match workspace::handle_put_workspace_avatar(
    body,
    content_type.as_deref(),
    &state.daemon,
).await {
    Ok(()) => StatusCode::NO_CONTENT.into_response(),
    Err(error) => error.into_response(),
}
```

Do not alter `DaemonConfig::max_request_bytes` or the body limits of JSON routes.

The GET entry builds a binary response with the detected content type and `Cache-Control: no-store`. OpenAPI documents `200`, `404`, and `503`; PUT documents `204`, `400`, `409` with `workspace is not configured`, and `503`.

- [ ] **Step 4: Run the focused router tests and verify GREEN**

Expected: PASS.

- [ ] **Step 5: Run formatting and the workspace route test group**

Run the confirmed repository target:

```powershell
bun x nx run rust-daemon:lint --skipNxCache
```

Then re-run `workspace_avatar` and workspace route tests.

- [ ] **Step 6: Record a clean task checkpoint**

Run `git diff --check`. Inspect only avatar hunks in `routes/mod.rs` because calendar work already modifies this file. Do not stage or commit unrelated content.

### Task 3: Web daemon client contract

**Files:**
- Modify: `apps/web/src/lib/daemon-api.ts`
- Modify: `apps/web/src/lib/daemon-api.test.ts`

- [ ] **Step 1: Write failing client tests**

Extend the workspace response fixture with `hasAvatar: false` and add:

```typescript
it('uploads the workspace avatar as raw bytes and accepts 204', async () => {
  const fetchMock = vi.fn<typeof fetch>().mockResolvedValue(
    new Response(null, { status: 204 }),
  );
  vi.stubGlobal('fetch', fetchMock);
  const file = new File([PNG_BYTES], 'avatar.png', { type: 'image/png' });

  await daemon.uploadWorkspaceAvatar(file);

  expect(fetchMock).toHaveBeenCalledWith('/api/workspace/avatar',
    expect.objectContaining({ method: 'PUT', body: file,
      headers: expect.objectContaining({ 'content-type': 'image/png' }) }));
});

it('builds a cache-busted workspace avatar URL', () => {
  expect(workspaceAvatarUrl(3)).toBe('/api/workspace/avatar?v=3');
});
```

- [ ] **Step 2: Run focused client tests and verify RED**

Run:

```powershell
bun x nx run @animaOS-SWARM/web:test --skipNxCache -- src/lib/daemon-api.test.ts
```

Expected: FAIL because `hasAvatar`, URL helper, and upload method do not exist.

- [ ] **Step 3: Implement minimal raw-upload support**

Add `hasAvatar: boolean` to `DaemonWorkspaceConfig`, export:

```typescript
export const workspaceAvatarUrl = (revision: number) =>
  `/api/workspace/avatar?v=${revision}`;
```

Keep mutation inputs separate from response-only fields:

```typescript
export interface WorkspaceConfigInput {
  rootPath: string;
  companyName: string;
  mission: string;
  values: string[];
}

export interface DaemonWorkspaceConfig extends WorkspaceConfigInput {
  hasAvatar: boolean;
}
```

The shared request helper already returns `undefined` for status 204; keep the new test as a regression assertion rather than changing that logic. Add:

```typescript
uploadWorkspaceAvatar: (file: File) =>
  request<void>('/workspace/avatar', {
    method: 'PUT',
    headers: { 'content-type': file.type },
    body: file,
  }),
```

The explicit image header must override the helper's JSON default.

- [ ] **Step 4: Run focused client tests and verify GREEN**

Expected: PASS.

- [ ] **Step 5: Record a clean task checkpoint**

Run `git diff --check`; no implementation commit in the dirty inline checkout.

### Task 4: Accessible avatar picker component

**Files:**
- Create: `apps/web/src/components/WorkspaceAvatar.tsx`
- Create: `apps/web/src/components/WorkspaceAvatar.test.tsx`

- [ ] **Step 1: Write failing fallback and existing-image tests**

Render the component with `placement="sidebar"`, `hasAvatar={false}`, and a no-op upload function. Assert the `Change workspace avatar` button exists, has the 44-pixel footprint, and contains the current orb rather than an image. Render with `hasAvatar` true and assert a decorative image uses `/api/workspace/avatar?v=0` and `object-cover`.

- [ ] **Step 2: Run the component test and verify RED**

Run:

```powershell
bun x nx run @animaOS-SWARM/web:test --skipNxCache -- src/components/WorkspaceAvatar.test.tsx
```

Expected: FAIL because the component does not exist.

- [ ] **Step 3: Implement the fallback rendering and picker shell**

Create a focused component with props:

```typescript
interface WorkspaceAvatarProps {
  placement: 'sidebar' | 'mobile-bar';
  hasAvatar: boolean;
  uploadAvatar(file: File): Promise<void>;
}
```

Use a real button with `aria-label="Change workspace avatar"`, `aria-busy`, and a ref-driven hidden file input accepting `.png,.jpg,.jpeg,.webp`. The button's native click and Space/Enter activation must call the input's `.click()`; test both pointer click and keyboard activation. Keep the existing nested orb rings/core as the fallback. Render the image with empty alt text and the placement-specific 44/32-pixel dimensions. Add a subtle camera/edit glyph overlay that becomes visible on `group-hover` and `group-focus-visible` without covering the identity text.

- [ ] **Step 4: Run the first component tests and verify GREEN**

Expected: PASS.

- [ ] **Step 5: Write failing validation, preview, success, and rollback tests**

Stub `URL.createObjectURL` and `URL.revokeObjectURL`. Assert:

- unsupported type and a `5 * 1024 * 1024 + 1` byte file show an adjacent `role="alert"`/`aria-live="polite"` error without calling `uploadAvatar`;
- valid selection immediately renders the object URL and exposes busy state;
- resolved upload swaps to `/api/workspace/avatar?v=1`, revokes the preview URL, clears the error, and permits another selection of the same file;
- rejected upload restores the previous confirmed image/orb, revokes the preview, and shows the error;
- an image `error` event returns to the orb fallback; and
- starting a new preview and completing a later valid upload clear `imageFailed`, so recovery renders the new image.

- [ ] **Step 6: Run the new tests and verify RED**

Expected: FAIL on the first unimplemented lifecycle behavior.

- [ ] **Step 7: Implement the minimal upload lifecycle**

Use local `previewUrl`, `revision`, `uploading`, `error`, and `imageFailed` state. Validate the browser-reported MIME and exact 5 MiB cap before creating a preview. In `finally`, clear `input.value`; revoke every object URL on replacement/unmount. Clear `imageFailed` when a new valid preview begins and again after a successful replacement. After success increment the revision and consider the server image available even before shared workspace state updates. On failure keep the last confirmed state.

- [ ] **Step 8: Run the full component test and verify GREEN**

Expected: PASS with no React act warnings or leaked object URLs.

- [ ] **Step 9: Record a clean task checkpoint**

Run `git diff --check`. New files may remain uncommitted with the rest of the inline implementation.

### Task 5: Integrate the picker into the sidebar, mobile bar, and controller

**Files:**
- Modify: `apps/web/src/components/AgentPresence.tsx`
- Modify: `apps/web/src/components/WorkspaceShell.tsx`
- Modify: `apps/web/src/components/WorkspaceShell.test.tsx`
- Modify: `apps/web/src/ViewHarness.tsx`
- Modify: `apps/web/src/ViewHarness.test.tsx`

- [ ] **Step 1: Write failing shell integration tests**

Extend the configured workspace fixture with `hasAvatar`. Assert the sidebar and mobile presence each expose one `Change workspace avatar` button, the configured state renders the daemon image, and uploading calls the callback passed to `WorkspaceShell`.

- [ ] **Step 2: Run the focused shell test and verify RED**

Run:

```powershell
bun x nx run @animaOS-SWARM/web:test --skipNxCache -- src/components/WorkspaceShell.test.tsx
```

Expected: FAIL because shell/presence do not accept avatar props.

- [ ] **Step 3: Thread narrow props through existing components**

Add to `WorkspaceShell`:

```typescript
onChangeWorkspaceAvatar: (file: File) => Promise<void>;
```

Derive `hasAvatar` only from `workspaceState?.configured && workspaceState.workspace?.hasAvatar === true`. Pass both values to desktop and mobile `AgentPresence`. Replace `AgentOrb` in `AgentPresence` with `WorkspaceAvatar` while preserving the user's current sidebar identity/layout changes.

- [ ] **Step 4: Connect `ViewHarness` to the daemon client**

Create a `useCallback` in `ViewHarness` that performs the required shared-state refresh:

```tsx
const changeWorkspaceAvatar = useCallback(async (file: File) => {
  await daemon.uploadWorkspaceAvatar(file);
  await refreshWorkspace();
}, [refreshWorkspace]);

// WorkspaceShell prop
onChangeWorkspaceAvatar={changeWorkspaceAvatar}
```

Do not place file bytes in `ViewHarness` state. Add/adjust a controller test that selects a valid file, verifies the daemon call, and verifies a subsequent `/api/workspace` refresh while existing agent/workspace behavior stays intact.

- [ ] **Step 5: Run shell and controller tests and verify GREEN**

Run:

```powershell
bun x nx run @animaOS-SWARM/web:test --skipNxCache -- src/components/WorkspaceShell.test.tsx src/ViewHarness.test.tsx
```

Expected: PASS.

- [ ] **Step 6: Run web typechecking**

Run:

```powershell
bun x nx run @animaOS-SWARM/web:typecheck --skipNxCache
```

Expected: PASS. Fix avatar-specific errors only; report any unrelated baseline failure separately.

- [ ] **Step 7: Record a clean task checkpoint**

Inspect diffs for the five target files and confirm existing sidebar changes remain. Run `git diff --check`; do not commit unrelated hunks.

### Task 6: Full verification and live UI check

**Files:**
- Verify all modified files; no new production scope.

- [ ] **Step 1: Run full web verification**

Run:

```powershell
bun x nx run @animaOS-SWARM/web:test --skipNxCache
bun x nx run @animaOS-SWARM/web:typecheck --skipNxCache
bun x nx run @animaOS-SWARM/web:build --skipNxCache
```

Expected: all PASS.

- [ ] **Step 2: Run full Rust daemon verification**

Run:

```powershell
bun x nx run rust-daemon:test --skipNxCache
```

If Windows reports that `target/debug/anima-daemon.exe` is locked, rerun exactly:

```powershell
$env:CI='1'
$env:CARGO_TARGET_DIR='target/validation-rust-daemon'
bun x nx run rust-daemon:test --skipNxCache
```

Expected: PASS.

- [ ] **Step 3: Inspect the live desktop interaction**

Reload `http://localhost:4200/`. Confirm the existing sidebar layout is unchanged, the avatar is a focused/hoverable button, selecting a valid image previews it and persists after reload, and a second selection replaces it.

- [ ] **Step 4: Inspect the compact mobile interaction**

Use a 390x844 viewport. Confirm the 32-pixel control works without shifting the existing mobile bar or bottom dock. Restore the normal viewport afterward.

- [ ] **Step 5: Final diff and preservation audit**

Run `git status --short`, `git diff --check`, and focused diffs for all avatar files. Confirm no calendar files, control-plane calendar fields, or unrelated sidebar behavior were removed or overwritten. Report exact commands and any baseline failures; do not claim completion without current passing evidence.
