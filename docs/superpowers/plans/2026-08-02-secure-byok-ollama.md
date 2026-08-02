# Secure BYOK And Ollama Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add production-grade, persistent local BYOK management and Ollama connection/model discovery to the Rust daemon and Playground without ever making secrets readable by the browser.

**Architecture:** `hosts/rust-daemon` owns a field-addressable OS-vault store, environment fallback, owner authorization, and live provider resolver. Each model operation resolves an immutable `ProviderRequestConfig` and passes it into the reusable `anima-model-adapters` package; Ollama generation, testing, and discovery share one redirect-free, DNS-pinned endpoint policy. The Playground receives only non-secret status and submits secrets through write-only administration endpoints.

**Tech Stack:** Rust 1.93, Axum 0.8, Reqwest 0.12/rustls, keyring 4.1.5, Tokio, Serde, Utoipa, React 19, TypeScript, Vite/Vitest, Nx, Bun.

**Design reference:** `docs/superpowers/specs/2026-08-02-secure-byok-ollama-design.md`

---

## File Map

### Reusable provider package

- Create `packages/core-rust/crates/anima-model-adapters/src/endpoint_policy.rs`: validated Ollama base URL, DNS classification, pinned Reqwest client construction, redirect rejection, and endpoint joining.
- Modify `packages/core-rust/crates/anima-model-adapters/src/lib.rs`: export redacting `ProviderRequestConfig`, `OllamaEndpointPolicy`, provider test, and model-discovery result types.
- Modify `packages/core-rust/crates/anima-model-adapters/src/adapter.rs`: accept operation-scoped provider snapshots and use the shared endpoint policy for both native and OpenAI-compatible Ollama calls.
- Modify `packages/core-rust/crates/anima-model-adapters/src/tests.rs`: request-snapshot, redaction, endpoint-construction, redirect, DNS classification, and Ollama test/discovery coverage.
- Modify `packages/core-rust/crates/anima-model-adapters/Cargo.toml`: add URL/CIDR support only if not already available transitively; do not add any host or vault dependency.

### Rust host

- Create `hosts/rust-daemon/src/provider_credentials.rs`: versioned vault record, `ProviderCredentialStore` trait, in-memory fake, keyring implementation, and safe store errors.
- Create `hosts/rust-daemon/src/provider_runtime.rs`: environment source, independent field resolution, provider summaries, immutable request snapshots, mutation methods, and outbound provider operations.
- Create `hosts/rust-daemon/src/routes/providers.rs`: provider administration handlers and owner-guarded request orchestration.
- Create `hosts/rust-daemon/src/routes/provider_owner.rs`: direct-loopback, Host, forwarding-header, exact-Origin, and originless-token policy.
- Modify `hosts/rust-daemon/src/runtime_model.rs`: resolve configuration at the start of every generation instead of retaining the startup environment snapshot.
- Modify `hosts/rust-daemon/src/app.rs`: construct production/in-memory provider services, pass listener binding facts, and use `ConnectInfo<SocketAddr>` for direct-peer checks.
- Modify `hosts/rust-daemon/src/routes/mod.rs`: add provider services to `AppState`, mount routes, preserve global API-key middleware, and register OpenAPI paths.
- Modify `hosts/rust-daemon/src/routes/http.rs`: expose constant-time comparison and add `no-store` JSON response support without logging secrets.
- Modify `hosts/rust-daemon/src/routes/contracts/providers.rs`: request/response DTOs and stable safe error codes.
- Modify `hosts/rust-daemon/src/routes/contracts/mod.rs`: re-export provider DTOs.
- Modify `hosts/rust-daemon/src/lib.rs`: register host-only provider modules.
- Modify `hosts/rust-daemon/Cargo.toml`: add `keyring = "4.1.5"`, `url`, and any narrowly required IP/CIDR dependency.
- Modify `Cargo.lock`: lock the new host dependencies.

### Playground

- Modify `apps/playground/src/lib/api.ts`: non-secret provider status, credential/connection mutation, test, and discovery client methods.
- Create `apps/playground/src/components/ProviderSettings.tsx`: provider cards, component-local password inputs, independent Ollama URL/token controls, status/error feedback, and model refresh.
- Create `apps/playground/src/components/ProviderSettings.spec.tsx`: write-only secret and independent field-action UI tests.
- Modify `apps/playground/src/components/CreateAgent.tsx`: discovered Ollama model selector with manual fallback.
- Modify `apps/playground/src/components/Sidebar.tsx`: dedicated Providers settings entry without pretending providers are deletable runtime entities.
- Modify `apps/playground/src/playground.tsx`: provider-settings view and a single refresh callback shared by polling and successful mutations.
- Modify `apps/playground/vite.config.mts`: add the Playground Vitest target configuration if the inferred target does not discover component tests.
- Modify `apps/playground/package.json`: add no dependency unless the existing workspace Vitest/jsdom setup proves insufficient.

