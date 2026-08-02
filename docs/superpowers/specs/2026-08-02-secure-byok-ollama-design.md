# Secure BYOK And Ollama Design

**Date:** 2026-08-02
**Status:** Approved for specification review

---

## Decision Summary

Add provider credential management to the browser UI without making the browser a secret store. The Rust daemon owns credential ingestion, secure persistence, resolution, redaction, and deletion. Local installations persist secrets in the operating system credential vault. Server deployments can later supply another implementation of the same host-owned credential-store boundary.

Ollama remains a first-class provider. The UI adds connection testing and installed-model discovery against a configurable local Ollama endpoint. The existing environment-variable configuration remains supported as a fallback and for headless deployments.

The core security rule is:

> The browser may submit a secret once, but it can never read that secret back.

---

## Context

The current workspace already has:

- A provider catalog in `packages/core-rust/crates/anima-model-adapters`.
- OpenAI, Anthropic, Google, Ollama, and OpenAI-compatible provider transports.
- Host-owned provider configuration assembled from environment variables in `hosts/rust-daemon`.
- A read-only `GET /api/providers` route consumed by the Playground provider selector.
- Native Ollama requests for ordinary generation and an OpenAI-compatible Ollama path for tool-enabled requests.

What is missing is a secure runtime credential-management boundary. Today users must restart the daemon with API keys in environment variables. Ollama exists in the provider catalog but has no UI for endpoint management, connection testing, or model discovery.

---

## Goals

1. Let a local owner add, replace, test, and remove provider API keys from the UI.
2. Persist keys securely across daemon restarts using the operating system credential vault.
3. Never return, log, serialize, snapshot, or expose stored secret values.
4. Preserve environment-variable configuration for CLI, CI, containers, and headless deployments.
5. Let users configure a local Ollama endpoint, test it, discover installed models, and select one when creating an agent.
6. Make credential changes effective without restarting the daemon.
7. Keep reusable provider behavior in `packages/*` and host-specific secret storage and HTTP policy in `hosts/rust-daemon`.

## Non-Goals

- Synchronizing API keys between machines.
- Sharing credentials between multiple operating-system users.
- Building a general-purpose password manager.
- Returning key suffixes, fingerprints, or masked secret fragments to the UI.
- Allowing arbitrary custom cloud-provider base URLs from the UI in the first release.
- Enabling credential administration over an unauthenticated remote daemon.
- Adding AWS Secrets Manager, Azure Key Vault, Google Secret Manager, or HashiCorp Vault in this release.
- Changing the model-provider wire protocols already implemented by `anima-model-adapters`.

---

## Threat Model And Security Boundary

### Protected assets

- Provider API keys and optional Ollama bearer tokens.
- Provider connection settings that determine where a credential is sent.
- The fact that a provider is configured, which is non-secret operational metadata.

### In-scope threats

- Secrets leaking through browser storage, HTTP responses, debug output, traces, errors, snapshots, or logs.
- A malicious website attempting to call a loopback credential endpoint from the user's browser.
- A forged forwarded address making a remote request appear local.
- A configured Ollama URL being used for server-side request forgery.
- Accidental key deletion or replacement caused by ambiguous update semantics.
- Credentials being sent to a provider alias or endpoint other than the one the user configured.

### Boundary assumptions

The initial implementation is a local-owner feature. Credential mutation is allowed only when all of these conditions hold:

- The daemon is directly bound to a loopback interface.
- The socket peer is loopback.
- The request host is loopback.
- No `Forwarded`, `X-Forwarded-For`, or equivalent forwarding metadata is present.
- The request origin matches an explicitly allowed local animaOS UI origin, or the request is a non-browser local client using the host's explicit local-owner mechanism.

If the daemon is exposed remotely or placed behind a proxy, credential mutation and credential testing fail closed until an authenticated owner boundary is configured. Read-only provider summaries may remain available because they contain no secrets.

The OS credential vault protects secrets at rest from casual file access and repository leakage. It does not claim to protect against malware already executing as the same operating-system user or a fully compromised daemon process.

---

## Architecture

### 1. Host-owned credential store

Add a `ProviderCredentialStore` boundary owned by `hosts/rust-daemon`. Its operations are:

- `status(provider_id)`
- `load(provider_id)`
- `put(provider_id, secret, connection_metadata)`
- `delete(provider_id)`

