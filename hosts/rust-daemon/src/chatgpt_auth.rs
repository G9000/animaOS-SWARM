#[cfg(test)]
mod tests {
    use super::*;
    /// Opt-in transport check: uses the saved subscription and consumes a small request.
    #[tokio::test]
    #[ignore = "requires a connected ChatGPT account and consumes subscription usage"]
    async fn subscription_live_smoke_returns_nonempty_text() {
        use anima_core::{
            AgentConfig, Content, Message, MessageRole, ModelAdapter, ModelGenerateRequest,
        };
        let config: AgentConfig = serde_json::from_value(serde_json::json!({
            "name":"Subscription smoke test", "provider":"chatgpt", "model":"gpt-5.5"
        }))
        .unwrap();
        let request = ModelGenerateRequest {
            system: "Reply briefly.".into(),
            messages: vec![Message {
                id: "smoke".into(),
                agent_id: "smoke".into(),
                room_id: "smoke".into(),
                role: MessageRole::User,
                created_at_ms: 0,
                content: Content {
                    text: "Reply with exactly OK.".into(),
                    attachments: None,
                    metadata: None,
                },
            }],
            temperature: None,
            max_tokens: None,
        };
        let adapter = crate::runtime_model::RuntimeModelAdapter::from_env(ChatGptAuth::new());
        let result = adapter.generate(&config, &request).await.unwrap();
        assert!(!result.content.text.trim().is_empty());
        println!(
            "Live subscription reply received: {} bytes",
            result.content.text.len()
        );
    }
    #[tokio::test]
    async fn disconnected_status_is_redacted_and_isolated() {
        let first = ChatGptAuth::in_memory();
        let second = ChatGptAuth::in_memory();
        let status = serde_json::to_value(first.status().await.unwrap()).unwrap();
        assert_eq!(
            status,
            serde_json::json!({"connected":false,"accountId":null,"planType":null,"login":null,"error":null})
        );
        assert!(second.usable_credential().await.is_err());
    }
}

use crate::connectors::oauth_apps::{InMemoryOAuthAppVault, OAuthVaultBackend, OsOAuthVault};
use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;
use zeroize::Zeroizing;

const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const SERVICE: &str = "anima.chatgpt.subscription.v1";
const ACCOUNT: &str = "account";
const AUTH: &str = "https://auth.openai.com";
const VAULT_ERROR: &str = "ChatGPT credential vault is unavailable";
const AUTH_ERROR: &str = "ChatGPT authorization failed; start sign-in again";
fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