### Documentation

- Modify `apps/docs/src/content/docs/cli/providers.mdx`: vault behavior, exact local origins, environment fallbacks, admin-token/API-key composition, Ollama security policy, and run/test instructions alongside the existing provider guide.
- Modify `docs/rust-daemon-architecture.md`: record the host-owned store/resolver and operation-scoped adapter boundary.

---

### Task 1: Establish The Operation-Scoped Provider Contract

**Files:**
- Modify: `packages/core-rust/crates/anima-model-adapters/Cargo.toml`
- Modify: `Cargo.lock`
- Modify: `packages/core-rust/crates/anima-model-adapters/src/lib.rs`
- Modify: `packages/core-rust/crates/anima-model-adapters/src/adapter.rs`
- Test: `packages/core-rust/crates/anima-model-adapters/src/tests.rs`

- [ ] **Step 1: Write failing tests for an operation-scoped configuration**

Add tests proving two sequential calls through the same adapter can use different keys/base URLs and that `Debug` never exposes the key:

```rust
#[tokio::test]
async fn each_operation_uses_its_supplied_provider_snapshot() {
    let first = ProviderRequestConfig::new(Some(ProviderSecret::new("secret-one")), first_url);
    let second = ProviderRequestConfig::new(Some(ProviderSecret::new("secret-two")), second_url);
    adapter.generate_with_config(&agent, &request(), first).await.unwrap();
    adapter.generate_with_config(&agent, &request(), second).await.unwrap();
    assert_eq!(first_server_hits.load(Ordering::SeqCst), 1);
    assert_eq!(second_server_hits.load(Ordering::SeqCst), 1);
}

#[test]
fn request_config_debug_redacts_secret() {
    let config = ProviderRequestConfig::new(
        Some(ProviderSecret::new("sentinel-secret")),
        "https://example.test",
    );
    assert!(!format!("{config:?}").contains("sentinel-secret"));
}
```

- [ ] **Step 2: Run the focused tests and verify failure**

Run: `cargo test -p anima-model-adapters`

Expected: FAIL because `ProviderRequestConfig` and `generate_with_config` do not exist.

- [ ] **Step 3: Add the minimal public snapshot type and adapter entry points**

Use an owned, cloneable, redacting value. `ProviderSecret` has a private inner string, public construction, Serde support for the host's versioned vault payload, redacted `Debug`, and an adapter-crate-private plaintext accessor. `ProviderRequestConfig` also keeps its fields private and exposes constructors rather than raw secret access:

First add the direct derive dependency required by this reusable type:

Run: `cargo add serde --features derive --package anima-model-adapters`

```rust
#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderSecret(String);

impl ProviderSecret {
    pub fn new(value: impl Into<String>) -> Self { Self(value.into()) }

    pub(crate) fn expose_for_request(&self) -> &str { &self.0 }
}

impl Debug for ProviderSecret {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result { f.write_str("[redacted]") }
}

#[derive(Clone)]
pub struct ProviderRequestConfig {
    api_key: Option<ProviderSecret>,
    base_url: String,
}

impl Debug for ProviderRequestConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderRequestConfig")
            .field("api_key", &self.api_key)
            .field("base_url", &self.base_url)
            .finish()
    }
}
```

Keep `ProviderAdapterConfig`/`ProviderModelAdapter::new` temporarily for compatibility, converting legacy strings immediately into `ProviderSecret`. Add `generate_with_config` and `stream_with_config`; the existing trait methods delegate through the legacy map until Task 4 migrates the host. Adapter transports may call `expose_for_request` only while constructing the outbound authorization header or provider-auth field; no host module or public caller can retrieve plaintext from the snapshot.

- [ ] **Step 4: Run package tests**

Run: `cargo test -p anima-model-adapters`

Expected: PASS with existing provider routing unchanged.

- [ ] **Step 5: Commit**

