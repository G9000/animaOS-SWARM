use serde::{Deserialize, Serialize};
use std::{fmt, future::Future, sync::Arc};
use tokio::sync::{oneshot, Mutex, OwnedMutexGuard};
use zeroize::{Zeroize, Zeroizing};

const SERVICE: &str = "animaos.oauth.apps";
const VERSION: u64 = 1;
const GOOGLE_MAIL: &str = "http://127.0.0.1:8080/api/connectors/mail/gmail/callback";
const GOOGLE_CALENDAR: &str = "http://127.0.0.1:8080/api/connectors/gcalendar/callback";
const MICROSOFT_MAIL: &str = "http://127.0.0.1:8080/api/connectors/mail/outlook/callback";

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum OAuthProvider {
    Google,
    Microsoft,
}
impl OAuthProvider {
    pub(crate) const ALL: [Self; 2] = [Self::Google, Self::Microsoft];
    pub(crate) fn redirect_uris(self) -> &'static [&'static str] {
        match self {
            Self::Google => &[GOOGLE_MAIL, GOOGLE_CALENDAR],
            Self::Microsoft => &[MICROSOFT_MAIL],
        }
    }
    fn account(self) -> &'static str {
        match self {
            Self::Google => "provider:google",
            Self::Microsoft => "provider:microsoft",
        }
    }
    fn env(self) -> (&'static str, &'static str, Option<&'static str>) {
        match self {
            Self::Google => ("ANIMA_GOOGLE_CLIENT_ID", "ANIMA_GOOGLE_CLIENT_SECRET", None),
            Self::Microsoft => (
                "ANIMA_MICROSOFT_CLIENT_ID",
                "ANIMA_MICROSOFT_CLIENT_SECRET",
                Some("ANIMA_MICROSOFT_TENANT"),
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum OAuthAppSource {
    Vault,
    Environment,
}

pub(crate) struct OAuthAppCredentials {
    client_id: String,
    secret: Zeroizing<String>,
    tenant: Option<String>,
}
impl OAuthAppCredentials {
    pub(crate) fn new(
        provider: OAuthProvider,
        client_id: impl Into<String>,
        secret: impl Into<String>,
        tenant: Option<String>,
    ) -> Result<Self, OAuthAppError> {
        let client_id = valid_text(client_id.into(), 2048).ok_or(OAuthAppError::InvalidClientId)?;
        let secret = valid_text(secret.into(), 4096).ok_or(OAuthAppError::InvalidClientSecret)?;
        let tenant = tenant_for(provider, tenant)?;
        Ok(Self {
            client_id,
            secret: Zeroizing::new(secret),
            tenant,
        })
    }
    pub(crate) fn client_id(&self) -> &str {
        &self.client_id
    }
    pub(crate) fn client_secret(&self) -> &str {
        self.secret.as_str()
    }
    pub(crate) fn tenant(&self) -> Option<&str> {
        self.tenant.as_deref()
    }
}
impl Clone for OAuthAppCredentials {
    fn clone(&self) -> Self {
        Self {
            client_id: self.client_id.clone(),
            secret: Zeroizing::new(self.secret.to_string()),
            tenant: self.tenant.clone(),
        }
    }
}
impl PartialEq for OAuthAppCredentials {
    fn eq(&self, other: &Self) -> bool {
        self.client_id == other.client_id
            && self.secret.as_bytes() == other.secret.as_bytes()
            && self.tenant == other.tenant
    }
}
impl Eq for OAuthAppCredentials {}
impl fmt::Debug for OAuthAppCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuthAppCredentials")
            .field("client_id", &hint(&self.client_id))
            .field("secret", &"[REDACTED]")
            .field("tenant", &self.tenant)
            .finish()
    }
}

#[derive(Clone)]
pub(crate) struct ResolvedOAuthApp {
    provider: OAuthProvider,
    credentials: OAuthAppCredentials,
    source: OAuthAppSource,
    revision: u64,
}
impl ResolvedOAuthApp {
    pub(crate) fn provider(&self) -> OAuthProvider {
        self.provider
    }
    pub(crate) fn credentials(&self) -> &OAuthAppCredentials {
        &self.credentials
    }
    pub(crate) fn source(&self) -> OAuthAppSource {
        self.source
    }
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }
}
impl fmt::Debug for ResolvedOAuthApp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolvedOAuthApp")
            .field("provider", &self.provider)
            .field("credentials", &self.credentials)
            .field("source", &self.source)
            .field("revision", &self.revision)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OAuthAppStatus {
    pub(crate) provider: OAuthProvider,
    pub(crate) configured: bool,
    pub(crate) source: Option<OAuthAppSource>,
    pub(crate) client_id_hint: Option<String>,
    pub(crate) redirect_uris: Vec<String>,
    pub(crate) tenant: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OAuthAppError {
    InvalidClientId,
    InvalidClientSecret,
    InvalidTenant,
    VaultUnavailable,
    InvalidVaultPayload,
    UnsupportedVaultPayloadVersion,
    VaultStateUncertain,
    OperationCancelled,
}
impl fmt::Display for OAuthAppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidClientId => "invalid OAuth client identifier",
            Self::InvalidClientSecret => "invalid OAuth client secret",
            Self::InvalidTenant => "invalid Microsoft tenant",
            Self::VaultUnavailable => "OAuth application vault unavailable",
            Self::InvalidVaultPayload => "OAuth application vault payload is invalid",
            Self::UnsupportedVaultPayloadVersion => {
                "OAuth application vault payload version is unsupported"
            }
            Self::VaultStateUncertain => "OAuth application vault state could not be confirmed",
            Self::OperationCancelled => "OAuth application vault operation did not complete",
        })
    }
}
impl std::error::Error for OAuthAppError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OAuthVaultError {
    Unavailable,
}
pub(crate) trait OAuthVaultBackend: Send + Sync {
    /// `Ok(None)` means the keyring returned `NoEntry`; every other failure is an error.
    fn load(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<Zeroizing<String>>, OAuthVaultError>;
    fn put(&self, service: &str, account: &str, payload: &str) -> Result<(), OAuthVaultError>;
    fn delete(&self, service: &str, account: &str) -> Result<(), OAuthVaultError>;
}
pub(crate) trait OAuthEnvironment: Send + Sync {
    fn get(&self, name: &str) -> Option<String>;
}
struct ProcessEnvironment;
impl OAuthEnvironment for ProcessEnvironment {
    fn get(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}
pub(crate) struct OsOAuthVault;
impl OAuthVaultBackend for OsOAuthVault {
    fn load(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<Zeroizing<String>>, OAuthVaultError> {
        let entry =
            keyring::Entry::new(service, account).map_err(|_| OAuthVaultError::Unavailable)?;
        match entry.get_password() {
            Ok(v) => Ok(Some(Zeroizing::new(v))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(OAuthVaultError::Unavailable),
        }
    }
    fn put(&self, service: &str, account: &str, payload: &str) -> Result<(), OAuthVaultError> {
        keyring::Entry::new(service, account)
            .map_err(|_| OAuthVaultError::Unavailable)?
            .set_password(payload)
            .map_err(|_| OAuthVaultError::Unavailable)
    }
    fn delete(&self, service: &str, account: &str) -> Result<(), OAuthVaultError> {
        let entry =
            keyring::Entry::new(service, account).map_err(|_| OAuthVaultError::Unavailable)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(OAuthVaultError::Unavailable),
        }
    }
}

#[derive(Default)]
struct Lifecycle {
    revision: u64,
}
struct Inner {
    vault: Arc<dyn OAuthVaultBackend>,
    environment: Arc<dyn OAuthEnvironment>,
    google: Arc<Mutex<Lifecycle>>,
    microsoft: Arc<Mutex<Lifecycle>>,
}
#[derive(Clone)]
pub(crate) struct OAuthAppService {
    inner: Arc<Inner>,
}
impl OAuthAppService {
    pub(crate) fn new() -> Self {
        Self::with_backends(Arc::new(OsOAuthVault), Arc::new(ProcessEnvironment))
    }
    pub(crate) fn with_backends(
        vault: Arc<dyn OAuthVaultBackend>,
        environment: Arc<dyn OAuthEnvironment>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                vault,
                environment,
                google: Arc::new(Mutex::new(Lifecycle::default())),
                microsoft: Arc::new(Mutex::new(Lifecycle::default())),
            }),
        }
    }
    fn lock(&self, p: OAuthProvider) -> Arc<Mutex<Lifecycle>> {
        match p {
            OAuthProvider::Google => self.inner.google.clone(),
            OAuthProvider::Microsoft => self.inner.microsoft.clone(),
        }
    }
    pub(crate) async fn locked_config(
        &self,
        provider: OAuthProvider,
    ) -> Result<OAuthAppLease, OAuthAppError> {
        let mut guard = self.lock(provider).lock_owned().await;
        let resolved = resolve(&self.inner, provider, &mut guard).await?;
        let revision = guard.revision;
        Ok(OAuthAppLease {
            resolved,
            revision,
            _guard: guard,
        })
    }
    pub(crate) async fn with_locked_config<T, F, Fut>(
        &self,
        provider: OAuthProvider,
        op: F,
    ) -> Result<T, OAuthAppError>
    where
        F: FnOnce(Option<ResolvedOAuthApp>, u64) -> Fut,
        Fut: Future<Output = Result<T, OAuthAppError>>,
    {
        let lease = self.locked_config(provider).await?;
        op(lease.resolved.clone(), lease.revision).await
    }
    pub(crate) async fn status(
        &self,
        provider: OAuthProvider,
    ) -> Result<OAuthAppStatus, OAuthAppError> {
        let lease = self.locked_config(provider).await?;
        Ok(make_status(provider, lease.resolved.as_ref()))
    }
    pub(crate) async fn statuses(&self) -> Result<Vec<OAuthAppStatus>, OAuthAppError> {
        let mut out = Vec::new();
        for p in OAuthProvider::ALL {
            out.push(self.status(p).await?);
        }
        Ok(out)
    }
    pub(crate) async fn put(
        &self,
        provider: OAuthProvider,
        credentials: OAuthAppCredentials,
    ) -> Result<u64, OAuthAppError> {
        let (tx, rx) = oneshot::channel();
        let this = self.clone();
        tokio::spawn(async move {
            let _ = tx.send(this.put_owned(provider, credentials).await);
        });
        rx.await.map_err(|_| OAuthAppError::OperationCancelled)?
    }
    async fn put_owned(
        &self,
        provider: OAuthProvider,
        credentials: OAuthAppCredentials,
    ) -> Result<u64, OAuthAppError> {
        let mut guard = self.lock(provider).lock_owned().await;
        let vault = self.inner.vault.clone();
        let known = guard.revision;
        let revision = tokio::task::spawn_blocking(move || {
            put_verified(vault.as_ref(), provider, credentials, known)
        })
        .await
        .map_err(|_| OAuthAppError::OperationCancelled)??;
        guard.revision = revision;
        Ok(revision)
    }
    pub(crate) async fn delete(&self, provider: OAuthProvider) -> Result<u64, OAuthAppError> {
        let (tx, rx) = oneshot::channel();
        let this = self.clone();
        tokio::spawn(async move {
            let _ = tx.send(this.delete_owned(provider).await);
        });
        rx.await.map_err(|_| OAuthAppError::OperationCancelled)?
    }
    async fn delete_owned(&self, provider: OAuthProvider) -> Result<u64, OAuthAppError> {
        let mut guard = self.lock(provider).lock_owned().await;
        let vault = self.inner.vault.clone();
        let known = guard.revision;
        let revision =
            tokio::task::spawn_blocking(move || delete_verified(vault.as_ref(), provider, known))
                .await
                .map_err(|_| OAuthAppError::OperationCancelled)??;
        guard.revision = revision;
        Ok(revision)
    }
}
impl Default for OAuthAppService {
    fn default() -> Self {
        Self::new()
    }
}
pub(crate) struct OAuthAppLease {
    resolved: Option<ResolvedOAuthApp>,
    revision: u64,
    _guard: OwnedMutexGuard<Lifecycle>,
}
impl OAuthAppLease {
    pub(crate) fn config(&self) -> Option<&ResolvedOAuthApp> {
        self.resolved.as_ref()
    }
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }
    pub(crate) fn revision_is_current(&self, r: u64) -> bool {
        self.revision == r
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stored {
    version: u64,
    revision: u64,
    client_id: String,
    client_secret: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tenant: Option<String>,
}
async fn resolve(
    inner: &Inner,
    p: OAuthProvider,
    state: &mut Lifecycle,
) -> Result<Option<ResolvedOAuthApp>, OAuthAppError> {
    let vault = inner.vault.clone();
    let payload = tokio::task::spawn_blocking(move || vault.load(SERVICE, p.account()))
        .await
        .map_err(|_| OAuthAppError::VaultUnavailable)?
        .map_err(|_| OAuthAppError::VaultUnavailable)?;
    if let Some(raw) = payload {
        let (c, r) = decode(p, raw.as_str())?;
        state.revision = state.revision.max(r);
        return Ok(Some(ResolvedOAuthApp {
            provider: p,
            credentials: c,
            source: OAuthAppSource::Vault,
            revision: r,
        }));
    }
    let Some(c) = from_env(inner.environment.as_ref(), p)? else {
        return Ok(None);
    };
    Ok(Some(ResolvedOAuthApp {
        provider: p,
        credentials: c,
        source: OAuthAppSource::Environment,
        revision: state.revision,
    }))
}
fn from_env(
    e: &dyn OAuthEnvironment,
    p: OAuthProvider,
) -> Result<Option<OAuthAppCredentials>, OAuthAppError> {
    let (i, s, t) = p.env();
    let (Some(i), Some(s)) = (e.get(i), e.get(s)) else {
        return Ok(None);
    };
    OAuthAppCredentials::new(p, i, s, t.and_then(|n| e.get(n))).map(Some)
}
fn decode(p: OAuthProvider, raw: &str) -> Result<(OAuthAppCredentials, u64), OAuthAppError> {
    let mut v: Stored =
        serde_json::from_str(raw).map_err(|_| OAuthAppError::InvalidVaultPayload)?;
    if v.version != VERSION {
        v.client_secret.zeroize();
        return Err(OAuthAppError::UnsupportedVaultPayloadVersion);
    }
    if v.revision == 0 {
        v.client_secret.zeroize();
        return Err(OAuthAppError::InvalidVaultPayload);
    }
    let r = v.revision;
    let s = std::mem::take(&mut v.client_secret);
    OAuthAppCredentials::new(p, v.client_id, s, v.tenant)
        .map(|c| (c, r))
        .map_err(|_| OAuthAppError::InvalidVaultPayload)
}
fn encode(c: &OAuthAppCredentials, r: u64) -> Result<Zeroizing<String>, OAuthAppError> {
    serde_json::to_string(&Stored {
        version: VERSION,
        revision: r,
        client_id: c.client_id.clone(),
        client_secret: c.secret.to_string(),
        tenant: c.tenant.clone(),
    })
    .map(Zeroizing::new)
    .map_err(|_| OAuthAppError::InvalidVaultPayload)
}
fn put_verified(
    v: &dyn OAuthVaultBackend,
    p: OAuthProvider,
    c: OAuthAppCredentials,
    known: u64,
) -> Result<u64, OAuthAppError> {
    let old = v
        .load(SERVICE, p.account())
        .map_err(|_| OAuthAppError::VaultUnavailable)?;
    let old_rev = old
        .as_ref()
        .map(|x| decode(p, x.as_str()).map(|x| x.1))
        .transpose()?
        .unwrap_or(0);
    let rev = known
        .max(old_rev)
        .checked_add(1)
        .ok_or(OAuthAppError::VaultStateUncertain)?;
    let payload = encode(&c, rev)?;
    let write = v.put(SERVICE, p.account(), payload.as_str());
    let observed = v.load(SERVICE, p.account());
    if write.is_ok()
        && observed
            .as_ref()
            .ok()
            .and_then(|x| x.as_ref())
            .is_some_and(|x| x.as_str() == payload.as_str())
    {
        return Ok(rev);
    }
    restore(v, p.account(), old.as_ref())?;
    if write.is_err() {
        Err(OAuthAppError::VaultUnavailable)
    } else {
        Err(OAuthAppError::VaultStateUncertain)
    }
}
fn restore(
    v: &dyn OAuthVaultBackend,
    a: &str,
    old: Option<&Zeroizing<String>>,
) -> Result<(), OAuthAppError> {
    let r = match old {
        Some(x) => v.put(SERVICE, a, x.as_str()),
        None => v.delete(SERVICE, a),
    };
    if r.is_err() {
        return Err(OAuthAppError::VaultStateUncertain);
    }
    let seen = v
        .load(SERVICE, a)
        .map_err(|_| OAuthAppError::VaultStateUncertain)?;
    if seen.as_ref().map(|x| x.as_str()) == old.map(|x| x.as_str()) {
        Ok(())
    } else {
        Err(OAuthAppError::VaultStateUncertain)
    }
}
fn delete_verified(
    v: &dyn OAuthVaultBackend,
    p: OAuthProvider,
    known: u64,
) -> Result<u64, OAuthAppError> {
    let old = v
        .load(SERVICE, p.account())
        .map_err(|_| OAuthAppError::VaultUnavailable)?;
    let old_rev = old
        .as_ref()
        .map(|x| decode(p, x.as_str()).map(|x| x.1))
        .transpose()?
        .unwrap_or(0);
    let rev = known
        .max(old_rev)
        .checked_add(1)
        .ok_or(OAuthAppError::VaultStateUncertain)?;
    let deletion = v.delete(SERVICE, p.account());
    let seen = v
        .load(SERVICE, p.account())
        .map_err(|_| OAuthAppError::VaultStateUncertain)?;
    if seen.is_none() {
        Ok(rev)
    } else if deletion.is_err() {
        Err(OAuthAppError::VaultUnavailable)
    } else {
        Err(OAuthAppError::VaultStateUncertain)
    }
}
fn make_status(p: OAuthProvider, c: Option<&ResolvedOAuthApp>) -> OAuthAppStatus {
    OAuthAppStatus {
        provider: p,
        configured: c.is_some(),
        source: c.map(|x| x.source),
        client_id_hint: c.map(|x| hint(x.credentials.client_id())),
        redirect_uris: p.redirect_uris().iter().map(|x| x.to_string()).collect(),
        tenant: c.and_then(|x| x.credentials.tenant.clone()),
    }
}
fn hint(id: &str) -> String {
    let s: String = id
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("...{s}")
}
fn valid_text(v: String, max: usize) -> Option<String> {
    let s = v.trim();
    (!s.is_empty() && s.len() <= max && !s.chars().any(char::is_control)).then(|| s.to_string())
}
fn tenant_for(p: OAuthProvider, t: Option<String>) -> Result<Option<String>, OAuthAppError> {
    if p == OAuthProvider::Google {
        return match t {
            Some(x) if !x.trim().is_empty() => Err(OAuthAppError::InvalidTenant),
            _ => Ok(None),
        };
    }
    let t = valid_text(t.unwrap_or_else(|| "common".into()), 255)
        .ok_or(OAuthAppError::InvalidTenant)?;
    if t.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.'))
    {
        Ok(Some(t))
    } else {
        Err(OAuthAppError::InvalidTenant)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::{HashMap, VecDeque},
        sync::{mpsc, Condvar, Mutex as StdMutex, RwLock},
        time::Duration,
    };
    const SECRET: &str = "oauth-client-secret-sentinel";
    #[derive(Default)]
    struct Env(HashMap<String, String>);
    impl OAuthEnvironment for Env {
        fn get(&self, n: &str) -> Option<String> {
            self.0.get(n).cloned()
        }
    }
    #[derive(Clone, Copy)]
    enum PB {
        Normal,
        FailAfter,
        SucceedWithout,
    }
    #[derive(Clone, Copy)]
    enum DB {
        Normal,
        SucceedWithout,
    }
    #[derive(Default)]
    struct Vault {
        values: RwLock<HashMap<String, Zeroizing<String>>>,
        load_errors: StdMutex<VecDeque<OAuthVaultError>>,
        puts: StdMutex<VecDeque<PB>>,
        deletes: StdMutex<VecDeque<DB>>,
    }
    impl OAuthVaultBackend for Vault {
        fn load(&self, _: &str, a: &str) -> Result<Option<Zeroizing<String>>, OAuthVaultError> {
            if let Some(e) = self.load_errors.lock().unwrap().pop_front() {
                return Err(e);
            }
            Ok(self.values.read().unwrap().get(a).cloned())
        }
        fn put(&self, _: &str, a: &str, p: &str) -> Result<(), OAuthVaultError> {
            match self.puts.lock().unwrap().pop_front().unwrap_or(PB::Normal) {
                PB::Normal => {
                    self.values
                        .write()
                        .unwrap()
                        .insert(a.into(), Zeroizing::new(p.into()));
                    Ok(())
                }
                PB::FailAfter => {
                    self.values
                        .write()
                        .unwrap()
                        .insert(a.into(), Zeroizing::new(p.into()));
                    Err(OAuthVaultError::Unavailable)
                }
                PB::SucceedWithout => Ok(()),
            }
        }
        fn delete(&self, _: &str, a: &str) -> Result<(), OAuthVaultError> {
            match self
                .deletes
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(DB::Normal)
            {
                DB::Normal => {
                    self.values.write().unwrap().remove(a);
                }
                DB::SucceedWithout => {}
            }
            Ok(())
        }
    }
    fn creds(p: OAuthProvider, id: &str) -> OAuthAppCredentials {
        OAuthAppCredentials::new(p, id, SECRET, None).unwrap()
    }
    fn svc(v: Arc<Vault>, env: &[(&str, &str)]) -> OAuthAppService {
        OAuthAppService::with_backends(
            v,
            Arc::new(Env(env
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect())),
        )
    }
    #[test]
    fn validation_defaults_and_redirects() {
        assert_eq!(
            OAuthProvider::Google.redirect_uris(),
            &[GOOGLE_MAIL, GOOGLE_CALENDAR]
        );
        assert_eq!(OAuthProvider::Microsoft.redirect_uris(), &[MICROSOFT_MAIL]);
        let c =
            OAuthAppCredentials::new(OAuthProvider::Microsoft, " id ", " secret ", None).unwrap();
        assert_eq!(
            (c.client_id(), c.client_secret(), c.tenant()),
            ("id", "secret", Some("common"))
        );
        for x in ["", "  ", "bad\nvalue"] {
            assert_eq!(
                OAuthAppCredentials::new(OAuthProvider::Google, x, "secret", None).unwrap_err(),
                OAuthAppError::InvalidClientId
            );
            assert_eq!(
                OAuthAppCredentials::new(OAuthProvider::Google, "id", x, None).unwrap_err(),
                OAuthAppError::InvalidClientSecret
            )
        }
        assert_eq!(
            OAuthAppCredentials::new(OAuthProvider::Google, "x".repeat(2049), "s", None)
                .unwrap_err(),
            OAuthAppError::InvalidClientId
        );
        assert_eq!(
            OAuthAppCredentials::new(OAuthProvider::Google, "i", "x".repeat(4097), None)
                .unwrap_err(),
            OAuthAppError::InvalidClientSecret
        );
        for x in ["tenant/name", "tenant_name", "tenant value", "bad\ntenant"] {
            assert_eq!(
                OAuthAppCredentials::new(OAuthProvider::Microsoft, "i", "s", Some(x.into()))
                    .unwrap_err(),
                OAuthAppError::InvalidTenant
            )
        }
        assert_eq!(
            OAuthAppCredentials::new(OAuthProvider::Microsoft, "i", "s", Some("x".repeat(256)))
                .unwrap_err(),
            OAuthAppError::InvalidTenant
        )
    }
    #[tokio::test]
    async fn redaction_and_environment_default() {
        let s = svc(
            Arc::new(Vault::default()),
            &[
                ("ANIMA_GOOGLE_CLIENT_ID", "client-12345678"),
                ("ANIMA_GOOGLE_CLIENT_SECRET", SECRET),
            ],
        );
        let st = s.status(OAuthProvider::Google).await.unwrap();
        assert_eq!(
            (st.source, st.client_id_hint.as_deref()),
            (Some(OAuthAppSource::Environment), Some("...5678"))
        );
        assert!(!serde_json::to_string(&st).unwrap().contains(SECRET));
        assert!(!format!(
            "{:?}",
            s.locked_config(OAuthProvider::Google)
                .await
                .unwrap()
                .config()
                .unwrap()
        )
        .contains(SECRET))
    }
    #[tokio::test]
    async fn vault_precedence_and_fail_closed() {
        let v = Arc::new(Vault::default());
        let s = svc(
            v.clone(),
            &[
                ("ANIMA_GOOGLE_CLIENT_ID", "env"),
                ("ANIMA_GOOGLE_CLIENT_SECRET", "env-secret"),
            ],
        );
        s.put(OAuthProvider::Google, creds(OAuthProvider::Google, "vault"))
            .await
            .unwrap();
        assert_eq!(
            s.locked_config(OAuthProvider::Google)
                .await
                .unwrap()
                .config()
                .unwrap()
                .credentials()
                .client_id(),
            "vault"
        );
        v.load_errors
            .lock()
            .unwrap()
            .push_back(OAuthVaultError::Unavailable);
        assert_eq!(
            s.status(OAuthProvider::Google).await.unwrap_err(),
            OAuthAppError::VaultUnavailable
        );
        v.values.write().unwrap().insert(
            OAuthProvider::Google.account().into(),
            Zeroizing::new("bad".into()),
        );
        assert_eq!(
            s.status(OAuthProvider::Google).await.unwrap_err(),
            OAuthAppError::InvalidVaultPayload
        )
    }
    #[tokio::test]
    async fn version_revision_compensation_and_delete_verification() {
        let v = Arc::new(Vault::default());
        let s = svc(v.clone(), &[]);
        assert_eq!(
            s.put(OAuthProvider::Google, creds(OAuthProvider::Google, "old"))
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            s.put(
                OAuthProvider::Google,
                creds(OAuthProvider::Google, "second")
            )
            .await
            .unwrap(),
            2
        );
        let raw = v
            .values
            .read()
            .unwrap()
            .get(OAuthProvider::Google.account())
            .unwrap()
            .clone();
        let stored: Stored = serde_json::from_str(raw.as_str()).unwrap();
        assert_eq!((stored.version, stored.revision), (VERSION, 2));
        v.puts.lock().unwrap().extend([PB::FailAfter, PB::Normal]);
        assert_eq!(
            s.put(OAuthProvider::Google, creds(OAuthProvider::Google, "new"))
                .await
                .unwrap_err(),
            OAuthAppError::VaultUnavailable
        );
        let lease = s.locked_config(OAuthProvider::Google).await.unwrap();
        assert_eq!(
            (
                lease.revision(),
                lease.config().unwrap().credentials().client_id()
            ),
            (2, "second")
        );
        drop(lease);
        v.puts
            .lock()
            .unwrap()
            .extend([PB::SucceedWithout, PB::Normal]);
        assert_eq!(
            s.put(OAuthProvider::Google, creds(OAuthProvider::Google, "new"))
                .await
                .unwrap_err(),
            OAuthAppError::VaultStateUncertain
        );
        v.deletes.lock().unwrap().push_back(DB::SucceedWithout);
        assert_eq!(
            s.delete(OAuthProvider::Google).await.unwrap_err(),
            OAuthAppError::VaultStateUncertain
        );
        assert_eq!(
            s.locked_config(OAuthProvider::Google)
                .await
                .unwrap()
                .revision(),
            2
        );
        assert_eq!(s.delete(OAuthProvider::Google).await.unwrap(), 3);
        assert_eq!(
            s.put(OAuthProvider::Google, creds(OAuthProvider::Google, "last"))
                .await
                .unwrap(),
            4
        )
    }
    struct Blocking {
        value: RwLock<Option<Zeroizing<String>>>,
        started: StdMutex<Option<mpsc::Sender<()>>>,
        finished: StdMutex<Option<mpsc::Sender<()>>>,
        release: Arc<(StdMutex<bool>, Condvar)>,
    }
    impl OAuthVaultBackend for Blocking {
        fn load(&self, _: &str, _: &str) -> Result<Option<Zeroizing<String>>, OAuthVaultError> {
            Ok(self.value.read().unwrap().clone())
        }
        fn put(&self, _: &str, _: &str, p: &str) -> Result<(), OAuthVaultError> {
            if let Some(tx) = self.started.lock().unwrap().take() {
                tx.send(()).unwrap();
                let (l, c) = &*self.release;
                let mut go = l.lock().unwrap();
                while !*go {
                    go = c.wait(go).unwrap()
                }
            }
            *self.value.write().unwrap() = Some(Zeroizing::new(p.into()));
            if let Some(tx) = self.finished.lock().unwrap().take() {
                tx.send(()).unwrap()
            }
            Ok(())
        }
        fn delete(&self, _: &str, _: &str) -> Result<(), OAuthVaultError> {
            *self.value.write().unwrap() = None;
            Ok(())
        }
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_safety() {
        let (stx, srx) = mpsc::channel();
        let (ftx, frx) = mpsc::channel();
        let release = Arc::new((StdMutex::new(false), Condvar::new()));
        let v = Arc::new(Blocking {
            value: RwLock::new(None),
            started: StdMutex::new(Some(stx)),
            finished: StdMutex::new(Some(ftx)),
            release: release.clone(),
        });
        let s = OAuthAppService::with_backends(v, Arc::new(Env::default()));
        let task = tokio::spawn({
            let s = s.clone();
            async move {
                s.put(OAuthProvider::Google, creds(OAuthProvider::Google, "saved"))
                    .await
            }
        });
        srx.recv_timeout(Duration::from_secs(2)).unwrap();
        task.abort();
        let status = tokio::spawn({
            let s = s.clone();
            async move { s.status(OAuthProvider::Google).await }
        });
        tokio::task::yield_now().await;
        assert!(!status.is_finished());
        let (l, c) = &*release;
        *l.lock().unwrap() = true;
        c.notify_all();
        frx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(status.await.unwrap().unwrap().configured)
    }

    struct BlockingDelete {
        value: RwLock<Option<Zeroizing<String>>>,
        started: StdMutex<Option<mpsc::Sender<()>>>,
        finished: StdMutex<Option<mpsc::Sender<()>>>,
        release: Arc<(StdMutex<bool>, Condvar)>,
    }
    impl OAuthVaultBackend for BlockingDelete {
        fn load(&self, _: &str, _: &str) -> Result<Option<Zeroizing<String>>, OAuthVaultError> {
            Ok(self.value.read().unwrap().clone())
        }
        fn put(&self, _: &str, _: &str, payload: &str) -> Result<(), OAuthVaultError> {
            *self.value.write().unwrap() = Some(Zeroizing::new(payload.into()));
            Ok(())
        }
        fn delete(&self, _: &str, _: &str) -> Result<(), OAuthVaultError> {
            if let Some(tx) = self.started.lock().unwrap().take() {
                tx.send(()).unwrap();
                let (lock, signal) = &*self.release;
                let mut released = lock.lock().unwrap();
                while !*released {
                    released = signal.wait(released).unwrap();
                }
            }
            *self.value.write().unwrap() = None;
            if let Some(tx) = self.finished.lock().unwrap().take() {
                tx.send(()).unwrap();
            }
            Ok(())
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn delete_survives_caller_cancellation() {
        let initial = encode(&creds(OAuthProvider::Google, "saved"), 7).unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let release = Arc::new((StdMutex::new(false), Condvar::new()));
        let vault = Arc::new(BlockingDelete {
            value: RwLock::new(Some(initial)),
            started: StdMutex::new(Some(started_tx)),
            finished: StdMutex::new(Some(finished_tx)),
            release: release.clone(),
        });
        let service = OAuthAppService::with_backends(vault, Arc::new(Env::default()));
        let task = tokio::spawn({
            let service = service.clone();
            async move { service.delete(OAuthProvider::Google).await }
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        task.abort();
        let status = tokio::spawn({
            let service = service.clone();
            async move { service.status(OAuthProvider::Google).await }
        });
        tokio::task::yield_now().await;
        assert!(!status.is_finished());
        let (lock, signal) = &*release;
        *lock.lock().unwrap() = true;
        signal.notify_all();
        finished_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let status = status.await.unwrap().unwrap();
        assert!(!status.configured);
        assert_eq!(status.source, None);
        let lease = service.locked_config(OAuthProvider::Google).await.unwrap();
        assert_eq!(lease.revision(), 8);
    }
}
