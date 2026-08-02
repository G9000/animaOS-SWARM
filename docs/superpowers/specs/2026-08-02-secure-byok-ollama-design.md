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

The initial implementation is a local-owner feature. Every provider-administration route that mutates state or causes an outbound request, including credential writes/deletes, connection writes, connection tests, and Ollama model discovery, is allowed only when all of these conditions hold:

- The daemon is directly bound to a loopback interface.
- The socket peer is loopback.
- The request host is loopback.
- No `Forwarded`, `X-Forwarded-For`, or equivalent forwarding metadata is present.
- A browser request has an `Origin` header that exactly matches the daemon's startup allowlist. The allowlist comes from `ANIMA_ALLOWED_UI_ORIGINS`; when unset, it contains only the documented loopback Playground/Web development origins and any same-origin UI actually served by the daemon. Wildcards, suffix matching, `null`, and dynamically trusting the request `Host` are forbidden.
- An originless non-browser request instead supplies `Authorization: Bearer <token>` matching `ANIMA_LOCAL_ADMIN_TOKEN`. If that variable is unset, originless administration is disabled. This token is compared in constant time and is subject to the same redaction and no-log rules as provider credentials.

Failure of any check rejects the request before reading a credential body, opening the vault, mutating state, resolving an outbound destination, or making an outbound request. If the daemon is exposed remotely or placed behind a proxy, all provider administration fails closed; enabling private Ollama destinations does not relax this owner boundary. Read-only provider summaries may remain available because they contain no secrets and cause no outbound traffic.

The OS credential vault protects secrets at rest from casual file access and repository leakage. It does not claim to protect against malware already executing as the same operating-system user or a fully compromised daemon process.

---

## Architecture

### 1. Host-owned credential store

Add a `ProviderCredentialStore` boundary owned by `hosts/rust-daemon`. A versioned provider record has two independently optional fields: `secret` and `base_url`. Its operations are:

- `status(provider_id)`
- `load(provider_id)`
- `put_secret(provider_id, secret)`
- `delete_secret(provider_id)`
- `put_base_url(provider_id, base_url)`
- `delete_base_url(provider_id)`

The production local implementation uses the operating system credential vault:

- Windows: Credential Manager backed by the current user's DPAPI protection.
- macOS: Keychain Services.
- Linux: Secret Service-compatible keyring.

The daemon uses a stable service name and canonical provider identifier, for example `animaos/provider/openai`. Provider aliases resolve to the canonical catalog entry before any lookup or mutation.

Tests use an in-memory fake implementation. If the platform vault is unavailable or locked, the API returns a stable safe error. It must never silently fall back to plaintext storage.

Each operation performs an atomic read-modify-write of only its field. Removing one field preserves the other; the physical vault record is deleted only when both fields are absent. This permits an endpoint-only Ollama record, token replacement or deletion without losing the endpoint, and endpoint replacement without touching the token. The serialized vault payload is never exposed through public APIs.

### 2. Live credential resolver

The current daemon constructs provider credentials from environment variables at startup. Replace that one-time snapshot with a host-owned resolver that creates an immutable effective configuration snapshot at the start of each provider operation.

Secret and base URL are resolved independently rather than selecting one whole record. For each field, resolution order is:

1. That field in the canonical provider record from the OS vault.
2. The existing provider-specific environment variable for that field.
3. For `base_url` only, the catalog default; for `secret`, no value.

This field-level merge means adding an Ollama token cannot suppress an environment base URL, and saving an endpoint cannot suppress an environment token. Deleting a vault-managed field reveals only that field's environment fallback. The read-only provider response reports the effective source of each field without returning secret material.

Dependency direction remains acyclic: `hosts/rust-daemon` owns the vault implementation, environment access, merge policy, and live resolver. It passes a per-operation `ProviderRequestConfig` value containing the resolved secret/base URL into `anima-model-adapters`. The reusable package defines or consumes that request snapshot but never imports the host, opens the vault, or reads host environment variables. All native Ollama, OpenAI-compatible tool, test, and discovery calls use this same snapshot and endpoint policy.

Secrets are represented with a redacting secret type and are exposed as plaintext only at the narrow point where an outbound authorization header or provider request is constructed. Debug implementations, errors, and telemetry must remain redacted.

### 3. HTTP API

Extend the existing provider route family:

#### `GET /api/providers`

Return the existing provider catalog plus:

- `configured: boolean`
- `credentialSource: "vault" | "environment" | "none"`
- `baseUrl`
- `baseUrlSource: "vault" | "environment" | "catalog"`
- `hasVaultCredential: boolean`
- `hasVaultBaseUrl: boolean`
- `supportsModelDiscovery: boolean`

`configured` means the effective configuration is usable for that provider: keyed cloud providers require an effective secret, while key-optional Ollama requires an effective base URL. The response never contains a key, masked key, fingerprint, vault record identifier, or raw environment value.

#### `PUT /api/providers/{providerId}/credential`

Body:

```json
{ "apiKey": "secret value" }
```

The key is required, trimmed only for surrounding whitespace, bounded in size, stored atomically, and never echoed. Replacing a key is explicit. An omitted or empty key is rejected rather than interpreted as deletion.

#### `DELETE /api/providers/{providerId}/credential`

Delete only the vault-managed secret field, preserving any vault-managed base URL. An environment-provided credential cannot be deleted through the UI. The response reports the new effective credential source and the unchanged base-URL source. For Ollama this action removes its optional bearer token without removing its connection endpoint.

#### `PUT /api/providers/ollama/connection`

