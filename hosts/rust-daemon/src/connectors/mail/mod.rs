//! Daemon-owned mail: only local immutable drafts; sending requires the owner HTTP boundary.
pub(crate) mod client;
#[cfg(test)]
mod tests;
use super::gcalendar::{
    now_ms,
    store::{GoogleCredentialStore, GoogleOAuthTokens},
};
use super::oauth_apps::{OAuthAppService, OAuthProvider, ResolvedOAuthApp};
use crate::{agent_runs::AgentRunCoordinator, app::SharedDaemonState, state::DaemonState};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use client::MailTransport;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Weak},
    time::Duration,
};
use tokio::sync::{Mutex, RwLock};
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Provider {
    Gmail,
    Outlook,
}
impl Provider {
    pub(crate) fn parse(v: &str) -> Result<Self, MailError> {
        match v {
            "gmail" => Ok(Self::Gmail),
            "outlook" => Ok(Self::Outlook),
            _ => Err(MailError::Invalid),
        }
    }
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Gmail => "gmail",
            Self::Outlook => "outlook",
        }
    }
    fn oauth_provider(self) -> OAuthProvider {
        match self {
            Self::Gmail => OAuthProvider::Google,
            Self::Outlook => OAuthProvider::Microsoft,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MailConnector {
    pub(crate) id: String,
    pub(crate) agent_id: String,
    #[serde(rename = "type")]
    pub(crate) provider: Provider,
    pub(crate) account_label: Option<String>,
    pub(crate) status: String,
    pub(crate) created_at_ms: u64,
    pub(crate) updated_at_ms: u64,
    pub(crate) last_synced_at_ms: Option<u64>,
    pub(crate) error: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MailMessage {
    pub(crate) id: String,
    pub(crate) from: String,
    pub(crate) subject: String,
    pub(crate) preview: String,
    pub(crate) received_at: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub(crate) struct MailRecord {
    pub(crate) connector: MailConnector,
    pub(crate) messages: Vec<MailMessage>,
    pub(crate) deleted: bool,
    #[serde(default)]
    pub(crate) auth_generation: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum DraftState {
    Pending,
    Sending,
    Sent,
    Rejected,
    Failed,
    Unknown,
}
impl DraftState {
    fn unresolved(self) -> bool {
        matches!(self, Self::Pending | Self::Sending | Self::Unknown)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MailDraft {
    pub(crate) id: String,
    pub(crate) connector_id: String,
    pub(crate) to: Vec<String>,
    pub(crate) subject: String,
    pub(crate) body: String,
    pub(crate) state: DraftState,
    pub(crate) error: Option<String>,
    pub(crate) created_at_ms: u64,
    pub(crate) resolved_at_ms: Option<u64>,
}
#[derive(Clone, Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DraftInput {
    pub(crate) to: Vec<String>,
    pub(crate) subject: String,
    pub(crate) body: String,
}
impl DraftInput {
    fn valid(&self) -> bool {
        !self.to.is_empty()
            && self.to.len() <= 20
            && self.to.iter().all(|s| {
                s.len() <= 254
                    && s.split('@').count() == 2
                    && !s.starts_with('@')
                    && !s.ends_with('@')
                    && s.contains('.')
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"@.!#$%&'*+-/=?^_`{|}~".contains(&b))
            })
            && !self.subject.is_empty()
            && self.subject.len() <= 998
            && !self.subject.contains(['\r', '\n', '\0'])
            && self.body.len() <= 100_000
            && !self.body.contains('\0')
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MailError {
    Invalid,
    NotFound,
    Conflict,
    Unconfigured,
    Unauthorized,
    Persistence,
    Credential,
    Upstream,
    Unknown,
}
impl std::fmt::Display for MailError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Invalid => "Invalid mail request or expired authorization",
            Self::NotFound => "Mail connector or draft was not found",
            Self::Conflict => "Mail operation is no longer available",
            Self::Unconfigured => "Mail OAuth is not configured",
            Self::Unauthorized => "Mail access must be reauthorized",
            Self::Persistence => "Mail persistence is unavailable",
            Self::Credential => "Credential vault is unavailable",
            Self::Upstream => "Mail provider is unavailable",
            Self::Unknown => {
                "Send outcome is unknown; verify Sent mail before creating another draft"
            }
        })
    }
}
#[derive(Clone)]
pub(crate) struct OAuthConfig {
    pub(crate) provider: Provider,
    pub(crate) client_id: String,
    pub(crate) secret: Zeroizing<String>,
    pub(crate) redirect: String,
    pub(crate) tenant: String,
}
impl OAuthConfig {
    fn from_resolved(provider: Provider, resolved: &ResolvedOAuthApp) -> Self {
        Self {
            provider,
            client_id: resolved.credentials().client_id().to_string(),
            secret: resolved.credentials().client_secret().to_string().into(),
            redirect: format!(
                "http://127.0.0.1:8080/api/connectors/mail/{}/callback",
                provider.name()
            ),
            tenant: resolved
                .credentials()
                .tenant()
                .unwrap_or("common")
                .to_string(),
        }
    }
    pub(crate) fn scopes(&self) -> &'static str {
        match self.provider{Provider::Gmail=>"https://www.googleapis.com/auth/gmail.readonly https://www.googleapis.com/auth/gmail.send",Provider::Outlook=>"offline_access https://graph.microsoft.com/User.Read https://graph.microsoft.com/Mail.Read https://graph.microsoft.com/Mail.Send"}
    }
    pub(crate) fn token_url(&self) -> String {
        match self.provider {
            Provider::Gmail => "https://oauth2.googleapis.com/token".into(),
            Provider::Outlook => format!(
                "https://login.microsoftonline.com/{}/oauth2/v2.0/token",
                self.tenant
            ),
        }
    }
    fn consent(&self, nonce: &str, verifier: &str) -> String {
        let base = match self.provider {
            Provider::Gmail => "https://accounts.google.com/o/oauth2/v2/auth".into(),
            Provider::Outlook => format!(
                "https://login.microsoftonline.com/{}/oauth2/v2.0/authorize",
                self.tenant
            ),
        };
        let mut url = reqwest::Url::parse(&base).expect("constant OAuth URL");
        url.query_pairs_mut().extend_pairs([
            ("client_id", self.client_id.as_str()),
            ("redirect_uri", &self.redirect),
            ("response_type", "code"),
            ("scope", self.scopes()),
            ("state", nonce),
            ("code_challenge_method", "S256"),
            (
                "code_challenge",
                &URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())),
            ),
            ("prompt", "consent"),
        ]);
        if self.provider == Provider::Gmail {
            url.query_pairs_mut().append_pair("access_type", "offline");
        }
        url.into()
    }
}
struct Pending {
    id: String,
    agent: String,
    provider: Provider,
    verifier: Zeroizing<String>,
    expires: u64,
    generation: String,
    config_revision: u64,
}
#[derive(Clone)]
pub(crate) struct MailManager {
    state: Weak<RwLock<DaemonState>>,
    transactions: Arc<Mutex<()>>,
    credentials: Arc<dyn GoogleCredentialStore>,
    transport: Arc<dyn MailTransport>,
    pub(crate) oauth_apps: OAuthAppService,
    pending: Arc<Mutex<HashMap<String, Pending>>>,
    locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}