```powershell
git add packages/core-rust/crates/anima-model-adapters/Cargo.toml packages/core-rust/crates/anima-model-adapters/src/lib.rs packages/core-rust/crates/anima-model-adapters/src/adapter.rs packages/core-rust/crates/anima-model-adapters/src/tests.rs Cargo.lock
git commit --no-gpg-sign -m "refactor: accept operation-scoped provider config"
```

### Task 2: Implement Field-Addressable Credential Storage

**Files:**
- Create: `hosts/rust-daemon/src/provider_credentials.rs`
- Modify: `hosts/rust-daemon/src/lib.rs`
- Test: `hosts/rust-daemon/src/provider_credentials.rs`

- [ ] **Step 1: Write failing store-contract tests**

Cover endpoint-only records, independent secret removal, record cleanup, malformed versions, canonical IDs, and sentinel-secret redaction:

```rust
#[tokio::test]
async fn deleting_secret_preserves_ollama_base_url() {
    let store = InMemoryProviderCredentialStore::default();
    store.put_base_url("ollama", "http://127.0.0.1:11434").await.unwrap();
    store.put_secret("ollama", ProviderSecret::new("token")).await.unwrap();
    store.delete_secret("ollama").await.unwrap();
    let record = store.load("ollama").await.unwrap().unwrap();
    assert!(record.secret.is_none());
    assert_eq!(record.base_url.as_deref(), Some("http://127.0.0.1:11434"));
}
```

- [ ] **Step 2: Run and verify failure**

Run: `cargo test -p anima-daemon provider_credentials::tests`

Expected: FAIL because the module and store do not exist.

- [ ] **Step 3: Implement the trait, record, fake, and safe errors**

Implement:

```rust
#[async_trait]
pub(crate) trait ProviderCredentialStore: Send + Sync {
    async fn status(&self, provider_id: &str) -> Result<ProviderRecordStatus, StoreError>;
    async fn load(&self, provider_id: &str) -> Result<Option<ProviderVaultRecord>, StoreError>;
    async fn put_secret(&self, provider_id: &str, secret: ProviderSecret) -> Result<(), StoreError>;
    async fn delete_secret(&self, provider_id: &str) -> Result<(), StoreError>;
    async fn put_base_url(&self, provider_id: &str, base_url: String) -> Result<(), StoreError>;
    async fn delete_base_url(&self, provider_id: &str) -> Result<(), StoreError>;
}

#[derive(Serialize, Deserialize)]
struct ProviderVaultRecord {
    version: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    secret: Option<ProviderSecret>,
    #[serde(skip_serializing_if = "Option::is_none")]
    base_url: Option<String>,
}
```

Serialize only inside the store boundary through `ProviderSecret`'s Serde implementation; host code must never unwrap it to a `String`. Never derive `Debug` for the serialized record. Protect read-modify-write with a per-store async mutex so concurrent field writes do not overwrite one another.

- [ ] **Step 4: Run tests**

Run: `cargo test -p anima-daemon provider_credentials::tests`

Expected: PASS.

- [ ] **Step 5: Commit**

```powershell
git add hosts/rust-daemon/src/provider_credentials.rs hosts/rust-daemon/src/lib.rs
git commit --no-gpg-sign -m "feat: add provider credential store contract"
```

### Task 3: Back The Store With The Operating-System Vault

**Files:**
- Modify: `hosts/rust-daemon/Cargo.toml`
- Modify: `Cargo.lock`
- Modify: `hosts/rust-daemon/src/provider_credentials.rs`
- Test: `hosts/rust-daemon/src/provider_credentials.rs`

- [ ] **Step 1: Add backend-adapter tests without touching the real vault**

Extract a narrow `VaultEntry` seam around get/set/delete. Test JSON round-trips, `NoEntry` mapping, atomic read-modify-write behavior, and that backend errors become `credential_store_unavailable`, `credential_write_failed`, or `credential_delete_failed` without backend text.

- [ ] **Step 2: Run and verify failure**

Run: `cargo test -p anima-daemon provider_credentials::tests::keyring`

Expected: FAIL because `KeyringProviderCredentialStore` is absent.

- [ ] **Step 3: Add the dependency and production implementation**

Run: `cargo add keyring@4.1.5 --package anima-daemon`

Use service `animaos.providers.v1` and canonical provider ID as the account. Wrap synchronous keyring calls in `tokio::task::spawn_blocking`. Use keyring's default `v1` feature, which selects Windows native Credential Manager, macOS Keychain, and a Secret Service store on Unix. Treat a locked/unavailable vault as an error and never write a plaintext fallback.

- [ ] **Step 4: Run focused and host tests**

