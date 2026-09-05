//! Google Calendar connector: OAuth connect, calendar tools for agent runs,
//! and confirmation-gated writes. No polling loop — calendar data is fetched
//! on demand when an agent calls a calendar tool, and proactive behavior is
//! expressed through ordinary scheduled prompts.

pub(crate) mod client;
pub(crate) mod store;
#[cfg(test)]
mod tests;

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use zeroize::Zeroizing;

use crate::agent_runs::{AgentRunCoordinator, AgentRunRequest, RunRoom};
use crate::app::SharedDaemonState;
use crate::connectors::oauth_apps::{OAuthAppService, OAuthProvider, ResolvedOAuthApp};
use crate::state::DaemonState;

use self::client::{GoogleCalendarEvent, GoogleCalendarTransport, GoogleTransportError};
use self::store::GoogleCredentialStore;

pub(crate) const CALENDAR_TOOL_NAMES: [&str; 4] = [
    "calendar_list_events",
    "calendar_create_event",
    "calendar_update_event",
    "calendar_delete_event",
];

const PENDING_AUTH_TTL_MS: u64 = 10 * 60 * 1000;
const PENDING_WRITE_TTL_MS: u64 = 24 * 60 * 60 * 1000;
const TOKEN_REFRESH_SKEW_MS: u64 = 60 * 1000;
const MAX_CALENDAR_ID_LENGTH: usize = 256;
const MAX_EVENT_FIELD_SCALARS: usize = 1024;
const MAX_EVENT_DESCRIPTION_SCALARS: usize = 8192;
const MAX_PENDING_WRITES_PER_AGENT: usize = 50;

static CONNECTOR_ID_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static WRITE_ID_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Durable records (never contain secrets)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GoogleCalendarConnectorRecord {
    pub(crate) id: String,
    pub(crate) agent_id: String,
    /// Non-secret account label (usually the Google account email) shown in
    /// the console. Verified after OAuth completes; older records may lack it.
    #[serde(default)]
    pub(crate) account_label: Option<String>,
    #[serde(default = "default_calendar_ids")]
    pub(crate) calendar_ids: Vec<String>,
    #[serde(default)]
    pub(crate) pending_auth: Option<GooglePendingAuth>,
    #[serde(default)]
    pub(crate) reauth_required: bool,
    #[serde(default = "default_enabled")]
    pub(crate) enabled: bool,
    #[serde(default)]
    pub(crate) deleted_at_ms: Option<u64>,
    pub(crate) created_at_ms: u64,
    pub(crate) updated_at_ms: u64,
}