impl MailManager {
    pub(crate) fn new(
        state: &SharedDaemonState,
        runs: AgentRunCoordinator,
        credentials: Arc<dyn GoogleCredentialStore>,
        transport: Arc<dyn MailTransport>,
        oauth_apps: OAuthAppService,
    ) -> Self {
        Self {
            state: Arc::downgrade(state),
            transactions: runs.control_plane_transactions(),
            credentials,
            transport,
            oauth_apps,
            pending: Arc::default(),
            locks: Arc::default(),
        }
    }
    pub(crate) fn production(
        state: &SharedDaemonState,
        runs: AgentRunCoordinator,
        oauth_apps: OAuthAppService,
    ) -> Self {
        Self::new(
            state,
            runs,
            Arc::new(super::gcalendar::store::OsKeyringGoogleCredentialStore::new()),
            Arc::new(client::MailClient::new()),
            oauth_apps,
        )
    }
    pub(crate) async fn configured(&self, p: Provider) -> Result<bool, MailError> {
        self.oauth_apps
            .status(p.oauth_provider())
            .await
            .map(|status| status.configured)
            .map_err(|_| MailError::Credential)
    }
    pub(crate) async fn has_provider_dependency(&self, provider: OAuthProvider) -> bool {
        let Some(state) = self.state.upgrade() else {
            return false;
        };
        let in_use = state.read().await.mail_records.values().any(|record| {
            !record.deleted && record.connector.provider.oauth_provider() == provider
        });
        in_use
    }
    fn shared(&self) -> Result<SharedDaemonState, MailError> {
        self.state.upgrade().ok_or(MailError::Persistence)
    }
    async fn lock(&self, agent: &str, p: Provider) -> Arc<Mutex<()>> {
        self.locks
            .lock()
            .await
            .entry(format!("{agent}:{}", p.name()))
            .or_default()
            .clone()
    }
    async fn edit<T>(
        &self,
        f: impl FnOnce(&mut DaemonState) -> Result<T, MailError>,
    ) -> Result<T, MailError> {
        let shared = self.shared()?;
        let _tx = self.transactions.lock().await;
        let mut s = shared.write().await;
        let old_records = s.mail_records.clone();
        let old_drafts = s.mail_drafts.clone();
        let old_tools = s
            .mail_records
            .values()
            .filter_map(|r| s.get_agent(&r.connector.agent_id))
            .map(|a| (a.state.id, a.state.config.tools.unwrap_or_default()))
            .collect::<Vec<_>>();
        let result = match f(&mut s) {
            Ok(result) => result,
            Err(error) => {
                s.mail_records = old_records;
                s.mail_drafts = old_drafts;
                for (id, tools) in old_tools {
                    let _ = s.update_agent(
                        &id,
                        anima_core::AgentConfigUpdate {
                            tools: Some(tools),
                            ..Default::default()
                        },
                    );
                }
                return Err(error);
            }
        };
        // Keep all actionable/uncertain drafts and a bounded recent terminal
        // history per connector; snapshot cost must not grow with years of mail.
        let mut terminal = s
            .mail_drafts
            .values()
            .filter(|d| !d.state.unresolved())
            .map(|d| (d.id.clone(), d.connector_id.clone(), d.created_at_ms))
            .collect::<Vec<_>>();
        terminal.sort_by_key(|(_, _, created)| std::cmp::Reverse(*created));
        let mut counts = HashMap::<String, usize>::new();
        for (id, connector, _) in terminal {
            let count = counts.entry(connector).or_default();
            *count += 1;
            if *count > 100 {
                s.mail_drafts.remove(&id);
            }
        }
        let persist = s.control_plane_persist_request();
        drop(s);
        if persist.save().await.is_err() {
            let mut s = shared.write().await;
            s.mail_records = old_records;
            s.mail_drafts = old_drafts;
            for (id, tools) in old_tools {
                let _ = s.update_agent(
                    &id,
                    anima_core::AgentConfigUpdate {
                        tools: Some(tools),
                        ..Default::default()
                    },
                );
            }
            return Err(MailError::Persistence);
        }
        Ok(result)
    }
    fn record<'a>(
        s: &'a DaemonState,
        agent: &str,
        id: &str,
        p: Provider,
    ) -> Result<&'a MailRecord, MailError> {
        if s.get_agent(agent).is_none() {
            return Err(MailError::NotFound);
        }
        s.mail_records
            .get(id)
            .filter(|r| !r.deleted && r.connector.agent_id == agent && r.connector.provider == p)
            .ok_or(MailError::NotFound)
    }
    pub(crate) async fn connector_for_agent(
        &self,
        agent: &str,
        p: Provider,
    ) -> Result<Option<MailConnector>, MailError> {
        let shared = self.shared()?;
        let s = shared.read().await;
        if s.get_agent(agent).is_none() {
            return Err(MailError::NotFound);
        }
        Ok(s.mail_records
            .values()
            .find(|r| !r.deleted && r.connector.agent_id == agent && r.connector.provider == p)
            .map(|r| r.connector.clone()))
    }
    pub(crate) async fn begin_connect(
        &self,
        agent: &str,
        p: Provider,
    ) -> Result<(MailConnector, String), MailError> {
        let oauth_lease = self
            .oauth_apps
            .locked_config(p.oauth_provider())
            .await
            .map_err(|_| MailError::Credential)?;
        let config = oauth_lease
            .config()
            .map(|resolved| OAuthConfig::from_resolved(p, resolved))
            .ok_or(MailError::Unconfigured)?;
        let config_revision = oauth_lease.revision();
        let lock = self.lock(agent, p).await;
        let _g = lock.lock().await;
        let old = self.connector_for_agent(agent, p).await?;
        let now = now_ms();
        let c = MailConnector {
            id: old
                .as_ref()
                .map(|c| c.id.clone())
                .unwrap_or_else(|| format!("mail-{}", uuid::Uuid::new_v4())),
            agent_id: agent.into(),
            provider: p,
            account_label: old.as_ref().and_then(|c| c.account_label.clone()),
            status: "pairing".into(),
            created_at_ms: old.map(|c| c.created_at_ms).unwrap_or(now),
            updated_at_ms: now,
            last_synced_at_ms: None,
            error: None,
        };
        let generation = uuid::Uuid::new_v4().to_string();
        self.edit(|s| {
            s.mail_records.insert(
                c.id.clone(),
                MailRecord {
                    connector: c.clone(),
                    messages: vec![],
                    deleted: false,
                    auth_generation: generation.clone(),
                },
            );
            Ok(())
        })
        .await?;
        let nonce = uuid::Uuid::new_v4().to_string();
        let verifier = Zeroizing::new(format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ));
        let url = config.consent(&nonce, &verifier);
        let mut pending = self.pending.lock().await;
        pending.retain(|_, v| v.expires > now && !(v.agent == agent && v.provider == p));
        pending.insert(
            nonce,
            Pending {
                id: c.id.clone(),
                agent: agent.into(),
                provider: p,
                verifier,
                expires: now + 600_000,
                generation,
                config_revision,
            },
        );
        Ok((c, url))
    }
    pub(crate) async fn complete_connect(
        &self,
        p: Provider,
        nonce: &str,
        code: &str,
    ) -> Result<MailConnector, MailError> {
        let oauth_lease = self
            .oauth_apps
            .locked_config(p.oauth_provider())
            .await
            .map_err(|_| MailError::Credential)?;
        let config = oauth_lease
            .config()
            .map(|resolved| OAuthConfig::from_resolved(p, resolved))
            .ok_or(MailError::Unconfigured)?;
        let pending = {
            let mut pending = self.pending.lock().await;
            if !pending.get(nonce).is_some_and(|v| {
                v.provider == p && oauth_lease.revision_is_current(v.config_revision)
            }) {
                return Err(MailError::Invalid);
            }
            pending.remove(nonce).ok_or(MailError::Invalid)?
        };
        let lock = self.lock(&pending.agent, p).await;
        let _g = lock.lock().await;
        if pending.expires <= now_ms() || code.is_empty() {
            return Err(MailError::Invalid);
        }
        {
            let state = self.shared()?;
            let s = state.read().await;
            let r = Self::record(&s, &pending.agent, &pending.id, p)?;
            if r.connector.status != "pairing" || r.auth_generation != pending.generation {
                return Err(MailError::Conflict);
            }
        }
        let tokens = self
            .transport
            .exchange(&config, code, &pending.verifier)
            .await?;
        let label = self.transport.account(p, tokens.access_token()).await?;
        let previous = self
            .credentials
            .load(&pending.id)
            .await
            .map_err(|_| MailError::Credential)?;
        self.credentials
            .put(&pending.id, tokens)
            .await
            .map_err(|_| MailError::Credential)?;
        let result = self
            .edit(|s| {
                Self::record(s, &pending.agent, &pending.id, p)?;
                let r = s
                    .mail_records
                    .get_mut(&pending.id)
                    .ok_or(MailError::NotFound)?;
                if r.auth_generation != pending.generation {
                    return Err(MailError::Conflict);
                }
                // A draft approved for a different account must never become sendable silently.
                if r.connector
                    .account_label
                    .as_ref()
                    .is_some_and(|old| !old.eq_ignore_ascii_case(&label))
                {
                    for d in s
                        .mail_drafts
                        .values_mut()
                        .filter(|d| d.connector_id == pending.id && d.state == DraftState::Pending)
                    {
                        d.state = DraftState::Rejected;
                        d.error = Some(
                            "Connected account changed; create a new draft for this account".into(),
                        );
                        d.resolved_at_ms = Some(now_ms());
                    }
                }
                r.connector.status = "active".into();
                r.connector.account_label = Some(label);
                r.connector.updated_at_ms = now_ms();
                let result = r.connector.clone();
                update_tools(s, &pending.agent, true)?;
                Ok(result)
            })
            .await;
        if result.is_err() {
            let cleanup = if let Some(previous) = previous {
                self.credentials.put(&pending.id, previous).await
            } else {
                self.credentials.delete(&pending.id).await
            };
            if cleanup.is_err() {
                return Err(MailError::Credential);
            }
        }
        result
    }
    async fn token(
        &self,
        agent: &str,
        id: &str,
        p: Provider,
        config: &OAuthConfig,
    ) -> Result<GoogleOAuthTokens, MailError> {
        {
            let shared = self.shared()?;
            let s = shared.read().await;
            let r = Self::record(&s, agent, id, p)?;
            if r.connector.status != "active" {
                return Err(MailError::Unauthorized);
            }
        }
        let tokens = self
            .credentials
            .load(id)
            .await
            .map_err(|_| MailError::Credential)?;
        let Some(tokens) = tokens else {
            self.record_error(agent, id, p, MailError::Unauthorized)
                .await?;
            return Err(MailError::Unauthorized);
        };
        if tokens.expires_at_ms() > now_ms() + 60_000 {
            return Ok(tokens);
        }
        let refreshed = self.transport.refresh(config, tokens.refresh_token()).await;
        match refreshed {
            Ok(tokens) => {
                self.credentials
                    .put(id, tokens.clone())
                    .await
                    .map_err(|_| MailError::Credential)?;
                Ok(tokens)
            }
            Err(e) => {
                self.record_error(agent, id, p, e).await?;
                Err(e)
            }
        }
    }
    async fn record_error(
        &self,
        agent: &str,
        id: &str,
        p: Provider,
        e: MailError,
    ) -> Result<(), MailError> {
        self.edit(|s| {
            Self::record(s, agent, id, p)?;
            let r = s.mail_records.get_mut(id).ok_or(MailError::NotFound)?;
            r.connector.error = Some(e.to_string());
            if e == MailError::Unauthorized {
                r.connector.status = "reauthRequired".into()
            }
            r.connector.updated_at_ms = now_ms();
            Ok(())
        })
        .await
    }
    pub(crate) async fn messages(
        &self,
        agent: &str,
        id: &str,
        p: Provider,
        refresh: bool,
    ) -> Result<Vec<MailMessage>, MailError> {
        let oauth_lease = self
            .oauth_apps
            .locked_config(p.oauth_provider())
            .await
            .map_err(|_| MailError::Credential)?;
        let config = oauth_lease
            .config()
            .map(|resolved| OAuthConfig::from_resolved(p, resolved))
            .ok_or(MailError::Unconfigured)?;
        let lock = self.lock(agent, p).await;
        let _g = lock.lock().await;
        if refresh {
            let token = self.token(agent, id, p, &config).await?;
            let result = self.transport.inbox(p, token.access_token()).await;
            match result {
                Ok(mut messages) => {
                    messages.truncate(25);
                    self.edit(|s| {
                        Self::record(s, agent, id, p)?;
                        let r = s.mail_records.get_mut(id).ok_or(MailError::NotFound)?;
                        r.messages = messages;
                        r.connector.last_synced_at_ms = Some(now_ms());
                        r.connector.error = None;
                        Ok(())
                    })
                    .await?
                }
                Err(e) => {
                    self.record_error(agent, id, p, e).await?;
                    return Err(e);
                }
            }
        }
        let shared = self.shared()?;
        let s = shared.read().await;
        Ok(Self::record(&s, agent, id, p)?.messages.clone())
    }
    pub(crate) async fn drafts(
        &self,
        agent: &str,
        id: &str,
        p: Provider,
    ) -> Result<Vec<MailDraft>, MailError> {
        let shared = self.shared()?;
        let s = shared.read().await;
        Self::record(&s, agent, id, p)?;
        let mut drafts = s
            .mail_drafts
            .values()
            .filter(|d| d.connector_id == id)
            .cloned()
            .collect::<Vec<_>>();
        drafts.sort_by_key(|d| (!d.state.unresolved(), std::cmp::Reverse(d.created_at_ms)));
        let unresolved = drafts.iter().filter(|d| d.state.unresolved()).count();
        drafts.truncate(100.max(unresolved));
        Ok(drafts)
    }
    pub(crate) async fn create_draft(
        &self,
        agent: &str,
        id: &str,
        p: Provider,
        input: DraftInput,
    ) -> Result<MailDraft, MailError> {
        if !input.valid() {
            return Err(MailError::Invalid);
        }
        let lock = self.lock(agent, p).await;
        let _g = lock.lock().await;
        self.edit(|s| {
            let r = Self::record(s, agent, id, p)?;
            if r.connector.status != "active" {
                return Err(MailError::Unauthorized);
            }
            if s.mail_drafts
                .values()
                .filter(|d| d.connector_id == id && d.state.unresolved())
                .count()
                >= 50
            {
                return Err(MailError::Conflict);
            }
            let draft = MailDraft {
                id: uuid::Uuid::new_v4().to_string(),
                connector_id: id.into(),
                to: input.to,
                subject: input.subject,
                body: input.body,
                state: DraftState::Pending,
                error: None,
                created_at_ms: now_ms(),
                resolved_at_ms: None,
            };
            s.mail_drafts.insert(draft.id.clone(), draft.clone());
            Ok(draft)
        })
        .await
    }
    pub(crate) async fn resolve(
        &self,
        agent: &str,
        id: &str,
        p: Provider,
        draft_id: &str,
        approve: bool,
    ) -> Result<MailDraft, MailError> {
        // Once accepted, the daemon owns claim/send/finalization even if the
        // browser closes or the HTTP deadline expires. Never retry a send.
        let manager = self.clone();
        let agent = agent.to_string();
        let id = id.to_string();
        let draft_id = draft_id.to_string();
        tokio::spawn(async move {
            manager
                .resolve_owned(&agent, &id, p, &draft_id, approve)
                .await
        })
        .await
        .map_err(|_| MailError::Unknown)?
    }
    async fn resolve_owned(
        &self,
        agent: &str,
        id: &str,
        p: Provider,
        draft_id: &str,
        approve: bool,
    ) -> Result<MailDraft, MailError> {
        let oauth_lease = self
            .oauth_apps
            .locked_config(p.oauth_provider())
            .await
            .map_err(|_| MailError::Credential)?;
        let config = oauth_lease
            .config()
            .map(|resolved| OAuthConfig::from_resolved(p, resolved))
            .ok_or(MailError::Unconfigured)?;
        let lock = self.lock(agent, p).await;
        let _g = lock.lock().await;
        // Validate immutable pending draft before even refreshing a token.
        {
            let shared = self.shared()?;
            let s = shared.read().await;
            Self::record(&s, agent, id, p)?;
            let d = s
                .mail_drafts
                .get(draft_id)
                .filter(|d| d.connector_id == id)
                .ok_or(MailError::NotFound)?;
            if d.state != DraftState::Pending {
                return Err(MailError::Conflict);
            }
        }
        let token = if approve {
            Some(self.token(agent, id, p, &config).await?)
        } else {
            None
        };
        let claimed = self
            .edit(|s| {
                Self::record(s, agent, id, p)?;
                let d = s.mail_drafts.get_mut(draft_id).ok_or(MailError::NotFound)?;
                if d.state != DraftState::Pending {
                    return Err(MailError::Conflict);
                }
                d.state = if approve {
                    DraftState::Sending
                } else {
                    DraftState::Rejected
                };
                if !approve {
                    d.resolved_at_ms = Some(now_ms())
                }
                Ok(d.clone())
            })
            .await?;
        if let Some(token) = token {
            let result = {
                // Prevent agent deletion from interleaving between the last
                // lifecycle check and the externally visible send.
                let state = self.shared()?;
                let guard = state.read().await;
                Self::record(&guard, agent, id, p)?;
                self.transport.send(p, token.access_token(), &claimed).await
            };
            let reauth = matches!(result, Err(MailError::Unauthorized));
            let (status, error) = match result {
                Ok(()) => (DraftState::Sent, None),
                Err(MailError::Unknown) => {
                    (DraftState::Unknown, Some(MailError::Unknown.to_string()))
                }
                Err(e) => (DraftState::Failed, Some(e.to_string())),
            };
            self.edit(|s| {
                Self::record(s, agent, id, p)?;
                if reauth {
                    if let Some(r) = s.mail_records.get_mut(id) {
                        r.connector.status = "reauthRequired".into();
                        r.connector.error = Some(MailError::Unauthorized.to_string());
                    }
                }
                let d = s.mail_drafts.get_mut(draft_id).ok_or(MailError::NotFound)?;
                d.state = status;
                d.error = error;
                d.resolved_at_ms = Some(now_ms());
                Ok(d.clone())
            })
            .await
        } else {
            Ok(claimed)
        }
    }
    pub(crate) async fn disconnect(
        &self,
        agent: &str,
        id: &str,
        p: Provider,
    ) -> Result<(), MailError> {
        let _oauth_lease = self
            .oauth_apps
            .locked_config(p.oauth_provider())
            .await
            .map_err(|_| MailError::Credential)?;
        let lock = self.lock(agent, p).await;
        let _g = lock.lock().await;
        self.edit(|s| {
            Self::record(s, agent, id, p)?;
            s.mail_records
                .get_mut(id)
                .ok_or(MailError::NotFound)?
                .deleted = true;
            for d in s
                .mail_drafts
                .values_mut()
                .filter(|d| d.connector_id == id && d.state == DraftState::Pending)
            {
                d.state = DraftState::Rejected;
                d.resolved_at_ms = Some(now_ms())
            }
            if !s.mail_records.values().any(|r| {
                !r.deleted && r.connector.agent_id == agent && r.connector.status == "active"
            }) {
                update_tools(s, agent, false)?
            }
            Ok(())
        })
        .await?;
        self.pending.lock().await.retain(|_, v| v.id != id);
        self.credentials
            .delete(id)
            .await
            .map_err(|_| MailError::Credential)
    }
    pub(crate) fn start(&self) {
        let manager = self.clone();
        tokio::spawn(async move {
            // Router creation is synchronous and may encounter a busy state lock.
            // Complete manager installation before background work in all cases.
            if let Ok(state) = manager.shared() {
                state.write().await.mail_manager = Some(manager.clone());
            }
            loop {
                if manager.state.strong_count() == 0 {
                    break;
                }
                // Agent deletion shares this transaction gate. Reconcile only after
                // deletion has either durably committed or rolled back.
                let _ = manager
                    .edit(|s| {
                        let orphaned = s
                            .mail_records
                            .values()
                            .filter(|r| !r.deleted && s.get_agent(&r.connector.agent_id).is_none())
                            .map(|r| r.connector.id.clone())
                            .collect::<Vec<_>>();
                        for id in orphaned {
                            if let Some(r) = s.mail_records.get_mut(&id) {
                                r.deleted = true;
                                r.messages.clear();
                            }
                            for d in s
                                .mail_drafts
                                .values_mut()
                                .filter(|d| d.connector_id == id && d.state == DraftState::Pending)
                            {
                                d.state = DraftState::Rejected;
                                d.resolved_at_ms = Some(now_ms());
                            }
                        }
                        Ok(())
                    })
                    .await;
                let records = match manager.shared() {
                    Ok(s) => s
                        .read()
                        .await
                        .mail_records
                        .values()
                        .cloned()
                        .collect::<Vec<_>>(),
                    Err(_) => break,
                };
                for r in records {
                    if r.deleted {
                        let _ = manager.credentials.delete(&r.connector.id).await;
                    } else if r.connector.status == "active" {
                        let _ = manager
                            .messages(
                                &r.connector.agent_id,
                                &r.connector.id,
                                r.connector.provider,
                                true,
                            )
                            .await;
                    }
                }
                tokio::time::sleep(Duration::from_secs(120)).await;
            }
        });
    }
}
fn update_tools(s: &mut DaemonState, agent: &str, add: bool) -> Result<(), MailError> {
    let snapshot = s.get_agent(agent).ok_or(MailError::NotFound)?;
    let tools = ["mail_list_messages", "mail_create_draft"];
    let mut names = snapshot
        .state
        .config
        .tools
        .unwrap_or_default()
        .into_iter()
        .map(|t| t.name)
        .filter(|n| add || !tools.contains(&n.as_str()))
        .collect::<Vec<_>>();
    if add {
        for name in tools {
            if !names.iter().any(|n| n == name) {
                names.push(name.into())
            }
        }
    }
    let descriptors = s
        .tool_registry
        .resolve_descriptors(names)
        .map_err(|_| MailError::Persistence)?;
    s.update_agent(
        agent,
        anima_core::AgentConfigUpdate {
            tools: Some(descriptors),
            ..Default::default()
        },
    )
    .map_err(|_| MailError::NotFound)?;
    Ok(())
}