Run: `cargo test -p anima-daemon provider_credentials::tests`

Expected: PASS without touching the developer's real credential vault.

- [ ] **Step 5: Commit**

```powershell
git add hosts/rust-daemon/Cargo.toml Cargo.lock hosts/rust-daemon/src/provider_credentials.rs
git commit --no-gpg-sign -m "feat: persist provider settings in OS vault"
```

### Task 4: Resolve Vault And Environment Fields Per Operation

**Files:**
- Create: `hosts/rust-daemon/src/provider_runtime.rs`
- Modify: `hosts/rust-daemon/src/runtime_model.rs`
- Modify: `hosts/rust-daemon/src/app.rs`
- Modify: `hosts/rust-daemon/src/routes/mod.rs`
- Modify: `hosts/rust-daemon/src/lib.rs`
- Test: `hosts/rust-daemon/src/provider_runtime.rs`
- Test: `hosts/rust-daemon/src/runtime_model/tests.rs`

- [ ] **Step 1: Write failing resolver tests**

Test field-level precedence explicitly:

```rust
#[tokio::test]
async fn vault_token_does_not_hide_environment_base_url() { /* assert both sources */ }

#[tokio::test]
async fn endpoint_only_record_does_not_hide_environment_token() { /* assert both sources */ }

#[tokio::test]
async fn deleting_one_override_reveals_only_its_fallback() { /* assert source transitions */ }
```

Also update the runtime-adapter test to mutate the fake store between two generations and assert the second request uses the new snapshot without restart.

- [ ] **Step 2: Run and verify failure**

Run: `cargo test -p anima-daemon`

Expected: FAIL because the resolver is not wired.

- [ ] **Step 3: Implement `ProviderRuntime` and independent sources**

The effective result must carry separate sources:

```rust
pub(crate) struct EffectiveProviderConfig {
    pub request: ProviderRequestConfig,
    pub credential_source: CredentialSource,
    pub base_url_source: BaseUrlSource,
    pub has_vault_credential: bool,
    pub has_vault_base_url: bool,
}
```

Canonicalize aliases before store access. Read environment fields independently on each resolution. Return catalog defaults only for base URLs. Build one immutable request snapshot before awaiting provider I/O.

- [ ] **Step 4: Wire generation through the resolver**

`RuntimeModelAdapter` should hold `Arc<ProviderRuntime>`, resolve after selecting the canonical provider, then call `ProviderModelAdapter::generate_with_config`. Keep deterministic/test behavior unchanged. Construct exactly one production `Arc<ProviderRuntime>` in `serve`; pass clones of that same Arc into both `RuntimeModelAdapter` and `routes::AppState`. App/test helpers construct exactly one in-memory runtime and share it the same way. Add a regression test that mutates through the `AppState` runtime and then generates through the model adapter, proving the new value is observed.

- [ ] **Step 5: Run focused and regression tests**

Run: `cargo test -p anima-daemon`

Expected: PASS.

Run: `cargo test -p anima-model-adapters`

Expected: PASS.

- [ ] **Step 6: Commit**

```powershell
git add hosts/rust-daemon/src/provider_runtime.rs hosts/rust-daemon/src/runtime_model.rs hosts/rust-daemon/src/runtime_model/tests.rs hosts/rust-daemon/src/app.rs hosts/rust-daemon/src/routes/mod.rs hosts/rust-daemon/src/lib.rs
git commit --no-gpg-sign -m "feat: resolve provider settings per request"
```

### Task 5: Enforce One Ollama Destination Policy Everywhere

**Files:**
- Create: `packages/core-rust/crates/anima-model-adapters/src/endpoint_policy.rs`
- Modify: `packages/core-rust/crates/anima-model-adapters/src/lib.rs`
- Modify: `packages/core-rust/crates/anima-model-adapters/src/adapter.rs`
- Modify: `packages/core-rust/crates/anima-model-adapters/Cargo.toml`
- Modify: `hosts/rust-daemon/src/provider_runtime.rs`
- Modify: `hosts/rust-daemon/src/app.rs`
- Modify: `hosts/rust-daemon/Cargo.toml`
- Test: `packages/core-rust/crates/anima-model-adapters/src/tests.rs`
- Test: `hosts/rust-daemon/src/provider_runtime.rs`

- [ ] **Step 1: Write failing URL-policy tests**