Body:

```json
{ "baseUrl": "http://127.0.0.1:11434" }
```

The daemon canonicalizes the URL and updates only the record's base-URL field. Loopback is allowed by default. Private-network or remote Ollama endpoints require an explicit host-level opt-in. Userinfo, fragments, non-HTTP schemes, and ambiguous hosts are rejected. An empty value is not deletion; a separate `DELETE /api/providers/ollama/connection` removes only the vault-managed base URL and reveals the environment or catalog fallback.

#### `POST /api/providers/{providerId}/test`

Resolve the effective credential and perform a bounded provider-specific connectivity check. Return only a stable success result or a sanitized error code and message. Upstream bodies, request URLs containing credentials, and secret values are never returned.

#### `GET /api/providers/ollama/models`

After passing the local-owner guard, call Ollama's model-list endpoint with a short timeout and bounded response size. Return normalized model identifiers and optional non-secret metadata. A connection error is safe and actionable but must not include arbitrary upstream response bodies.

All provider-administration responses, including discovery, set `Cache-Control: no-store`. Credential request bodies and the local-admin bearer token must be excluded from request logging.

### 4. Browser UI

Add a Provider Settings view to the Playground, following its existing provider selection patterns.

Each provider row shows:

- Provider label.
- Configuration status.
- Configuration source: UI vault, environment, or not configured.
- Add/replace key action for keyed providers.
- Test connection action.
- Remove UI-managed key action only when `hasVaultCredential` is true.

The API-key input is a password field. Its value remains component-local, is cleared after submission or navigation, and is never placed in localStorage, sessionStorage, URL state, global application state, analytics, or error reporting.

The Ollama section additionally shows:

- Base URL input.
- Base URL source and a remove-override action only when `hasVaultBaseUrl` is true.
- Optional bearer-token add/replace/remove actions independent of the base URL.
- Test connection action.
- Refresh models action.
- Installed-model list.

Agent creation continues to use the provider catalog. When Ollama is selected, its model field becomes a discovered-model selector with a manual fallback for advanced users. Cloud-provider model entry remains unchanged in this release.

### 5. Ollama runtime behavior

Ollama remains key-optional. A secured remote Ollama installation may store an optional bearer token through the ordinary credential endpoint.

The configured base URL feeds both native Ollama generation and the existing OpenAI-compatible tool path. URL normalization must preserve the current behavior where `/v1` is used for OpenAI-compatible requests while native generation targets `/api/chat` and model discovery targets `/api/tags`.

Every Ollama outbound operation uses one shared endpoint builder and enforcing HTTP client policy. At request time it resolves the hostname, normalizes IPv4-mapped IPv6 addresses, and rejects the destination unless every returned address is loopback or belongs to a host-level explicitly allowed CIDR. The client connects to an approved resolved address while preserving the canonical HTTP `Host` value and TLS SNI, so a second DNS lookup cannot change the destination. DNS is re-resolved and revalidated for every new operation and connection reuse may not cross a configuration change. Redirects are disabled; a 3xx response is an error. Mixed allowed/disallowed DNS results are rejected. Userinfo and URL credentials are always rejected. A bearer token may be sent over cleartext HTTP only to a loopback destination; allowed non-loopback destinations require HTTPS.

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
- `provider_owner_auth_required`

Public messages describe the corrective action without including the submitted secret, upstream response body, authorization header, query credential, vault path, or internal platform error.

Credential-store failures do not mutate the live resolver. Connection-setting changes are validated and persisted before becoming active.

---

## Data And Lifecycle Rules

- Vault records are versioned so their payload can evolve without accepting malformed legacy data.
- Credential and base-URL replacement are independently atomic from the daemon's perspective.
- Field deletion preserves the other field, removes the physical vault record only when empty, and invalidates the affected effective configuration.
- No daemon snapshot, agent definition, swarm definition, memory record, step log, or durable execution checkpoint contains a provider secret.
- Provider configuration status may be persisted or logged; secret values may not.
- Credential use may emit a safe audit event containing provider ID, operation, source, success/failure, and timestamp, but never the secret or request payload.
- Environment credentials are read-only through this API.
- Aliases always share the canonical provider's credential; they never create duplicate secret records.

---

## Testing Strategy

### Rust unit tests

- Canonical provider and alias resolution.
- Independent vault-over-environment precedence for secret and base URL, including fallback after each field is deleted.
- Key bounds and empty-key rejection.
- Redacted `Debug`, error, and telemetry output.
- Versioned vault-record validation.
- Atomic field replacement, endpoint-only records, independent token deletion, and whole-record cleanup when empty.
- Ollama URL normalization plus rejection of mixed DNS results, IPv4-mapped IPv6 bypasses, redirects, DNS rebinding attempts, and cleartext tokens to non-loopback destinations.
- Native, OpenAI-compatible, and discovery endpoint construction from one Ollama base URL.

### Host integration tests

- Provider list exposes status and source without secret material.
- Put, replace, test, and delete against an in-memory credential store.
- Restart simulation reloads a vault-managed credential.
- Provider-administration routes reject remote peers, forwarded metadata, unapproved/browserless origins, missing or invalid originless-client tokens, and disabled remote administration before side effects.
- Request and response bodies remain bounded.
- Sanitized errors do not contain a submitted sentinel secret or upstream body.
- Environment credentials remain usable and cannot be deleted through the API.
- Ollama generation, testing, and discovery share the enforcing endpoint client; discovery uses a mock `/api/tags` server and handles timeout, redirects, malformed data, and oversized data.

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