The production local implementation uses the operating system credential vault:

- Windows: Credential Manager backed by the current user's DPAPI protection.
- macOS: Keychain Services.
- Linux: Secret Service-compatible keyring.

The daemon uses a stable service name and canonical provider identifier, for example `animaos/provider/openai`. Provider aliases resolve to the canonical catalog entry before any lookup or mutation.

Tests use an in-memory fake implementation. If the platform vault is unavailable or locked, the API returns a stable safe error. It must never silently fall back to plaintext storage.

For this release, the vault record may contain a small versioned payload with the secret and provider-specific connection metadata. This keeps Ollama's endpoint persistent without introducing a second local settings database. The serialized vault payload is never exposed through public APIs.

### 2. Live credential resolver

The current daemon constructs provider credentials from environment variables at startup. Replace that one-time snapshot with a shared resolver used by the provider adapter at request time.

Resolution order is:

1. Canonical provider record from the OS vault.
2. Existing provider-specific environment variables.
3. Catalog default base URL with no key.

This order lets an explicit UI-managed credential replace an environment value. Deleting the vault record reveals the environment fallback again. The read-only provider response reports the effective source as `vault`, `environment`, or `none` without returning secret material.

Secrets are represented with a redacting secret type and are exposed as plaintext only at the narrow point where an outbound authorization header or provider request is constructed. Debug implementations, errors, and telemetry must remain redacted.

### 3. HTTP API

Extend the existing provider route family:

#### `GET /api/providers`

Return the existing provider catalog plus:

- `configured: boolean`
- `credentialSource: "vault" | "environment" | "none"`
- `baseUrl`
- `supportsModelDiscovery: boolean`

The response never contains a key, masked key, fingerprint, vault record identifier, or raw environment value.

#### `PUT /api/providers/{providerId}/credential`

Body:

```json
{ "apiKey": "secret value" }
```

The key is required, trimmed only for surrounding whitespace, bounded in size, stored atomically, and never echoed. Replacing a key is explicit. An omitted or empty key is rejected rather than interpreted as deletion.

#### `DELETE /api/providers/{providerId}/credential`

Delete only the vault-managed record. An environment-provided credential cannot be deleted through the UI. The response reports the new effective configuration source.

#### `PUT /api/providers/ollama/connection`

Body:

```json
{ "baseUrl": "http://127.0.0.1:11434" }
```

The daemon canonicalizes the URL. Loopback is allowed by default. Private-network or remote Ollama endpoints require an explicit host-level opt-in. Userinfo, fragments, non-HTTP schemes, and ambiguous hosts are rejected.

#### `POST /api/providers/{providerId}/test`

Resolve the effective credential and perform a bounded provider-specific connectivity check. Return only a stable success result or a sanitized error code and message. Upstream bodies, request URLs containing credentials, and secret values are never returned.

#### `GET /api/providers/ollama/models`

Call Ollama's model-list endpoint with a short timeout and bounded response size. Return normalized model identifiers and optional non-secret metadata. A connection error is safe and actionable but must not include arbitrary upstream response bodies.

All mutation and test responses set `Cache-Control: no-store`. Credential request bodies must be excluded from request logging.

### 4. Browser UI

Add a Provider Settings view to the Playground, following its existing provider selection patterns.

Each provider row shows:

- Provider label.
- Configuration status.
- Configuration source: UI vault, environment, or not configured.
- Add/replace key action for keyed providers.
- Test connection action.
- Remove UI-managed key action when a vault record exists.

The API-key input is a password field. Its value remains component-local, is cleared after submission or navigation, and is never placed in localStorage, sessionStorage, URL state, global application state, analytics, or error reporting.

The Ollama section additionally shows:

- Base URL input.
- Test connection action.
- Refresh models action.
- Installed-model list.

Agent creation continues to use the provider catalog. When Ollama is selected, its model field becomes a discovered-model selector with a manual fallback for advanced users. Cloud-provider model entry remains unchanged in this release.

### 5. Ollama runtime behavior

Ollama remains key-optional. A secured remote Ollama installation may store an optional bearer token through the ordinary credential endpoint.

The configured base URL feeds both native Ollama generation and the existing OpenAI-compatible tool path. URL normalization must preserve the current behavior where `/v1` is used for OpenAI-compatible requests while native generation targets `/api/chat` and model discovery targets `/api/tags`.