Cover loopback literals, `localhost`, query/userinfo/fragment rejection, non-HTTP schemes, IPv4-mapped IPv6 normalization, mixed DNS results, CIDR opt-in, redirect rejection, DNS rebinding prevention, cleartext bearer-token rejection off loopback, and exact `/api/chat`, `/api/tags`, `/v1/chat/completions` construction. Add host tests for absent, valid, malformed, and mixed `ANIMA_OLLAMA_ALLOWED_CIDRS` values.

- [ ] **Step 2: Run and verify failure**

Run: `cargo test -p anima-model-adapters`

Expected: FAIL because the endpoint policy and outbound helpers are absent.

- [ ] **Step 3: Implement URL validation and request-time DNS pinning**

Reject base URLs with query strings in addition to the approved spec's userinfo/fragment restrictions. Resolve all A/AAAA results for every new operation, normalize mapped addresses, require every result to satisfy loopback or configured CIDRs, and pin approved `SocketAddr` values through Reqwest's resolver override while preserving URL host/SNI. Build a per-operation client with `Policy::none()` redirects and no connection reuse across configuration changes.

The host parses `ANIMA_OLLAMA_ALLOWED_CIDRS` once at startup as a comma-separated list of explicit IPv4/IPv6 CIDRs. Unset/empty means loopback only; any malformed member fails daemon startup instead of weakening the policy. Store the parsed list on the shared `ProviderRuntime` and include it in every Ollama request snapshot. The reusable package receives only parsed policy data and never reads host environment variables. Task 5 extends `ProviderRequestConfig` with the now-defined `OllamaEndpointPolicy`; Task 1 deliberately compiles without that field.

- [ ] **Step 4: Add shared provider test and discovery helpers**

Export bounded operations:

```rust
pub async fn test_provider_connection(
    provider_id: &str,
    config: ProviderRequestConfig,
) -> Result<(), ProviderOperationError>;

pub async fn discover_ollama_models(
    config: ProviderRequestConfig,
) -> Result<Vec<OllamaModel>, ProviderOperationError>;
```

Use a short timeout, bounded response reads, normalized unique model names, and sanitized errors. Native generation, tool-enabled generation, testing, and discovery must all call the same endpoint-policy builder. Connectivity probes are deterministic and read-only: OpenAI and OpenAI-compatible providers request `GET /models` with bearer auth, Anthropic requests `GET /v1/models` with `x-api-key` and the existing Anthropic version header, Google requests `GET /v1beta/models` using the adapter's existing key transport, and Ollama requests `GET /api/tags` with its optional bearer token. Treat any 2xx response with a bounded parseable provider shape as connected; sanitize all other responses without returning bodies or credential-bearing URLs.

- [ ] **Step 5: Run package tests**

Run: `cargo test -p anima-model-adapters`

Expected: PASS, including mock-server redirect and destination tests.

- [ ] **Step 6: Commit**

```powershell
git add packages/core-rust/crates/anima-model-adapters/Cargo.toml packages/core-rust/crates/anima-model-adapters/src/endpoint_policy.rs packages/core-rust/crates/anima-model-adapters/src/lib.rs packages/core-rust/crates/anima-model-adapters/src/adapter.rs packages/core-rust/crates/anima-model-adapters/src/tests.rs hosts/rust-daemon/Cargo.toml hosts/rust-daemon/src/provider_runtime.rs hosts/rust-daemon/src/app.rs Cargo.lock
git commit --no-gpg-sign -m "feat: enforce safe Ollama destinations"
```

### Task 6: Implement The Fail-Closed Local-Owner Guard

**Files:**
- Create: `hosts/rust-daemon/src/routes/provider_owner.rs`
- Modify: `hosts/rust-daemon/src/routes/http.rs`
- Modify: `hosts/rust-daemon/src/app.rs`
- Modify: `hosts/rust-daemon/src/routes/mod.rs`
- Test: `hosts/rust-daemon/src/routes/provider_owner.rs`

- [ ] **Step 1: Write a table-driven owner-policy test matrix**

Test exact default serialized origins (`http://localhost:4200`, `http://127.0.0.1:4200`, `http://localhost:4201`, `http://127.0.0.1:4201`), configured origins, suffix attacks, `Origin: null`, missing origin, remote peer, non-loopback bind, non-loopback Host, every standard forwarding header, missing/wrong/correct originless bearer token, and rejection before a counting store/outbound fake records a side effect.

- [ ] **Step 2: Run and verify failure**

Run: `cargo test -p anima-daemon provider_owner::tests`

Expected: FAIL because the policy does not exist.