#[derive(Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LoginStatus {
    pub user_code: String,
    pub verification_url: String,
    pub expires_at_ms: u64,
}
#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Status {
    pub connected: bool,
    pub account_id: Option<String>,
    pub plan_type: Option<String>,
    pub login: Option<LoginStatus>,
    pub error: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Credential {
    access_token: String,
    refresh_token: String,
    account_id: String,
    plan_type: Option<String>,
    expires_at_ms: u64,
}
impl Drop for Credential {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.access_token.zeroize();
        self.refresh_token.zeroize();
    }
}
struct Device {
    id: Zeroizing<String>,
    status: LoginStatus,
    interval: u64,
}
#[async_trait]
trait AuthTransport: Send + Sync {
    async fn start(&self) -> Result<Device, String>;
    async fn poll(&self, device: &Device) -> Result<Option<Credential>, String>;
    async fn refresh(&self, credential: &Credential) -> Result<Credential, String>;
}
struct OfficialTransport {
    client: reqwest::Client,
}
impl OfficialTransport {
    fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(30))
                .build()
                .expect("valid fixed auth client"),
        }
    }
    async fn tokens(
        &self,
        form: &[(&str, &str)],
        previous: Option<&Credential>,
    ) -> Result<Credential, String> {
        let response = self
            .client
            .post(format!("{AUTH}/oauth/token"))
            .form(form)
            .send()
            .await
            .map_err(|_| AUTH_ERROR.to_string())?;
        if !response.status().is_success() {
            return Err(AUTH_ERROR.into());
        }
        let text = Zeroizing::new(response.text().await.map_err(|_| AUTH_ERROR.to_string())?);
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|_| AUTH_ERROR.to_string())?;
        let access = value["access_token"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or(AUTH_ERROR)?;
        let claims = value["id_token"]
            .as_str()
            .and_then(jwt_claims)
            .or_else(|| jwt_claims(access));
        let auth = claims
            .as_ref()
            .and_then(|v| v.get("https://api.openai.com/auth"));
        let access_claims = jwt_claims(access);
        let access_auth = access_claims
            .as_ref()
            .and_then(|v| v.get("https://api.openai.com/auth"));
        let account = auth
            .and_then(|v| v["chatgpt_account_id"].as_str())
            .or_else(|| access_auth.and_then(|v| v["chatgpt_account_id"].as_str()))
            .map(str::to_owned)
            .or_else(|| previous.map(|p| p.account_id.clone()))
            .ok_or(AUTH_ERROR)?;
        let plan = auth
            .and_then(|v| v["chatgpt_plan_type"].as_str())
            .or_else(|| access_auth.and_then(|v| v["chatgpt_plan_type"].as_str()))
            .map(str::to_owned)
            .or_else(|| previous.and_then(|p| p.plan_type.clone()));
        let refresh = value["refresh_token"]
            .as_str()
            .filter(|s| !s.is_empty())
            .or_else(|| previous.map(|p| p.refresh_token.as_str()))
            .ok_or(AUTH_ERROR)?;
        let expiry = jwt_claims(access)
            .and_then(|v| v["exp"].as_u64())
            .map(|s| s.saturating_mul(1000))
            .unwrap_or_else(|| {
                now_ms().saturating_add(
                    value["expires_in"]
                        .as_u64()
                        .unwrap_or(3600)
                        .saturating_mul(1000),
                )
            });
        Ok(Credential {
            access_token: access.into(),
            refresh_token: refresh.into(),
            account_id: account,
            plan_type: plan,
            expires_at_ms: expiry,
        })
    }
}
fn jwt_claims(token: &str) -> Option<serde_json::Value> {
    let bytes = URL_SAFE_NO_PAD.decode(token.split('.').nth(1)?).ok()?;
    serde_json::from_slice(&bytes).ok()
}
#[async_trait]
impl AuthTransport for OfficialTransport {
    async fn start(&self) -> Result<Device, String> {
        let r = self
            .client
            .post(format!("{AUTH}/api/accounts/deviceauth/usercode"))
            .json(&serde_json::json!({"client_id":CLIENT_ID}))
            .send()
            .await
            .map_err(|_| AUTH_ERROR.to_string())?;
        if !r.status().is_success() {
            return Err(AUTH_ERROR.into());
        }
        let v: serde_json::Value = r.json().await.map_err(|_| AUTH_ERROR.to_string())?;
        let code = v["user_code"]
            .as_str()
            .or_else(|| v["usercode"].as_str())
            .filter(|s| !s.is_empty())
            .ok_or(AUTH_ERROR)?;
        Ok(Device {
            id: Zeroizing::new(v["device_auth_id"].as_str().ok_or(AUTH_ERROR)?.into()),
            interval: v["interval"]
                .as_u64()
                .or_else(|| v["interval"].as_str().and_then(|s| s.parse().ok()))
                .unwrap_or(5)
                .clamp(1, 60),
            status: LoginStatus {
                user_code: code.into(),
                verification_url: format!("{AUTH}/codex/device"),
                expires_at_ms: now_ms() + 900_000,
            },
        })
    }
    async fn poll(&self, d: &Device) -> Result<Option<Credential>, String> {
        let r = self
            .client
            .post(format!("{AUTH}/api/accounts/deviceauth/token"))
            .json(
                &serde_json::json!({"device_auth_id":d.id.as_str(),"user_code":d.status.user_code}),
            )
            .send()
            .await
            .map_err(|_| AUTH_ERROR.to_string())?;
        if matches!(r.status().as_u16(), 403 | 404) {
            return Ok(None);
        }
        if !r.status().is_success() {
            return Err(AUTH_ERROR.into());
        }
        let text = Zeroizing::new(r.text().await.map_err(|_| AUTH_ERROR.to_string())?);
        let v: serde_json::Value =
            serde_json::from_str(&text).map_err(|_| AUTH_ERROR.to_string())?;
        self.tokens(
            &[
                ("grant_type", "authorization_code"),
                ("client_id", CLIENT_ID),
                ("code", v["authorization_code"].as_str().ok_or(AUTH_ERROR)?),
                (
                    "code_verifier",
                    v["code_verifier"].as_str().ok_or(AUTH_ERROR)?,
                ),
                (
                    "redirect_uri",
                    "https://auth.openai.com/deviceauth/callback",
                ),
            ],
            None,
        )
        .await
        .map(Some)
    }
    async fn refresh(&self, c: &Credential) -> Result<Credential, String> {
        self.tokens(
            &[
                ("grant_type", "refresh_token"),
                ("client_id", CLIENT_ID),
                ("refresh_token", &c.refresh_token),
            ],
            Some(c),
        )
        .await
    }
}
#[derive(Default)]
struct Lifecycle {
    generation: u64,
    login: Option<LoginStatus>,
    error: Option<String>,
}
#[derive(Clone)]
pub(crate) struct ChatGptAuth {
    lifecycle: Arc<Mutex<Lifecycle>>,
    refresh_lock: Arc<Mutex<()>>,
    start_lock: Arc<Mutex<()>>,
    vault: Arc<dyn OAuthVaultBackend>,
    transport: Arc<dyn AuthTransport>,
}
impl ChatGptAuth {
    pub fn new() -> Self {
        Self::with_dependencies(Arc::new(OsOAuthVault), Arc::new(OfficialTransport::new()))
    }
    pub fn in_memory() -> Self {
        Self::with_dependencies(
            Arc::new(InMemoryOAuthAppVault::default()),
            Arc::new(UnavailableTransport),
        )
    }
    fn with_dependencies(
        vault: Arc<dyn OAuthVaultBackend>,
        transport: Arc<dyn AuthTransport>,
    ) -> Self {
        Self {
            vault,
            transport,
            lifecycle: Arc::new(Mutex::new(Lifecycle::default())),
            refresh_lock: Arc::new(Mutex::new(())),
            start_lock: Arc::new(Mutex::new(())),
        }
    }
    fn manifest(&self) -> Result<Option<Manifest>, String> {
        self.vault
            .load(SERVICE, ACCOUNT)
            .map_err(|_| VAULT_ERROR.to_string())?
            .map(|s| {
                let manifest: Manifest =
                    serde_json::from_str(&s).map_err(|_| VAULT_ERROR.to_string())?;
                if manifest.version != 1
                    || manifest.chunks == 0
                    || manifest.chunks > 132
                    || uuid::Uuid::parse_str(&manifest.generation).is_err()
                {
                    return Err(VAULT_ERROR.into());
                }
                Ok(manifest)
            })
            .transpose()
    }
    fn load(&self) -> Result<Option<Credential>, String> {
        let Some(m) = self.manifest()? else {
            return Ok(None);
        };
        let mut payload = Zeroizing::new(String::new());
        for index in 0..m.chunks {
            let chunk = self
                .vault
                .load(SERVICE, &m.key(index))
                .map_err(|_| VAULT_ERROR.to_string())?
                .ok_or(VAULT_ERROR)?;
            if chunk.len() > 2000 || payload.len() + chunk.len() > 65536 {
                return Err(VAULT_ERROR.into());
            }
            payload.push_str(&chunk);
        }
        let credential: Credential =
            serde_json::from_str(&payload).map_err(|_| VAULT_ERROR.to_string())?;
        if credential.access_token.is_empty()
            || credential.refresh_token.is_empty()
            || credential.account_id.is_empty()
        {
            return Err(VAULT_ERROR.into());
        }
        Ok(Some(credential))
    }
    fn clean_chunks(&self, m: &Manifest) -> Result<(), String> {
        for index in 0..m.chunks {
            self.vault
                .delete(SERVICE, &m.key(index))
                .map_err(|_| VAULT_ERROR.to_string())?;
        }
        Ok(())
    }
    fn recover_cleanup(&self) -> Result<(), String> {
        let Some(payload) = self
            .vault
            .load(SERVICE, "cleanup")
            .map_err(|_| VAULT_ERROR.to_string())?
        else {
            return Ok(());
        };
        let journal: Vec<Manifest> =
            serde_json::from_str(&payload).map_err(|_| VAULT_ERROR.to_string())?;
        if journal.len() > 2
            || journal.iter().any(|m| {
                m.version != 1
                    || m.chunks == 0
                    || m.chunks > 132
                    || uuid::Uuid::parse_str(&m.generation).is_err()
            })
        {
            return Err(VAULT_ERROR.into());
        }
        let active = self.manifest()?;
        for generation in journal {
            // A failed manifest write may have succeeded. Never remove active secrets.
            if active
                .as_ref()
                .is_some_and(|m| m.generation == generation.generation)
            {
                continue;
            }
            self.clean_chunks(&generation)?;
        }
        self.vault
            .delete(SERVICE, "cleanup")
            .map_err(|_| VAULT_ERROR.to_string())
    }
    fn save(&self, c: &Credential) -> Result<(), String> {
        self.recover_cleanup()?;
        let previous = self.manifest()?;
        let payload =
            Zeroizing::new(serde_json::to_string(c).map_err(|_| VAULT_ERROR.to_string())?);
        if payload.len() > 65536 {
            return Err(VAULT_ERROR.into());
        }
        // Windows Credential Manager limits each UTF-16 secret to 2560 bytes.
        // Immutable chunks precede the manifest; readers see one complete generation.
        let chars: Zeroizing<Vec<char>> = Zeroizing::new(payload.chars().collect());
        let chunks = chars
            .chunks(500)
            .map(|c| Zeroizing::new(c.iter().collect::<String>()))
            .collect::<Vec<_>>();
        let manifest = Manifest {
            version: 1,
            generation: uuid::Uuid::new_v4().to_string(),
            chunks: chunks.len(),
        };
        // Persist cleanup intent before any chunk is written. This journal fits in one
        // Windows keyring entry and survives partial writes, rotations, and restarts.
        let mut journal = previous.into_iter().collect::<Vec<_>>();
        journal.push(manifest.clone());
        let intent = serde_json::to_string(&journal).map_err(|_| VAULT_ERROR.to_string())?;
        self.vault
            .put(SERVICE, "cleanup", &intent)
            .map_err(|_| VAULT_ERROR.to_string())?;
        for (index, chunk) in chunks.iter().enumerate() {
            if self
                .vault
                .put(SERVICE, &manifest.key(index), chunk)
                .is_err()
            {
                // The durable journal owns cleanup on the next mutation.
                return Err(VAULT_ERROR.into());
            }
        }
        let serialized = serde_json::to_string(&manifest).map_err(|_| VAULT_ERROR.to_string())?;
        if self.vault.put(SERVICE, ACCOUNT, &serialized).is_err() {
            return Err(VAULT_ERROR.into());
        }
        self.recover_cleanup()
    }
    fn disconnect_vault(&self) -> Result<(), String> {
        self.recover_cleanup()?;
        if let Some(active) = self.manifest()? {
            // Keep the manifest until every chunk is removed, so deletion can retry
            // after a failure or process exit without forgetting remaining secrets.
            self.clean_chunks(&active)?;
            self.vault
                .delete(SERVICE, ACCOUNT)
                .map_err(|_| VAULT_ERROR.to_string())?;
        }
        Ok(())
    }
    pub async fn status(&self) -> Result<Status, String> {
        let l = self.lifecycle.lock().await;
        let c = self.load()?;
        Ok(Status {
            connected: c.is_some(),
            account_id: c.as_ref().map(|c| c.account_id.clone()),
            plan_type: c.as_ref().and_then(|c| c.plan_type.clone()),
            login: l.login.clone(),
            error: l.error.clone(),
        })
    }
    pub async fn login(&self) -> Result<Status, String> {
        let _start = self.start_lock.lock().await;
        self.load()?; // Fail before initiating device authorization if the vault cannot be read.
        let generation = {
            let mut l = self.lifecycle.lock().await;
            if l.login.as_ref().is_some_and(|d| d.expires_at_ms > now_ms()) {
                drop(l);
                return self.status().await;
            }
            l.generation += 1;
            l.error = None;
            l.generation
        };
        let d = self.transport.start().await?;
        {
            let mut l = self.lifecycle.lock().await;
            if l.generation != generation {
                return Err("ChatGPT sign-in cancelled".into());
            }
            l.login = Some(d.status.clone());
        }
        let service = self.clone();
        tokio::spawn(async move {
            service.poll_login(generation, d).await;
        });
        self.status().await
    }
    async fn poll_login(&self, generation: u64, device: Device) {
        loop {
            if self.lifecycle.lock().await.generation != generation {
                return;
            }
            let remaining = device.status.expires_at_ms.saturating_sub(now_ms());
            let result = if remaining == 0 {
                Err("ChatGPT sign-in expired; start again".into())
            } else {
                match tokio::time::timeout(
                    Duration::from_millis(remaining),
                    self.transport.poll(&device),
                )
                .await
                {
                    Ok(r) => r,
                    Err(_) => Err("ChatGPT sign-in expired; start again".into()),
                }
            };
            let mut l = self.lifecycle.lock().await;
            if l.generation != generation {
                return;
            }
            match result {
                Ok(Some(c)) => {
                    l.error = self.save(&c).err();
                    l.generation += 1;
                    l.login = None;
                    return;
                }
                Err(e) => {
                    l.error = Some(e);
                    l.login = None;
                    return;
                }
                Ok(None) => {}
            }
            drop(l);
            tokio::time::sleep(
                Duration::from_secs(device.interval).min(Duration::from_millis(
                    device.status.expires_at_ms.saturating_sub(now_ms()),
                )),
            )
            .await;
        }
    }
    pub async fn cancel(&self, disconnect: bool) -> Result<Status, String> {
        {
            let mut l = self.lifecycle.lock().await;
            l.generation += 1;
            l.login = None;
            l.error = None;
            if disconnect {
                self.disconnect_vault()?;
            }
        }
        self.status().await
    }
    pub async fn usable_credential(&self) -> Result<(String, String), String> {
        let _refresh = self.refresh_lock.lock().await;
        let (generation, c) = {
            let l = self.lifecycle.lock().await;
            (
                l.generation,
                self.load()?
                    .ok_or("ChatGPT is disconnected; sign in in settings")?,
            )
        };
        if c.expires_at_ms > now_ms() + 60_000 {
            return Ok((c.access_token.clone(), c.account_id.clone()));
        }
        let refreshed = self.transport.refresh(&c).await;
        let mut l = self.lifecycle.lock().await;
        if l.generation != generation {
            return Err("ChatGPT authorization changed; retry".into());
        }
        match refreshed {
            Ok(c) => {
                self.save(&c)?;
                l.error = None;
                Ok((c.access_token.clone(), c.account_id.clone()))
            }
            Err(e) => {
                l.error = Some(e.clone());
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;
    struct Transport {
        polls: AtomicUsize,
        refreshes: AtomicUsize,
        entered: Notify,
        release: Notify,
    }
    impl Transport {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                polls: AtomicUsize::new(0),
                refreshes: AtomicUsize::new(0),
                entered: Notify::new(),
                release: Notify::new(),
            })
        }
    }
    fn credential(expired: bool) -> Credential {
        Credential {
            access_token: "secret-access".into(),
            refresh_token: "secret-refresh".into(),
            account_id: "account-1".into(),
            plan_type: Some("plus".into()),
            expires_at_ms: if expired { 0 } else { now_ms() + 3600_000 },
        }
    }
    #[async_trait]
    impl AuthTransport for Transport {
        async fn start(&self) -> Result<Device, String> {
            Ok(Device {
                id: Zeroizing::new("secret-device".into()),
                interval: 1,
                status: LoginStatus {
                    user_code: "USER-CODE".into(),
                    verification_url: "https://auth.openai.com/codex/device".into(),
                    expires_at_ms: now_ms() + 900_000,
                },
            })
        }
        async fn poll(&self, _: &Device) -> Result<Option<Credential>, String> {
            self.polls.fetch_add(1, Ordering::SeqCst);
            self.entered.notify_one();
            self.release.notified().await;
            Ok(Some(credential(false)))
        }
        async fn refresh(&self, _: &Credential) -> Result<Credential, String> {
            self.refreshes.fetch_add(1, Ordering::SeqCst);
            self.entered.notify_one();
            self.release.notified().await;
            Ok(credential(false))
        }
    }
    #[tokio::test]
    async fn cancel_discards_inflight_login_and_duplicate_start_is_idempotent() {
        let t = Transport::new();
        let service =
            ChatGptAuth::with_dependencies(Arc::new(InMemoryOAuthAppVault::default()), t.clone());
        let first = service.login().await.unwrap();
        t.entered.notified().await;
        let second = service.login().await.unwrap();
        assert_eq!(
            first.login.unwrap().user_code,
            second.login.unwrap().user_code
        );
        service.cancel(false).await.unwrap();
        t.release.notify_one();
        tokio::task::yield_now().await;
        assert!(!service.status().await.unwrap().connected);
        assert!(service.load().unwrap().is_none());
        assert_eq!(t.polls.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn refresh_is_serialized_and_disconnect_prevents_resurrection() {
        let t = Transport::new();
        let service =
            ChatGptAuth::with_dependencies(Arc::new(InMemoryOAuthAppVault::default()), t.clone());
        service.save(&credential(true)).unwrap();
        let first = {
            let s = service.clone();
            tokio::spawn(async move { s.usable_credential().await })
        };
        t.entered.notified().await;
        let second = {
            let s = service.clone();
            tokio::spawn(async move { s.usable_credential().await })
        };
        service.cancel(true).await.unwrap();
        t.release.notify_one();
        assert!(first.await.unwrap().is_err());
        assert!(second.await.unwrap().is_err());
        assert_eq!(t.refreshes.load(Ordering::SeqCst), 1);
        assert!(service.load().unwrap().is_none());
    }
    #[tokio::test]
    async fn concurrent_refresh_uses_rotated_persisted_token_once() {
        let t = Transport::new();
        let vault = Arc::new(InMemoryOAuthAppVault::default());
        let service = ChatGptAuth::with_dependencies(vault.clone(), t.clone());
        service.save(&credential(true)).unwrap();
        let first = {
            let s = service.clone();
            tokio::spawn(async move { s.usable_credential().await })
        };
        t.entered.notified().await;
        let second = {
            let s = service.clone();
            tokio::spawn(async move { s.usable_credential().await })
        };
        t.release.notify_one();
        assert!(first.await.unwrap().is_ok());
        assert!(second.await.unwrap().is_ok());
        assert_eq!(t.refreshes.load(Ordering::SeqCst), 1);
        let restored = ChatGptAuth::with_dependencies(vault, t);
        let json = serde_json::to_string(&restored.status().await.unwrap()).unwrap();
        assert!(json.contains("account-1"));
        assert!(!json.contains("secret"));
        assert!(restored.usable_credential().await.is_ok());
    }
    #[tokio::test]
    async fn expired_login_never_polls_and_reports_expiry() {
        let t = Transport::new();
        let service =
            ChatGptAuth::with_dependencies(Arc::new(InMemoryOAuthAppVault::default()), t.clone());
        let mut d = t.start().await.unwrap();
        d.status.expires_at_ms = 0;
        service.poll_login(0, d).await;
        let status = service.status().await.unwrap();
        assert!(status.login.is_none());
        assert!(status.error.unwrap().contains("expired"));
        assert_eq!(t.polls.load(Ordering::SeqCst), 0);
    }
}
struct UnavailableTransport;
#[async_trait]
impl AuthTransport for UnavailableTransport {
    async fn start(&self) -> Result<Device, String> {
        Err("ChatGPT sign-in is unavailable in this runtime".into())
    }
    async fn poll(&self, _: &Device) -> Result<Option<Credential>, String> {
        Err("ChatGPT sign-in is unavailable in this runtime".into())
    }
    async fn refresh(&self, _: &Credential) -> Result<Credential, String> {
        Err("ChatGPT sign-in is unavailable in this runtime".into())
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct Manifest {
    version: u8,
    generation: String,
    chunks: usize,
}
impl Manifest {
    fn key(&self, index: usize) -> String {
        format!("{}.{}", self.generation, index)
    }
}

#[cfg(test)]
mod vault_and_generation_tests {
    use super::*;
    use crate::connectors::oauth_apps::OAuthVaultError;
    use tokio::sync::Notify;
    struct LimitedVault(InMemoryOAuthAppVault);
    impl OAuthVaultBackend for LimitedVault {
        fn load(&self, s: &str, a: &str) -> Result<Option<Zeroizing<String>>, OAuthVaultError> {
            self.0.load(s, a)
        }
        fn put(&self, s: &str, a: &str, p: &str) -> Result<(), OAuthVaultError> {
            if p.encode_utf16().count() * 2 > 2560 {
                return Err(OAuthVaultError::Unavailable);
            }
            self.0.put(s, a, p)
        }
        fn delete(&self, s: &str, a: &str) -> Result<(), OAuthVaultError> {
            self.0.delete(s, a)
        }
    }
    fn credential(account: &str) -> Credential {
        Credential {
            access_token: "jwt".repeat(1400),
            refresh_token: "refresh".repeat(500),
            account_id: account.into(),
            plan_type: None,
            expires_at_ms: 0,
        }
    }
    #[tokio::test]
    async fn windows_sized_vault_chunks_round_trip_rotate_and_disconnect() {
        let vault = Arc::new(LimitedVault(InMemoryOAuthAppVault::default()));
        let service = ChatGptAuth::with_dependencies(vault.clone(), Arc::new(UnavailableTransport));
        service.save(&credential("first")).unwrap();
        let old = service.manifest().unwrap().unwrap();
        assert!(old.chunks > 1);
        assert_eq!(service.load().unwrap().unwrap().access_token.len(), 4200);
        service.save(&credential("second")).unwrap();
        assert!(vault.load(SERVICE, &old.key(0)).unwrap().is_none());
        let current = service.manifest().unwrap().unwrap();
        service.cancel(true).await.unwrap();
        assert!(service.load().unwrap().is_none());
        assert!(vault.load(SERVICE, &current.key(0)).unwrap().is_none());
    }
    struct RaceTransport {
        refresh_entered: Notify,
        release_refresh: Notify,
    }
    #[async_trait]
    impl AuthTransport for RaceTransport {
        async fn start(&self) -> Result<Device, String> {
            Ok(Device {
                id: Zeroizing::new("device".into()),
                interval: 1,
                status: LoginStatus {
                    user_code: "code".into(),
                    verification_url: "https://auth.openai.com/codex/device".into(),
                    expires_at_ms: now_ms() + 900_000,
                },
            })
        }
        async fn poll(&self, _: &Device) -> Result<Option<Credential>, String> {
            Ok(Some(credential("new-account")))
        }
        async fn refresh(&self, _: &Credential) -> Result<Credential, String> {
            self.refresh_entered.notify_one();
            self.release_refresh.notified().await;
            Ok(credential("old-account"))
        }
    }
    #[tokio::test]
    async fn successful_login_invalidates_refresh_started_during_pending_login() {
        let transport = Arc::new(RaceTransport {
            refresh_entered: Notify::new(),
            release_refresh: Notify::new(),
        });
        let service = ChatGptAuth::with_dependencies(
            Arc::new(InMemoryOAuthAppVault::default()),
            transport.clone(),
        );
        service.save(&credential("old-account")).unwrap();
        // Login was initiated, then an old account refresh started while it was pending.
        service.lifecycle.lock().await.generation = 1;
        let refresh = {
            let s = service.clone();
            tokio::spawn(async move { s.usable_credential().await })
        };
        transport.refresh_entered.notified().await;
        service
            .poll_login(1, transport.start().await.unwrap())
            .await;
        transport.release_refresh.notify_one();
        assert!(refresh.await.unwrap().is_err());
        assert_eq!(service.load().unwrap().unwrap().account_id, "new-account");
    }
}

#[cfg(test)]
mod cleanup_tests {
    use super::*;
    use crate::connectors::oauth_apps::OAuthVaultError;
    use std::{
        collections::HashSet,
        sync::{
            atomic::{AtomicBool, Ordering},
            Mutex as StdMutex,
        },
    };
    #[derive(Default)]
    struct FailingVault {
        inner: InMemoryOAuthAppVault,
        keys: StdMutex<HashSet<String>>,
        fail_delete: AtomicBool,
        fail_manifest: AtomicBool,
    }
    impl OAuthVaultBackend for FailingVault {
        fn load(&self, s: &str, a: &str) -> Result<Option<Zeroizing<String>>, OAuthVaultError> {
            self.inner.load(s, a)
        }
        fn put(&self, s: &str, a: &str, p: &str) -> Result<(), OAuthVaultError> {
            if a == ACCOUNT && self.fail_manifest.swap(false, Ordering::SeqCst) {
                return Err(OAuthVaultError::Unavailable);
            }
            self.inner.put(s, a, p)?;
            self.keys.lock().unwrap().insert(a.into());
            Ok(())
        }
        fn delete(&self, s: &str, a: &str) -> Result<(), OAuthVaultError> {
            if a.contains('.') && self.fail_delete.swap(false, Ordering::SeqCst) {
                return Err(OAuthVaultError::Unavailable);
            }
            self.inner.delete(s, a)?;
            self.keys.lock().unwrap().remove(a);
            Ok(())
        }
    }
    fn credential() -> Credential {
        Credential {
            access_token: "access".repeat(1000),
            refresh_token: "refresh".into(),
            account_id: "account".into(),
            plan_type: None,
            expires_at_ms: 0,
        }
    }
    #[tokio::test]
    async fn failed_disconnect_retains_manifest_until_retry_removes_all_secrets() {
        let vault = Arc::new(FailingVault::default());
        let service = ChatGptAuth::with_dependencies(vault.clone(), Arc::new(UnavailableTransport));
        service.save(&credential()).unwrap();
        vault.fail_delete.store(true, Ordering::SeqCst);
        assert!(service.cancel(true).await.is_err());
        assert!(service.manifest().unwrap().is_some());
        service.cancel(true).await.unwrap();
        assert!(vault.keys.lock().unwrap().is_empty());
    }
    #[tokio::test]
    async fn manifest_failure_staged_chunks_recover_on_restarted_service_retry() {
        let vault = Arc::new(FailingVault::default());
        let service = ChatGptAuth::with_dependencies(vault.clone(), Arc::new(UnavailableTransport));
        service.save(&credential()).unwrap();
        vault.fail_manifest.store(true, Ordering::SeqCst);
        assert!(service.save(&credential()).is_err());
        let restored =
            ChatGptAuth::with_dependencies(vault.clone(), Arc::new(UnavailableTransport));
        restored.save(&credential()).unwrap();
        restored.cancel(true).await.unwrap();
        assert!(vault.keys.lock().unwrap().is_empty());
    }
    #[tokio::test]
    async fn rotation_cleanup_failure_journal_survives_until_retry() {
        let vault = Arc::new(FailingVault::default());
        let service = ChatGptAuth::with_dependencies(vault.clone(), Arc::new(UnavailableTransport));
        service.save(&credential()).unwrap();
        vault.fail_delete.store(true, Ordering::SeqCst);
        assert!(service.save(&credential()).is_err());
        assert!(service.load().unwrap().is_some());
        let restored =
            ChatGptAuth::with_dependencies(vault.clone(), Arc::new(UnavailableTransport));
        restored.cancel(true).await.unwrap();
        assert!(vault.keys.lock().unwrap().is_empty());
    }
}