impl GoogleCalendarConnectorRecord {
    pub(crate) fn is_active(&self) -> bool {
        self.enabled && self.deleted_at_ms.is_none() && self.pending_auth.is_none()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GooglePendingAuth {
    pub(crate) nonce: String,
    pub(crate) expires_at_ms: u64,
    #[serde(default)]
    pub(crate) config_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CalendarEventDraft {
    pub(crate) calendar_id: String,
    #[serde(default)]
    pub(crate) event_id: Option<String>,
    pub(crate) title: String,
    pub(crate) start: String,
    pub(crate) end: String,
    #[serde(default)]
    pub(crate) location: Option<String>,
    #[serde(default)]
    pub(crate) description: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum CalendarWriteOperation {
    Create,
    Update,
    Delete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum CalendarWriteState {
    Pending,
    Applied,
    Rejected,
    Failed,
}

impl CalendarWriteState {
    pub(crate) fn is_terminal(self) -> bool {
        !matches!(self, Self::Pending)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CalendarPendingWriteRecord {
    pub(crate) id: String,
    pub(crate) connector_id: String,
    pub(crate) agent_id: String,
    pub(crate) operation: CalendarWriteOperation,
    pub(crate) draft: CalendarEventDraft,
    /// Human-readable one-liner shown on the confirmation card.
    pub(crate) summary: String,
    pub(crate) state: CalendarWriteState,
    #[serde(default)]
    pub(crate) error: Option<String>,
    pub(crate) created_at_ms: u64,
    #[serde(default)]
    pub(crate) resolved_at_ms: Option<u64>,
}

fn default_calendar_ids() -> Vec<String> {
    vec!["primary".to_string()]
}

fn default_enabled() -> bool {
    true
}

// ---------------------------------------------------------------------------
// OAuth configuration (daemon-level, from environment)
// ---------------------------------------------------------------------------

const DEFAULT_REDIRECT_URI: &str = "http://127.0.0.1:8080/api/connectors/gcalendar/callback";

#[derive(Clone)]
pub(crate) struct GoogleOAuthConfig {
    pub(crate) client_id: String,
    client_secret: Zeroizing<String>,
    pub(crate) redirect_uri: String,
}

impl GoogleOAuthConfig {
    fn from_resolved(resolved: &ResolvedOAuthApp) -> Self {
        Self {
            client_id: resolved.credentials().client_id().to_string(),
            client_secret: Zeroizing::new(resolved.credentials().client_secret().to_string()),
            redirect_uri: DEFAULT_REDIRECT_URI.to_string(),
        }
    }
    #[cfg(test)]
    pub(crate) fn for_tests() -> Self {
        Self {
            client_id: "test-client-id".to_string(),
            client_secret: Zeroizing::new("test-client-secret".to_string()),
            redirect_uri: DEFAULT_REDIRECT_URI.to_string(),
        }
    }

    pub(crate) fn client_secret(&self) -> &str {
        self.client_secret.as_str()
    }

    pub(crate) fn consent_url(&self, nonce: &str) -> String {
        let scope = "https://www.googleapis.com/auth/calendar.events";
        format!(
            "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&redirect_uri={}&response_type=code&scope={}&access_type=offline&prompt=consent&state={}",
            url_encode(&self.client_id),
            url_encode(&self.redirect_uri),
            url_encode(scope),
            url_encode(nonce),
        )
    }
}

impl fmt::Debug for GoogleOAuthConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GoogleOAuthConfig")
            .field("client_id", &self.client_id)
            .field("client_secret", &"[REDACTED]")
            .field("redirect_uri", &self.redirect_uri)
            .finish()
    }
}

pub(crate) fn url_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CalendarError {
    AgentNotFound,
    ConnectorNotFound,
    AlreadyConnected,
    /// OAuth client credentials are not configured on the daemon.
    Unconfigured,
    PairingNotFound,
    NotConnected,
    ReauthRequired,
    WriteNotPending,
    /// Too many pending writes await approval.
    Conflict,
    Transport,
    Credential,
    Persistence,
    InvalidDraft,
}

// ---------------------------------------------------------------------------
// Manager
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub(crate) struct CalendarManager {
    state: Weak<RwLock<DaemonState>>,
    agent_runs: AgentRunCoordinator,
    credentials: Arc<dyn GoogleCredentialStore>,
    transport: Arc<dyn GoogleCalendarTransport>,
    pub(crate) oauth_apps: OAuthAppService,
}

impl CalendarManager {
    pub(crate) fn new(
        state: &SharedDaemonState,
        agent_runs: AgentRunCoordinator,
        credentials: Arc<dyn GoogleCredentialStore>,
        transport: Arc<dyn GoogleCalendarTransport>,
        oauth_apps: OAuthAppService,
    ) -> Self {
        Self {
            state: Arc::downgrade(state),
            agent_runs,
            credentials,
            transport,
            oauth_apps,
        }
    }

    pub(crate) async fn oauth_configured(&self) -> Result<bool, CalendarError> {
        self.oauth_apps
            .status(OAuthProvider::Google)
            .await
            .map(|status| status.configured)
            .map_err(|_| CalendarError::Credential)
    }

    pub(crate) async fn has_provider_dependency(&self, provider: OAuthProvider) -> bool {
        if provider != OAuthProvider::Google {
            return false;
        }
        let Some(state) = self.state.upgrade() else {
            return false;
        };
        let in_use = state
            .read()
            .await
            .calendar_connectors
            .values()
            .any(|record| record.deleted_at_ms.is_none());
        in_use
    }

    fn shared_state(&self) -> Result<SharedDaemonState, CalendarError> {
        self.state.upgrade().ok_or(CalendarError::Persistence)
    }

    pub(crate) async fn connector_for_agent(
        &self,
        agent_id: &str,
    ) -> Option<GoogleCalendarConnectorRecord> {
        let state = self.state.upgrade()?;
        let guard = state.read().await;
        guard
            .calendar_connectors
            .values()
            .find(|connector| connector.agent_id == agent_id && connector.deleted_at_ms.is_none())
            .cloned()
    }

    /// Starts or restarts OAuth, preserving connector identity and pending writes.
    /// A new nonce invalidates all previously issued consent links.
    pub(crate) async fn begin_connect(
        &self,
        agent_id: &str,
    ) -> Result<(GoogleCalendarConnectorRecord, String), CalendarError> {
        let oauth_lease = self
            .oauth_apps
            .locked_config(OAuthProvider::Google)
            .await
            .map_err(|_| CalendarError::Credential)?;
        let oauth = oauth_lease
            .config()
            .map(GoogleOAuthConfig::from_resolved)
            .ok_or(CalendarError::Unconfigured)?;
        let config_revision = oauth_lease.revision();
        let state = self.shared_state()?;
        let now = now_ms();
        let _transaction = self.agent_runs.control_plane_transaction().await;
        let mut record = {
            let guard = state.read().await;
            if guard.get_agent(agent_id).is_none() {
                return Err(CalendarError::AgentNotFound);
            }
            if let Some(existing) = guard.calendar_connectors.values().find(|connector| {
                connector.agent_id == agent_id && connector.deleted_at_ms.is_none()
            }) {
                existing.clone()
            } else {
                let sequence = CONNECTOR_ID_SEQUENCE.fetch_add(1, Ordering::Relaxed);
                GoogleCalendarConnectorRecord {
                    id: format!("gcalendar-{now}-{sequence}"),
                    agent_id: agent_id.to_string(),
                    account_label: None,
                    calendar_ids: default_calendar_ids(),
                    pending_auth: Some(GooglePendingAuth {
                        nonce: uuid::Uuid::new_v4().to_string(),
                        expires_at_ms: now + PENDING_AUTH_TTL_MS,
                        config_revision,
                    }),
                    reauth_required: false,
                    enabled: true,
                    deleted_at_ms: None,
                    created_at_ms: now,
                    updated_at_ms: now,
                }
            }
        };
        record.pending_auth = Some(GooglePendingAuth {
            nonce: uuid::Uuid::new_v4().to_string(),
            expires_at_ms: now + PENDING_AUTH_TTL_MS,
            config_revision,
        });
        record.updated_at_ms = now;
        let consent_url = oauth.consent_url(
            &record
                .pending_auth
                .as_ref()
                .expect("pending auth was just set")
                .nonce,
        );

        let persist = {
            let mut guard = state.write().await;
            guard
                .calendar_connectors
                .insert(record.id.clone(), record.clone());
            guard.control_plane_persist_request()
        };
        persist
            .save()
            .await
            .map_err(|_| CalendarError::Persistence)?;
        Ok((record, consent_url))
    }

    /// Completes OAuth from the Google redirect callback. The `nonce` is the
    /// CSRF `state` parameter issued by [`CalendarManager::begin_connect`].
    pub(crate) async fn complete_connect(
        &self,
        nonce: &str,
        code: &str,
    ) -> Result<GoogleCalendarConnectorRecord, CalendarError> {
        let oauth_lease = self
            .oauth_apps
            .locked_config(OAuthProvider::Google)
            .await
            .map_err(|_| CalendarError::Credential)?;
        let oauth = oauth_lease
            .config()
            .map(GoogleOAuthConfig::from_resolved)
            .ok_or(CalendarError::Unconfigured)?;
        let state = self.shared_state()?;
        let now = now_ms();
        // Consume the nonce durably before contacting Google. Keep an expired
        // pending record so a failed exchange cannot accidentally activate old tokens.
        let (connector_id, original_account) = {
            let _transaction = self.agent_runs.control_plane_transaction().await;
            let mut guard = state.write().await;
            let record = guard
                .calendar_connectors
                .values_mut()
                .find(|connector| {
                    connector.enabled
                        && connector.deleted_at_ms.is_none()
                        && connector.pending_auth.as_ref().is_some_and(|pending| {
                            pending.nonce == nonce
                                && pending.expires_at_ms >= now
                                && oauth_lease.revision_is_current(pending.config_revision)
                        })
                })
                .ok_or(CalendarError::PairingNotFound)?;
            let identity = (record.id.clone(), record.account_label.clone());
            record
                .pending_auth
                .as_mut()
                .expect("validated nonce")
                .expires_at_ms = 0;
            record.reauth_required = true;
            record.updated_at_ms = now;
            let persist = guard.control_plane_persist_request();
            drop(guard);
            persist
                .save()
                .await
                .map_err(|_| CalendarError::Persistence)?;
            identity
        };

        let tokens = self
            .transport
            .exchange_code(&oauth, code)
            .await
            .map_err(|_| CalendarError::Transport)?;
        let account_label = self
            .transport
            .primary_calendar(tokens.access_token())
            .await
            .map_err(|_| CalendarError::Transport)?;
        if original_account
            .as_ref()
            .is_some_and(|original| original != &account_label)
        {
            return Err(CalendarError::ReauthRequired);
        }

        let _transaction = self.agent_runs.control_plane_transaction().await;
        // Revalidate after network awaits, including deletion outside this manager.
        {
            let guard = state.read().await;
            let valid = guard
                .calendar_connectors
                .get(&connector_id)
                .is_some_and(|record| {
                    record.enabled
                        && record.deleted_at_ms.is_none()
                        && guard.get_agent(&record.agent_id).is_some()
                        && record.pending_auth.as_ref().is_some_and(|pending| {
                            pending.nonce == nonce && pending.expires_at_ms == 0
                        })
                });
            if !valid {
                return Err(CalendarError::PairingNotFound);
            }
        }
        self.credentials
            .put(&connector_id, tokens)
            .await
            .map_err(|_| CalendarError::Credential)?;
        let persist = {
            let mut guard = state.write().await;
            let Some(record) = guard.calendar_connectors.get_mut(&connector_id) else {
                return Err(CalendarError::ConnectorNotFound);
            };
            record.pending_auth = None;
            record.account_label = Some(account_label);
            record.reauth_required = false;
            record.updated_at_ms = now_ms();
            let agent_id = record.agent_id.clone();
            if original_account.is_none() {
                // Legacy connections could lack an account label. No old write
                // can be safely bound to the newly verified Google identity.
                for write in guard.calendar_writes.values_mut() {
                    if write.connector_id == connector_id
                        && write.state == CalendarWriteState::Pending
                    {
                        write.state = CalendarWriteState::Rejected;
                        write.error = Some("original calendar account could not be verified; review and create a new change".into());
                        write.resolved_at_ms = Some(now_ms());
                    }
                }
            }
            add_calendar_tools(&mut guard, &agent_id)?;
            guard.control_plane_persist_request()
        };
        persist
            .save()
            .await
            .map_err(|_| CalendarError::Persistence)?;

        let guard = state.read().await;
        guard
            .calendar_connectors
            .get(&connector_id)
            .cloned()
            .ok_or(CalendarError::ConnectorNotFound)
    }

    pub(crate) async fn disconnect(
        &self,
        agent_id: &str,
        connector_id: &str,
    ) -> Result<(), CalendarError> {
        let _oauth_lease = self
            .oauth_apps
            .locked_config(OAuthProvider::Google)
            .await
            .map_err(|_| CalendarError::Credential)?;
        let state = self.shared_state()?;
        {
            let guard = state.read().await;
            let owned = guard
                .calendar_connectors
                .get(connector_id)
                .is_some_and(|connector| {
                    connector.agent_id == agent_id && connector.deleted_at_ms.is_none()
                });
            if !owned {
                return Err(CalendarError::ConnectorNotFound);
            }
        }
        self.credentials
            .delete(connector_id)
            .await
            .map_err(|_| CalendarError::Credential)?;

        let now = now_ms();
        let _transaction = self.agent_runs.control_plane_transaction().await;
        let persist = {
            let mut guard = state.write().await;
            let now_record = guard.calendar_connectors.get_mut(connector_id);
            let Some(record) = now_record else {
                return Err(CalendarError::ConnectorNotFound);
            };
            record.enabled = false;
            record.pending_auth = None;
            record.deleted_at_ms = Some(now);
            record.updated_at_ms = now;
            remove_calendar_tools(&mut guard, agent_id);
            // Any still-pending writes for this connector can never be applied.
            for write in guard.calendar_writes.values_mut() {
                if write.connector_id == connector_id && write.state == CalendarWriteState::Pending
                {
                    write.state = CalendarWriteState::Rejected;
                    write.error = Some("calendar connector disconnected".to_string());
                    write.resolved_at_ms = Some(now);
                }
            }
            guard.control_plane_persist_request()
        };
        persist
            .save()
            .await
            .map_err(|_| CalendarError::Persistence)?;
        Ok(())
    }

    /// Lists events for the agent's connector. Used by the read tool.
    pub(crate) async fn list_events_for_agent(
        &self,
        agent_id: &str,
        calendar_id: Option<&str>,
        time_min: &str,
        time_max: &str,
    ) -> Result<Vec<GoogleCalendarEvent>, CalendarError> {
        let connector = self
            .connector_for_agent(agent_id)
            .await
            .filter(|connector| connector.is_active())
            .ok_or(CalendarError::NotConnected)?;
        if connector.reauth_required {
            return Err(CalendarError::ReauthRequired);
        }
        let calendar_id = calendar_id
            .map(str::to_string)
            .unwrap_or_else(|| connector.calendar_ids[0].clone());
        validate_calendar_id(&calendar_id)?;
        let access_token = self.fresh_access_token(&connector).await?;
        self.transport
            .list_events(&access_token, &calendar_id, time_min, time_max)
            .await
            .map_err(map_transport_error)
    }

    /// Creates a pending write from a tool call. Never touches Google.
    pub(crate) async fn submit_write(
        &self,
        agent_id: &str,
        operation: CalendarWriteOperation,
        mut draft: CalendarEventDraft,
    ) -> Result<CalendarPendingWriteRecord, CalendarError> {
        let connector = self
            .connector_for_agent(agent_id)
            .await
            .filter(|connector| connector.is_active())
            .ok_or(CalendarError::NotConnected)?;
        if draft.calendar_id.is_empty() {
            draft.calendar_id = connector.calendar_ids[0].clone();
        }
        validate_draft(operation, &draft)?;
        let summary = summarize_write(operation, &draft);

        let state = self.shared_state()?;
        let now = now_ms();
        let sequence = WRITE_ID_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let record = CalendarPendingWriteRecord {
            id: format!("calendar-write-{now}-{sequence}"),
            connector_id: connector.id.clone(),
            agent_id: agent_id.to_string(),
            operation,
            draft,
            summary,
            state: CalendarWriteState::Pending,
            error: None,
            created_at_ms: now,
            resolved_at_ms: None,
        };

        let _transaction = self.agent_runs.control_plane_transaction().await;
        let persist = {
            let mut guard = state.write().await;
            let pending_count = guard
                .calendar_writes
                .values()
                .filter(|write| {
                    write.agent_id == agent_id && write.state == CalendarWriteState::Pending
                })
                .count();
            if pending_count >= MAX_PENDING_WRITES_PER_AGENT {
                return Err(CalendarError::Conflict);
            }
            guard
                .calendar_writes
                .insert(record.id.clone(), record.clone());
            guard.control_plane_persist_request()
        };
        persist
            .save()
            .await
            .map_err(|_| CalendarError::Persistence)?;
        Ok(record)
    }

    pub(crate) async fn list_writes(
        &self,
        agent_id: &str,
        connector_id: &str,
    ) -> Result<Vec<CalendarPendingWriteRecord>, CalendarError> {
        self.expire_stale_writes().await?;
        let state = self.shared_state()?;
        let guard = state.read().await;
        let mut writes = guard
            .calendar_writes
            .values()
            .filter(|write| write.agent_id == agent_id && write.connector_id == connector_id)
            .cloned()
            .collect::<Vec<_>>();
        writes.sort_by(|left, right| {
            right
                .created_at_ms
                .cmp(&left.created_at_ms)
                .then_with(|| right.id.cmp(&left.id))
        });
        Ok(writes)
    }

    pub(crate) async fn approve_write(
        &self,
        agent_id: &str,
        connector_id: &str,
        write_id: &str,
    ) -> Result<CalendarPendingWriteRecord, CalendarError> {
        self.expire_stale_writes().await?;
        let state = self.shared_state()?;
        let (connector, write) = {
            let guard = state.read().await;
            let connector = guard
                .calendar_connectors
                .get(connector_id)
                .filter(|connector| {
                    connector.agent_id == agent_id && connector.deleted_at_ms.is_none()
                })
                .cloned()
                .ok_or(CalendarError::ConnectorNotFound)?;
            let write = guard
                .calendar_writes
                .get(write_id)
                .filter(|write| write.agent_id == agent_id && write.connector_id == connector_id)
                .cloned()
                .ok_or(CalendarError::ConnectorNotFound)?;
            (connector, write)
        };
        if write.state != CalendarWriteState::Pending {
            return Err(CalendarError::WriteNotPending);
        }
        if !connector.is_active() || connector.reauth_required {
            return Err(CalendarError::ReauthRequired);
        }

        let applied = match self.fresh_access_token(&connector).await {
            Ok(access_token) => match write.operation {
                CalendarWriteOperation::Create => self
                    .transport
                    .create_event(&access_token, &write.draft)
                    .await
                    .map(|_| ()),
                CalendarWriteOperation::Update => {
                    self.transport
                        .update_event(&access_token, &write.draft)
                        .await
                }
                CalendarWriteOperation::Delete => {
                    self.transport
                        .delete_event(&access_token, &write.draft)
                        .await
                }
            }
            .map_err(map_transport_error),
            Err(error) => Err(error),
        };

        let now = now_ms();
        let (next_state, error_text) = match applied {
            Ok(()) => (CalendarWriteState::Applied, None),
            Err(CalendarError::ReauthRequired) => {
                // Keep the write actionable after the owner reconnects.
                return Err(CalendarError::ReauthRequired);
            }
            Err(_) => (
                CalendarWriteState::Failed,
                Some("Google Calendar rejected or could not apply this change".to_string()),
            ),
        };

        let _transaction = self.agent_runs.control_plane_transaction().await;
        let persist = {
            let mut guard = state.write().await;
            let Some(record) = guard.calendar_writes.get_mut(write_id) else {
                return Err(CalendarError::ConnectorNotFound);
            };
            record.state = next_state;
            record.error = error_text;
            record.resolved_at_ms = Some(now);
            guard.control_plane_persist_request()
        };
        persist
            .save()
            .await
            .map_err(|_| CalendarError::Persistence)?;

        let updated = {
            let guard = state.read().await;
            guard
                .calendar_writes
                .get(write_id)
                .cloned()
                .ok_or(CalendarError::ConnectorNotFound)?
        };

        if next_state == CalendarWriteState::Applied {
            self.notify_agent_write_applied(&updated);
        }
        Ok(updated)
    }

    pub(crate) async fn reject_write(
        &self,
        agent_id: &str,
        connector_id: &str,
        write_id: &str,
    ) -> Result<CalendarPendingWriteRecord, CalendarError> {
        let state = self.shared_state()?;
        let now = now_ms();
        let _transaction = self.agent_runs.control_plane_transaction().await;
        let persist = {
            let mut guard = state.write().await;
            let Some(record) = guard.calendar_writes.get_mut(write_id) else {
                return Err(CalendarError::ConnectorNotFound);
            };
            if record.agent_id != agent_id || record.connector_id != connector_id {
                return Err(CalendarError::ConnectorNotFound);
            }
            if record.state != CalendarWriteState::Pending {
                return Err(CalendarError::WriteNotPending);
            }
            record.state = CalendarWriteState::Rejected;
            record.resolved_at_ms = Some(now);
            guard.control_plane_persist_request()
        };
        persist
            .save()
            .await
            .map_err(|_| CalendarError::Persistence)?;
        let guard = state.read().await;
        guard
            .calendar_writes
            .get(write_id)
            .cloned()
            .ok_or(CalendarError::ConnectorNotFound)
    }

    /// Lazily expires pending writes older than the TTL.
    async fn expire_stale_writes(&self) -> Result<(), CalendarError> {
        let state = self.shared_state()?;
        let now = now_ms();
        let stale_ids = {
            let guard = state.read().await;
            guard
                .calendar_writes
                .values()
                .filter(|write| {
                    write.state == CalendarWriteState::Pending
                        && write.created_at_ms + PENDING_WRITE_TTL_MS <= now
                })
                .map(|write| write.id.clone())
                .collect::<Vec<_>>()
        };
        if stale_ids.is_empty() {
            return Ok(());
        }
        let _transaction = self.agent_runs.control_plane_transaction().await;
        let persist = {
            let mut guard = state.write().await;
            for id in &stale_ids {
                if let Some(write) = guard.calendar_writes.get_mut(id) {
                    if write.state == CalendarWriteState::Pending {
                        write.state = CalendarWriteState::Rejected;
                        write.error = Some("confirmation expired".to_string());
                        write.resolved_at_ms = Some(now);
                    }
                }
            }
            guard.control_plane_persist_request()
        };
        persist
            .save()
            .await
            .map_err(|_| CalendarError::Persistence)?;
        Ok(())
    }

    /// Loads tokens from the vault and refreshes them when expired. On a
    /// rejected refresh the connector is flagged `reauth_required`.
    async fn fresh_access_token(
        &self,
        connector: &GoogleCalendarConnectorRecord,
    ) -> Result<String, CalendarError> {
        let tokens = self
            .credentials
            .load(&connector.id)
            .await
            .map_err(|_| CalendarError::Credential)?
            .ok_or(CalendarError::ReauthRequired)?;
        let now = now_ms();
        if tokens.expires_at_ms() > now + TOKEN_REFRESH_SKEW_MS {
            return Ok(tokens.access_token().to_string());
        }
        let oauth_lease = self
            .oauth_apps
            .locked_config(OAuthProvider::Google)
            .await
            .map_err(|_| CalendarError::Credential)?;
        let oauth = oauth_lease
            .config()
            .map(GoogleOAuthConfig::from_resolved)
            .ok_or(CalendarError::ReauthRequired)?;
        let refreshed = match self
            .transport
            .refresh_tokens(&oauth, tokens.refresh_token())
            .await
        {
            Ok(refreshed) => refreshed,
            Err(GoogleTransportError::Unauthorized) => {
                self.flag_reauth_required(&connector.id).await;
                return Err(CalendarError::ReauthRequired);
            }
            Err(_) => return Err(CalendarError::Transport),
        };
        let access_token = refreshed.access_token().to_string();
        self.credentials
            .put(&connector.id, refreshed)
            .await
            .map_err(|_| CalendarError::Credential)?;
        Ok(access_token)
    }

    /// Marks the connector as needing reauthorization (best-effort persist).
    async fn flag_reauth_required(&self, connector_id: &str) {
        let Some(state) = self.state.upgrade() else {
            return;
        };
        let _transaction = self.agent_runs.control_plane_transaction().await;
        let persist = {
            let mut guard = state.write().await;
            let now = now_ms();
            match guard.calendar_connectors.get_mut(connector_id) {
                Some(record) if !record.reauth_required => {
                    record.reauth_required = true;
                    record.updated_at_ms = now;
                    Some(guard.control_plane_persist_request())
                }
                _ => None,
            }
        };
        if let Some(persist) = persist {
            let _ = persist.save().await;
        }
    }

    /// Posts the applied outcome back into the agent's workspace room so the
    /// conversation reflects the confirmed write. Best-effort.
    fn notify_agent_write_applied(&self, write: &CalendarPendingWriteRecord) {
        let coordinator = self.agent_runs.clone();
        let agent_id = write.agent_id.clone();
        let write_id = write.id.clone();
        let text = format!(
            "Calendar change confirmed and applied: {}. Continue the conversation accordingly.",
            write.summary
        );
        tokio::spawn(async move {
            let _ = coordinator
                .run(AgentRunRequest {
                    agent_id,
                    content: anima_core::Content {
                        text,
                        ..Default::default()
                    },
                    room: RunRoom::Generated,
                    idempotency_key: Some(format!("calendar-write:{write_id}")),
                })
                .await;
        });
    }
}

// ---------------------------------------------------------------------------
// Validation and mutation helpers
// ---------------------------------------------------------------------------

fn map_transport_error(error: GoogleTransportError) -> CalendarError {
    match error {
        GoogleTransportError::Unauthorized => CalendarError::ReauthRequired,
        _ => CalendarError::Transport,
    }
}

fn validate_calendar_id(calendar_id: &str) -> Result<(), CalendarError> {
    let valid = !calendar_id.is_empty()
        && calendar_id.len() <= MAX_CALENDAR_ID_LENGTH
        && calendar_id.chars().all(|ch| !ch.is_control());
    if valid {
        Ok(())
    } else {
        Err(CalendarError::InvalidDraft)
    }
}

fn field_within_limits(value: &str, max_scalars: usize) -> bool {
    !value.is_empty()
        && value.chars().count() <= max_scalars
        && value.chars().all(|ch| !ch.is_control())
}

fn validate_draft(
    operation: CalendarWriteOperation,
    draft: &CalendarEventDraft,
) -> Result<(), CalendarError> {
    validate_calendar_id(&draft.calendar_id)?;
    match operation {
        CalendarWriteOperation::Create => {
            if !field_within_limits(&draft.title, MAX_EVENT_FIELD_SCALARS)
                || !field_within_limits(&draft.start, 64)
                || !field_within_limits(&draft.end, 64)
            {
                return Err(CalendarError::InvalidDraft);
            }
        }
        CalendarWriteOperation::Update => {
            let has_change = !draft.title.is_empty()
                || !draft.start.is_empty()
                || !draft.end.is_empty()
                || draft.location.is_some()
                || draft.description.is_some();
            if !has_change {
                return Err(CalendarError::InvalidDraft);
            }
        }
        CalendarWriteOperation::Delete => {}
    }
    if matches!(
        operation,
        CalendarWriteOperation::Update | CalendarWriteOperation::Delete
    ) {
        let event_id = draft.event_id.as_deref().unwrap_or("");
        if !field_within_limits(event_id, MAX_EVENT_FIELD_SCALARS) {
            return Err(CalendarError::InvalidDraft);
        }
    }
    if let Some(location) = &draft.location {
        if !field_within_limits(location, MAX_EVENT_FIELD_SCALARS) {
            return Err(CalendarError::InvalidDraft);
        }
    }
    if let Some(description) = &draft.description {
        if description.chars().count() > MAX_EVENT_DESCRIPTION_SCALARS {
            return Err(CalendarError::InvalidDraft);
        }
    }
    Ok(())
}

fn summarize_write(operation: CalendarWriteOperation, draft: &CalendarEventDraft) -> String {
    match operation {
        CalendarWriteOperation::Create => format!(
            "Create event \"{}\" ({} → {})",
            draft.title, draft.start, draft.end
        ),
        CalendarWriteOperation::Update => format!(
            "Update event {}",
            draft.event_id.as_deref().unwrap_or("(unknown)")
        ),
        CalendarWriteOperation::Delete => format!(
            "Delete event {}",
            draft.event_id.as_deref().unwrap_or("(unknown)")
        ),
    }
}

/// Ensures the agent's config advertises the calendar tools so the model can
/// call them while a connector is active.
fn add_calendar_tools(guard: &mut DaemonState, agent_id: &str) -> Result<(), CalendarError> {
    let Some(snapshot) = guard.get_agent(agent_id) else {
        return Err(CalendarError::AgentNotFound);
    };
    let mut names = snapshot
        .state
        .config
        .tools
        .as_ref()
        .map(|tools| {
            tools
                .iter()
                .map(|tool| tool.name.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for name in CALENDAR_TOOL_NAMES {
        if !names.iter().any(|existing| existing == name) {
            names.push(name.to_string());
        }
    }
    let descriptors = guard
        .tool_registry
        .resolve_descriptors(names)
        .map_err(|_| CalendarError::Persistence)?;
    guard
        .update_agent(
            agent_id,
            anima_core::AgentConfigUpdate {
                tools: Some(descriptors),
                ..Default::default()
            },
        )
        .map_err(|_| CalendarError::AgentNotFound)?;
    Ok(())
}

fn remove_calendar_tools(guard: &mut DaemonState, agent_id: &str) {
    let Some(snapshot) = guard.get_agent(agent_id) else {
        return;
    };
    let names = snapshot
        .state
        .config
        .tools
        .as_ref()
        .map(|tools| {
            tools
                .iter()
                .map(|tool| tool.name.clone())
                .filter(|name| !CALENDAR_TOOL_NAMES.contains(&name.as_str()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let Ok(descriptors) = guard.tool_registry.resolve_descriptors(names) else {
        return;
    };
    let _ = guard.update_agent(
        agent_id,
        anima_core::AgentConfigUpdate {
            tools: Some(descriptors),
            ..Default::default()
        },
    );
}