- [ ] **Step 3: Implement the policy as pure validation plus Axum extraction**

Parse startup configuration once. Browser requests require exact serialized-Origin membership. Originless requests require `Authorization: Bearer <ANIMA_LOCAL_ADMIN_TOKEN>` using the existing constant-time helper. Reject `Forwarded`, all `X-Forwarded-*`, and `Via`. Require listener bind, socket peer, and Host to be loopback.

Document and test global-auth composition: when `ANIMAOS_RS_API_KEY` differs, an originless client sends the global key in `X-Api-Key` and the local-owner token in `Authorization`; when equal, one bearer value satisfies both. The Playground path uses exact Origin and does not receive the local-owner token.

- [ ] **Step 4: Preserve peer information in production**

Change serving to `app.into_make_service_with_connect_info::<SocketAddr>()`. Tests inject `ConnectInfo<SocketAddr>` explicitly; missing peer metadata fails closed for provider administration.

- [ ] **Step 5: Run owner-policy and route regression tests**

Run: `cargo test -p anima-daemon`

Expected: PASS.

- [ ] **Step 6: Commit**

```powershell
git add hosts/rust-daemon/src/routes/provider_owner.rs hosts/rust-daemon/src/routes/http.rs hosts/rust-daemon/src/app.rs hosts/rust-daemon/src/routes/mod.rs
git commit --no-gpg-sign -m "feat: guard local provider administration"
```

### Task 7: Add Provider Administration HTTP APIs

**Files:**
- Create: `hosts/rust-daemon/src/routes/providers.rs`
- Modify: `hosts/rust-daemon/src/routes/contracts/providers.rs`
- Modify: `hosts/rust-daemon/src/routes/contracts/mod.rs`
- Modify: `hosts/rust-daemon/src/routes/mod.rs`
- Modify: `hosts/rust-daemon/src/provider_runtime.rs`
- Test: `hosts/rust-daemon/src/routes/providers.rs`

- [ ] **Step 1: Write failing handler integration tests**

Using the in-memory store and local-owner request extensions, cover:

- `GET /api/providers` field-level sources/status without a sentinel secret.
- `PUT`/`DELETE /api/providers/{id}/credential` with bounded non-empty keys.
- `PUT`/`DELETE /api/providers/ollama/connection` with independent persistence.
- `POST /api/providers/{id}/test` safe success/failure.
- `GET /api/providers/ollama/models` normalized bounded results.
- canonical aliases and unknown providers.
- `Cache-Control: no-store` on every administration response.
- vault/outbound failures mapped to stable codes with no secret, URL credential, upstream body, or keyring detail.

- [ ] **Step 2: Run and verify failure**

Run: `cargo test -p anima-daemon routes::providers::tests`

Expected: FAIL because the routes are absent.

- [ ] **Step 3: Implement request and response contracts**

Provider summaries must serialize:

```json
{
  "id": "ollama",
  "label": "Ollama (local)",
  "requiresKey": false,
  "configured": true,
  "credentialSource": "none",
  "baseUrl": "http://127.0.0.1:11434",
  "baseUrlSource": "catalog",
  "hasVaultCredential": false,
  "hasVaultBaseUrl": false,
  "supportsModelDiscovery": true,
  "apiKeyEnvs": []
}
```

Never add masked keys, suffixes, fingerprints, or vault identifiers.

- [ ] **Step 4: Mount handlers behind the guard**

Mount the exact spec routes. Apply owner validation before body reads or outbound resolution. Bound credential and URL bodies below the daemon-wide request maximum. Empty values are validation errors, never implicit deletes. Register paths and schemas in Utoipa.

- [ ] **Step 5: Run focused and full host tests**

Run: `cargo test -p anima-daemon routes::providers::tests`

Expected: PASS.

Run: `bun x nx run rust-daemon:test --skipNxCache`

Expected: PASS.

- [ ] **Step 6: Commit**

```powershell
git add hosts/rust-daemon/src/routes/providers.rs hosts/rust-daemon/src/routes/contracts/providers.rs hosts/rust-daemon/src/routes/contracts/mod.rs hosts/rust-daemon/src/routes/mod.rs hosts/rust-daemon/src/provider_runtime.rs
git commit --no-gpg-sign -m "feat: expose secure provider administration API"
```

### Task 8: Add The Provider Settings UI

