# OAuth App Configuration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the workspace owner securely configure Google and Microsoft OAuth applications from the Connectors screen without code, environment variables, or daemon restarts.

**Architecture:** Add a daemon-owned OAuth app configuration service backed by Windows Credential Manager with environment fallback. Mail and Calendar resolve configuration through the shared service under provider lifecycle locks. Expose redacted owner-only HTTP endpoints, wrap them in the SDK, and add setup forms to the existing Connectors view.

**Tech Stack:** Rust/Axum/Tokio/keyring/zeroize, TypeScript SDK, React/Vitest, Nx/Bun.

---

### Task 1: Credential configuration service

**Files:**
- Create: `hosts/rust-daemon/src/connectors/oauth_apps.rs`
- Modify: `hosts/rust-daemon/src/connectors/mod.rs`
- Test: `hosts/rust-daemon/src/connectors/oauth_apps.rs`

- [ ] Write failing tests for redacted status, Google/Microsoft validation, `common` tenant default, versioned keyring payloads, vault-over-environment precedence, fail-closed vault errors, replacement/removal, and cancellation-safe owned mutation completion.
- [ ] Run `bun x nx run rust-daemon:test --excludeTaskDependencies --args='connectors::oauth_apps::tests' --skipNxCache` and confirm failures are caused by the missing service.
- [ ] Implement `OAuthAppProvider`, zeroizing config values, status envelopes, environment fallback, in-memory/keyring stores, provider lifecycle locks and revisioned daemon-owned mutations.
- [ ] Rerun the focused tests and confirm they pass.

### Task 2: Live Calendar and mail configuration resolution

**Files:**
- Modify: `hosts/rust-daemon/src/connectors/gcalendar/mod.rs`
- Modify: `hosts/rust-daemon/src/connectors/mail/mod.rs`
- Modify: `hosts/rust-daemon/src/app.rs`
- Modify: `hosts/rust-daemon/src/routes/mod.rs`
- Test: `hosts/rust-daemon/src/connectors/gcalendar/tests.rs`
- Test: `hosts/rust-daemon/src/connectors/mail/tests.rs`

- [ ] Write failing tests proving a saved Google configuration enables Gmail and Calendar without restart, a saved Microsoft configuration enables Outlook, stale OAuth revisions are rejected, and PUT/DELETE conflict with every non-deleted dependent connector or pending flow.
- [ ] Run the focused Calendar and mail tests and verify the expected failures.
- [ ] Inject the shared OAuth app configuration service into both managers; resolve the current config inside the provider lifecycle boundary for begin, callback, refresh and disconnect.
- [ ] Rerun the focused tests and confirm they pass.

### Task 3: Owner-only HTTP API and OpenAPI

**Files:**
- Create: `hosts/rust-daemon/src/routes/oauth_apps.rs`
- Modify: `hosts/rust-daemon/src/routes/mod.rs`
- Modify: `hosts/rust-daemon/src/routes/http.rs`
- Test: `hosts/rust-daemon/src/routes/mod.rs`

- [ ] Write failing route tests for GET/PUT/DELETE envelopes, stable error codes, authorization before body parsing, request-size enforcement, secret redaction, and exact provider paths.
- [ ] Run the focused route tests and confirm expected failures.
- [ ] Add `/api/connectors/oauth-apps/{provider}` routes using the bounded body reader and local-owner authorization, then register schemas and paths in OpenAPI.
- [ ] Rerun focused route tests and confirm they pass.

### Task 4: Typed SDK

**Files:**
- Modify: `packages/sdk/src/connectors.ts`
- Modify: `packages/sdk/src/connectors.spec.ts`
- Modify: `packages/sdk/README.md`

- [ ] Write failing SDK tests for encoded provider paths, PUT body, redacted status response and DELETE behavior.
- [ ] Run `bun x nx run @animaOS-SWARM/sdk:test --skipNxCache` and confirm expected failures.
- [ ] Add `OAuthAppStatus`, `ConfigureOAuthAppInput`, `oauthAppStatus`, `configureOAuthApp` and `removeOAuthApp`.
- [ ] Rerun SDK tests, typecheck and build.

### Task 5: Connectors setup UI

**Files:**
- Modify: `apps/web/src/components/ConnectorsView.tsx`
- Modify: `apps/web/src/components/ConnectorsView.test.tsx`
- Modify: `apps/web/src/studio.css` only if existing utilities cannot express the layout

- [ ] Write failing UI tests for visible redirect URIs, masked secret inputs, Google shared-service copy, Microsoft tenant default, successful save clearing secrets and enabling Connect, independent provider errors, replacement/removal conflicts, and no secret rendering.
- [ ] Run the focused web test and confirm expected failures.
- [ ] Build accessible Google and Microsoft setup cards above the connector grid. Keep provider status isolated, use generic errors, and refresh affected service cards after Save/Remove.
- [ ] Rerun focused and full web tests, typecheck and build.

### Task 6: Documentation, validation and live restart

**Files:**
- Modify: `apps/docs/src/content/docs/sdk/connectors.mdx`
- Modify: `docs/superpowers/plans/2026-09-05-oauth-app-configuration.md`

- [ ] Replace terminal-first setup guidance with the UI flow while retaining environment-variable fallback documentation.
- [ ] Run Rust formatting and the full serial Rust daemon suite via Nx.
- [ ] Run SDK/web tests, typechecks and builds plus the docs build through Nx.
- [ ] Run an isolated daemon smoke test with fake app credentials, confirm only redacted status is returned and Gmail/Calendar readiness changes without restart, then remove the fake credentials without starting provider OAuth.
- [ ] Restart the existing workspace launcher with its environment preserved, reload the saved workspace if the control plane is ephemeral, and verify the Connectors UI/API is live without writing real provider credentials.
- [ ] Mark this plan complete with current verification evidence.