Changing the Ollama URL or token takes effect for subsequent requests without restarting the daemon. In-flight requests continue with the credential snapshot they started with.

---

## Error Model

Provider administration uses stable host error codes:

- `provider_not_found`
- `credential_required`
- `credential_store_unavailable`
- `credential_write_failed`
- `credential_delete_failed`
- `provider_not_configured`
- `provider_connection_failed`
- `provider_response_invalid`
- `provider_url_invalid`
- `provider_remote_admin_disabled`
- `provider_origin_rejected`

Public messages describe the corrective action without including the submitted secret, upstream response body, authorization header, query credential, vault path, or internal platform error.

Credential-store failures do not mutate the live resolver. Connection-setting changes are validated and persisted before becoming active.

---

## Data And Lifecycle Rules

- Vault records are versioned so their payload can evolve without accepting malformed legacy data.
- Credential replacement is atomic from the daemon's perspective.
- Deletion removes the vault value and evicts any live cached value.
- No daemon snapshot, agent definition, swarm definition, memory record, step log, or durable execution checkpoint contains a provider secret.
- Provider configuration status may be persisted or logged; secret values may not.
- Credential use may emit a safe audit event containing provider ID, operation, source, success/failure, and timestamp, but never the secret or request payload.
- Environment credentials are read-only through this API.
- Aliases always share the canonical provider's credential; they never create duplicate secret records.

---

## Testing Strategy

### Rust unit tests

- Canonical provider and alias resolution.
- Vault-over-environment precedence and fallback after deletion.
- Key bounds and empty-key rejection.
- Redacted `Debug`, error, and telemetry output.
- Versioned vault-record validation.
- Atomic replacement and delete behavior.
- Ollama URL normalization and SSRF-oriented rejection cases.
- Native, OpenAI-compatible, and discovery endpoint construction from one Ollama base URL.

### Host integration tests

- Provider list exposes status and source without secret material.
- Put, replace, test, and delete against an in-memory credential store.
- Restart simulation reloads a vault-managed credential.
- Credential routes reject remote peers, forwarded metadata, and unapproved origins.
- Request and response bodies remain bounded.
- Sanitized errors do not contain a submitted sentinel secret or upstream body.
- Environment credentials remain usable and cannot be deleted through the API.
- Ollama discovery uses a mock `/api/tags` server and handles timeout, malformed data, and oversized data.

### UI tests

- Key value never enters browser storage or URL state.
- Successful submission clears the input.
- Environment-managed and vault-managed states expose the correct actions.
- Removing a vault credential reveals an environment fallback when present.
- Ollama connection, model refresh, discovered selection, and manual fallback states.
- Failed operations keep actionable non-secret feedback visible.

### Required verification

- Relevant Playground tests and typecheck/build targets.
- `bun x nx run core-rust:test --skipNxCache`.
- `bun x nx run rust-daemon:test --skipNxCache`.
- `bun x nx test workspace-dev --runInBand --skipNxCache` only if launcher environment forwarding changes.

---

## Rollout

1. Add the credential-store abstraction, in-memory test store, and OS-vault implementation.
2. Add the live resolver and preserve existing environment-based behavior.
3. Extend provider APIs with local-owner guards and safe errors.
4. Add Ollama connection testing and model discovery.
5. Add Provider Settings UI and integrate discovered Ollama models into agent creation.
6. Document BYOK storage, threat boundary, deletion, environment fallback, and Ollama setup.

The feature ships disabled for remote credential administration. A future authenticated owner boundary may enable the same APIs for remote deployments without changing the credential-store contract.

---

## Acceptance Criteria

- A user can add an OpenAI or Anthropic key in the Playground, restart the daemon, and use the provider without setting an environment variable.
- The stored key is in the current user's OS credential vault and is absent from repository files, browser storage, daemon snapshots, logs, errors, and API responses.
- Replacing or deleting a UI-managed key takes effect for the next model request without daemon restart.
- Existing environment-variable configuration continues to work unchanged.
- A user can connect to local Ollama, list installed models, choose one, create an agent, and run it.
- Credential mutation fails closed outside the direct local-owner boundary.
- Tests prove that sentinel secrets do not appear in public output or durable state.