**Files:**
- Modify: `apps/playground/src/lib/api.ts`
- Create: `apps/playground/src/components/ProviderSettings.tsx`
- Create: `apps/playground/src/components/ProviderSettings.spec.tsx`
- Modify: `apps/playground/src/components/Sidebar.tsx`
- Modify: `apps/playground/src/playground.tsx`
- Modify: `apps/playground/vite.config.mts`

- [ ] **Step 1: Add failing UI/API tests**

Use Vitest/jsdom and mocked fetch. Assert password inputs never receive an existing value, successful submission clears component-local state, request bodies contain a key only on explicit PUT, remove-key does not remove Ollama URL, remove-URL does not remove token, status labels reflect field sources, and refresh models never runs merely by rendering the page.

- [ ] **Step 2: Run and verify failure**

First confirm the inferred target after adding a minimal test config:

Run: `bun x nx show project @animaOS-SWARM/playground --json`

Then run: `bun x nx run @animaOS-SWARM/playground:test --skipNxCache`

Expected: FAIL because the API methods/component are absent. If no `test` target is inferred, add a `test` block to `vite.config.mts` (`environment: 'jsdom'`) and re-run `bun x nx show project ... --json`; do not bypass Nx.

- [ ] **Step 3: Extend the typed API client**

Add `CredentialSource`, `BaseUrlSource`, expanded `Provider`, `OllamaModel`, and methods `putCredential`, `deleteCredential`, `putOllamaConnection`, `deleteOllamaConnection`, `test`, and `listOllamaModels`. Do not accept or return secret-shaped fields from list/test/discovery responses.

- [ ] **Step 4: Build `ProviderSettings`**

Use restrained existing tokens and form primitives. Keep each password value inside its provider-row component; clear on success/unmount. Show vault/environment/none status, independent Ollama endpoint/token actions, explicit destructive confirmations, bounded busy states, sanitized errors, and refresh callbacks. Do not use browser storage, URL state, global app state, analytics, or masked-key display.

- [ ] **Step 5: Wire navigation and refresh**

Add a dedicated Providers button below the existing entity tabs. It opens settings without adding providers to `EntityKind` row/delete behavior. Lift a memoized `refreshProviders` into `Playground`, use it for polling and mutation success, and hide the irrelevant Memory Inspector while settings are open.

- [ ] **Step 6: Run UI tests, typecheck, and build**

Run: `bun x nx run @animaOS-SWARM/playground:test --skipNxCache`

Expected: PASS.

Run: `bun x nx run @animaOS-SWARM/playground:typecheck --skipNxCache`

Expected: PASS.

Run: `bun x nx run @animaOS-SWARM/playground:build --skipNxCache`

Expected: PASS.

- [ ] **Step 7: Commit**

```powershell
git add apps/playground/src/lib/api.ts apps/playground/src/components/ProviderSettings.tsx apps/playground/src/components/ProviderSettings.spec.tsx apps/playground/src/components/Sidebar.tsx apps/playground/src/playground.tsx apps/playground/vite.config.mts
git commit --no-gpg-sign -m "feat: add secure provider settings UI"
```

### Task 9: Integrate Ollama Model Discovery Into Agent Creation

**Files:**
- Modify: `apps/playground/src/components/CreateAgent.tsx`
- Modify: `apps/playground/src/components/ProviderSettings.spec.tsx`
- Create: `apps/playground/src/components/CreateAgent.spec.tsx`

- [ ] **Step 1: Write failing interaction tests**

Assert selecting Ollama loads installed models only after selection, choosing a model updates `AgentConfig.model`, discovery failure keeps a manual input available, manual mode preserves typed text, switching away from Ollama restores the ordinary model input, and no provider secret appears in state or requests.

- [ ] **Step 2: Run and verify failure**

Run: `bun x nx run @animaOS-SWARM/playground:test --skipNxCache`

Expected: FAIL because Create Agent has only a free-text model input.

- [ ] **Step 3: Implement discovered select plus manual fallback**

When `provider === 'ollama'`, fetch `/api/providers/ollama/models`, render a select for results, and expose an explicit “Enter model manually” action. Keep the currently chosen model valid across refreshes; if it disappears, retain it as a manual value rather than silently selecting a different model.

- [ ] **Step 4: Run tests and build**

Run: `bun x nx run @animaOS-SWARM/playground:test --skipNxCache`

Expected: PASS.

Run: `bun x nx run @animaOS-SWARM/playground:typecheck --skipNxCache`

Expected: PASS.

- [ ] **Step 5: Commit**

```powershell
git add apps/playground/src/components/CreateAgent.tsx apps/playground/src/components/CreateAgent.spec.tsx apps/playground/src/components/ProviderSettings.spec.tsx
git commit --no-gpg-sign -m "feat: select discovered Ollama models"
```

### Task 10: Document, Verify, And Manually Exercise The Complete Flow

**Files:**
- Modify: `apps/docs/src/content/docs/cli/providers.mdx`
- Modify: `docs/rust-daemon-architecture.md`
- Test: all touched projects

- [ ] **Step 1: Write operational documentation**

Document:

- `bun dev --host rust` and Playground `http://localhost:4201`.
- OS-vault persistence and the same-user compromise limitation.
- exact default Origins and `ANIMA_ALLOWED_UI_ORIGINS` replacement semantics.
- `ANIMA_LOCAL_ADMIN_TOKEN` for originless clients.
- composition with `ANIMAOS_RS_API_KEY`: global key in `X-Api-Key`, local-owner token in `Authorization` when different.
- environment fallback and field-level precedence.
- Ollama default URL, private CIDR opt-in, HTTPS requirement for non-loopback tokens, DNS/redirect policy, and model discovery.
- deletion behavior and safe recovery when the OS vault is unavailable.

- [ ] **Step 2: Run formatting checks on touched Rust files**

Run: `cargo fmt --all --check`

Expected: PASS. If unrelated existing format drift remains, run `cargo fmt --all --check` from the isolated worktree and distinguish untouched baseline failures before changing anything outside this feature.

- [ ] **Step 3: Run package and host verification through Nx**

Run: `bun x nx run core-rust:test --skipNxCache`

Expected: PASS.

Run: `bun x nx run rust-daemon:test --skipNxCache`

Expected: PASS. On Windows executable-lock failure only, rerun with `$env:CI='1'; $env:CARGO_TARGET_DIR='target/validation-rust-daemon'; bun x nx run rust-daemon:test --skipNxCache`.

- [ ] **Step 4: Run Playground verification through Nx**

Run: `bun x nx run @animaOS-SWARM/playground:test --skipNxCache`

Expected: PASS.

Run: `bun x nx run @animaOS-SWARM/playground:typecheck --skipNxCache`

Expected: PASS.

Run: `bun x nx run @animaOS-SWARM/playground:build --skipNxCache`

Expected: PASS.

- [ ] **Step 5: Run docs verification**

Run: `bun x nx run @animaOS-SWARM/docs:build --skipNxCache`

Expected: PASS.

- [ ] **Step 6: Perform a local smoke test without exposing the key**

Run `bun dev --host rust`, open `http://localhost:4201`, then verify:

1. Add a disposable test-provider key through Provider Settings.
2. Reload the browser and confirm only “UI vault” status returns, never the key.
3. Restart the daemon and confirm vault status persists.
4. Replace then remove the key; confirm environment fallback appears when configured.
5. Save `http://127.0.0.1:11434`, connect to Ollama, refresh models, choose one, create an agent, and run a prompt.
6. Remove only the Ollama token and confirm the URL remains; remove only the URL and confirm the token remains/falls back independently.
7. From an unapproved Origin and an originless request without the admin token, confirm administration is rejected before outbound traffic.

Do not paste a real provider key into test output, screenshots, shell history, or the plan journal.

- [ ] **Step 7: Inspect the final diff and commit docs**

Run: `git diff --check`

Run: `git status --short`

Confirm only feature files are staged; preserve unrelated user changes.

```powershell
git add apps/docs/src/content/docs/cli/providers.mdx docs/rust-daemon-architecture.md
git commit --no-gpg-sign -m "docs: explain secure provider settings"
```

---

## Acceptance Matrix

| Requirement | Primary proof |
|---|---|
| Browser cannot read stored keys | Provider API integration tests plus UI tests |
| Secrets survive restart securely | keyring adapter contract plus manual restart smoke test |
| No plaintext fallback | keyring error-path tests |
| Vault and environment merge by field | `provider_runtime` unit tests |
| Changes apply without restart | runtime adapter two-operation test |
| Local-owner administration fails closed | owner-policy matrix and side-effect counter |
| Ollama cannot be used as unrestricted SSRF | endpoint-policy DNS/CIDR/redirect tests |
| One policy covers every Ollama operation | adapter integration tests for native/tools/test/discovery |
| Ollama models are selectable with fallback | Create Agent component tests and smoke test |
| Existing workflows remain healthy | Nx core-rust, rust-daemon, Playground, and docs targets |
