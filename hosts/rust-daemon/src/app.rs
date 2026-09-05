pub(crate) mod lifecycle;
pub(crate) mod persistence;

use std::io;
use std::sync::Arc;
use std::time::Duration;

use anima_core::DatabaseAdapter;
use async_trait::async_trait;
use axum::Router;
use tokio::net::TcpListener;
use tokio::sync::{RwLock, Semaphore};

use self::{lifecycle::shutdown_signal, persistence::configure_persistence};
use crate::agent_runs::AgentRunCoordinator;
use crate::connectors::credentials::{
    InMemoryCredentialStore, OsKeyringCredentialStore, TelegramBotToken,
};
use crate::connectors::runtime::{ConnectorManager, TelegramTransport};
use crate::connectors::telegram::{
    TelegramClient, TelegramSentMessage, TelegramTransportError, TelegramUpdateBatch,
};
use crate::connectors::{
    TelegramBotIdentity, TelegramChatKind, TelegramChatMetadata, TelegramSenderMetadata,
};
use crate::events::{EventFanout, DEFAULT_EVENT_BUFFER};
use crate::routes;
use crate::runtime_model::RuntimeModelAdapter;
use crate::schedules::SchedulerService;
use crate::state::DaemonState;
use crate::tools::DEFAULT_MAX_BACKGROUND_PROCESSES;

pub(crate) type SharedDaemonState = Arc<RwLock<DaemonState>>;

struct DaemonRuntime {
    run_limiter: Arc<Semaphore>,
    agent_runs: AgentRunCoordinator,
    connectors: ConnectorManager,
    calendar: crate::connectors::gcalendar::CalendarManager,
    mail: crate::connectors::mail::MailManager,
    oauth_apps: crate::connectors::oauth_apps::OAuthAppService,
    scheduler: SchedulerService,
}

const DEFAULT_MAX_CONCURRENT_RUNS: usize = 8;
const DEFAULT_DB_MAX_CONNECTIONS: u32 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PersistenceMode {
    Memory,
    Postgres,
}

impl PersistenceMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Postgres => "postgres",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DaemonConfig {
    pub max_request_bytes: usize,
    pub request_timeout: Duration,
    pub persistence_mode: PersistenceMode,
    pub max_concurrent_runs: usize,
    pub max_background_processes: usize,
    /// Postgres connection pool size when `persistence_mode` is `Postgres`.
    /// Should comfortably exceed `max_concurrent_runs` to leave headroom for
    /// background snapshot saves and step-log writes.
    pub db_max_connections: u32,
    /// Capacity of the in-process broadcast channels backing SSE event
    /// streams. Lagged consumers receive a synthetic gap marker rather than
    /// silent drops; this controls the burst buffer before that triggers.
    pub event_buffer: usize,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            max_request_bytes: 64 * 1024,
            request_timeout: Duration::from_secs(30),
            persistence_mode: PersistenceMode::Memory,
            max_concurrent_runs: DEFAULT_MAX_CONCURRENT_RUNS,
            max_background_processes: DEFAULT_MAX_BACKGROUND_PROCESSES,
            db_max_connections: DEFAULT_DB_MAX_CONNECTIONS,
            event_buffer: DEFAULT_EVENT_BUFFER,
        }
    }
}

/// Builds a router wired to a [`DeterministicModelAdapter`] — the in-process
/// mock model that just echoes the input.
///
/// **For tests and library embedding only.** This does NOT perform LLM calls.
/// To run a real daemon that talks to providers, use [`serve`]. See also
/// [`app_with_config`] / [`app_with_database`], which share the same caveat.
pub fn app() -> Router {
    app_with_config(DaemonConfig::default())
}

/// Builds a router with the supplied [`DaemonConfig`].
///
/// **Test/embedding helper** — wires the deterministic mock model adapter.
/// Use [`serve`] for a real daemon.
pub fn app_with_config(config: DaemonConfig) -> Router {
    let event_fanout = EventFanout::new(config.event_buffer);
    let state = Arc::new(RwLock::new(DaemonState::with_events_and_limits(
        event_fanout,
        config.max_background_processes,
    )));
    app_with_state(state, config)
}

/// Builds a router with a custom database adapter, default config, and the
/// deterministic mock model adapter.
///
/// **Test/embedding helper** — does not run LLM calls. Use [`serve`] for
/// production.
pub fn app_with_database(db: Arc<dyn DatabaseAdapter>) -> Router {
    let config = DaemonConfig::default();
    let event_fanout = EventFanout::new(config.event_buffer);
    let mut daemon_state =
        DaemonState::with_events_and_limits(event_fanout, config.max_background_processes);
    daemon_state.set_database(db);
    let state = Arc::new(RwLock::new(daemon_state));
    app_with_state(state, config)
}

pub(crate) fn app_with_state(state: SharedDaemonState, config: DaemonConfig) -> Router {
    let runtime = deterministic_daemon_runtime(Arc::clone(&state), &config);
    // Construction-time state is uncontended, so this always succeeds in
    // practice; calendar tools simply report "unconfigured" otherwise.
    if let Ok(mut guard) = state.try_write() {
        guard.set_calendar_manager(Some(runtime.calendar.clone()));
    }
    router_with_runtime(
        state,
        config,
        runtime,
        routes::configured_bind_is_loopback(),
    )
}

pub async fn app_with_configured_persistence(config: DaemonConfig) -> io::Result<Router> {
    let event_fanout = EventFanout::new(config.event_buffer);
    let state = Arc::new(RwLock::new(DaemonState::with_events_and_limits(
        event_fanout,
        config.max_background_processes,
    )));
    configure_persistence(&state, &config).await?;
    let runtime = daemon_runtime(Arc::clone(&state), &config)?;
    state
        .write()
        .await
        .set_calendar_manager(Some(runtime.calendar.clone()));
    runtime.connectors.start_restored().await;
    runtime.scheduler.start().await;
    Ok(router_with_runtime(state, config, runtime, false))
}

pub async fn serve(listener: TcpListener, config: DaemonConfig) -> io::Result<()> {
    let event_fanout = EventFanout::new(config.event_buffer);
    let state = Arc::new(RwLock::new(
        DaemonState::with_model_adapter_and_events_and_limits(
            Arc::new(RuntimeModelAdapter::from_env()),
            event_fanout,
            config.max_background_processes,
        ),
    ));

    configure_persistence(&state, &config).await?;

    serve_with_state(listener, state, config).await
}

pub(crate) async fn serve_with_state(
    listener: TcpListener,
    state: SharedDaemonState,
    config: DaemonConfig,
) -> io::Result<()> {
    let bind_is_loopback = listener.local_addr()?.ip().is_loopback();
    let runtime = daemon_runtime(Arc::clone(&state), &config)?;
    state
        .write()
        .await
        .set_calendar_manager(Some(runtime.calendar.clone()));
    runtime.connectors.start_restored().await;
    runtime.scheduler.start().await;
    let connectors = runtime.connectors.clone();
    let scheduler = runtime.scheduler.clone();
    let router = router_with_runtime(state, config, runtime, bind_is_loopback);
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            scheduler.shutdown().await;
            connectors.shutdown().await;
        })
        .await
}

fn daemon_runtime(state: SharedDaemonState, config: &DaemonConfig) -> io::Result<DaemonRuntime> {
    let run_limiter = Arc::new(Semaphore::new(config.max_concurrent_runs));
    let agent_runs = AgentRunCoordinator::new(Arc::clone(&state), Arc::clone(&run_limiter));
    let transport = TelegramClient::new()
        .map_err(|error| io::Error::new(io::ErrorKind::Other, error.to_string()))?;
    let connectors = ConnectorManager::new(
        Arc::clone(&state),
        agent_runs.clone(),
        Arc::new(OsKeyringCredentialStore::new()),
        Arc::new(transport),
    );
    let google_transport = crate::connectors::gcalendar::client::GoogleCalendarClient::new()
        .map_err(|error| io::Error::new(io::ErrorKind::Other, error.to_string()))?;
    let oauth_apps = crate::connectors::oauth_apps::OAuthAppService::new();
    let calendar = crate::connectors::gcalendar::CalendarManager::new(
        &state,
        agent_runs.clone(),
        Arc::new(crate::connectors::gcalendar::store::OsKeyringGoogleCredentialStore::new()),
        Arc::new(google_transport),
        oauth_apps.clone(),
    );
    let mail = crate::connectors::mail::MailManager::new(
        &state,
        agent_runs.clone(),
        Arc::new(crate::connectors::gcalendar::store::OsKeyringGoogleCredentialStore::new()),
        Arc::new(crate::connectors::mail::client::MailClient::new()),
        oauth_apps.clone(),
    );
    let scheduler = SchedulerService::new(state, agent_runs.clone(), connectors.clone());
    Ok(DaemonRuntime {
        run_limiter,
        agent_runs,
        connectors,
        calendar,
        mail,
        oauth_apps,
        scheduler,
    })
}

fn deterministic_daemon_runtime(state: SharedDaemonState, config: &DaemonConfig) -> DaemonRuntime {
    deterministic_daemon_runtime_with_mail_transport(
        state,
        config,
        Arc::new(DeterministicMailTransport),
    )
}

fn deterministic_daemon_runtime_with_mail_transport(
    state: SharedDaemonState,
    config: &DaemonConfig,
    mail_transport: Arc<dyn crate::connectors::mail::client::MailTransport>,
) -> DaemonRuntime {
    let run_limiter = Arc::new(Semaphore::new(config.max_concurrent_runs));
    let agent_runs = AgentRunCoordinator::new(Arc::clone(&state), Arc::clone(&run_limiter));
    let connectors = ConnectorManager::new(
        Arc::clone(&state),
        agent_runs.clone(),
        Arc::new(InMemoryCredentialStore::default()),
        Arc::new(DeterministicTelegramTransport),
    );
    let oauth_apps = crate::connectors::oauth_apps::OAuthAppService::in_memory();
    let calendar = crate::connectors::gcalendar::CalendarManager::new(
        &state,
        agent_runs.clone(),
        Arc::new(crate::connectors::gcalendar::store::InMemoryGoogleCredentialStore::default()),
        Arc::new(crate::connectors::gcalendar::client::UnconfiguredGoogleTransport),
        oauth_apps.clone(),
    );
    let mail = crate::connectors::mail::MailManager::new(
        &state,
        agent_runs.clone(),
        Arc::new(crate::connectors::gcalendar::store::InMemoryGoogleCredentialStore::default()),
        mail_transport,
        oauth_apps.clone(),
    );
    let scheduler = SchedulerService::new(state, agent_runs.clone(), connectors.clone());
    DaemonRuntime {
        run_limiter,
        agent_runs,
        connectors,
        calendar,
        mail,
        oauth_apps,
        scheduler,
    }
}

fn router_with_runtime(
    state: SharedDaemonState,
    config: DaemonConfig,
    runtime: DaemonRuntime,
    bind_is_loopback: bool,
) -> Router {
    routes::router_with_all_services(
        state,
        config,
        runtime.run_limiter,
        runtime.agent_runs,
        runtime.connectors,
        runtime.calendar,
        runtime.mail,
        runtime.oauth_apps,
        runtime.scheduler,
        bind_is_loopback,
    )
}

/// Deterministic Telegram boundary paired with the deterministic model in the
/// public test/embedding router helpers. Production `serve` always uses the
/// fixed-origin `TelegramClient` and OS credential vault.
struct DeterministicTelegramTransport;

pub(crate) struct DeterministicMailTransport;

#[async_trait]
impl crate::connectors::mail::client::MailTransport for DeterministicMailTransport {
    async fn exchange(
        &self,
        _config: &crate::connectors::mail::OAuthConfig,
        _code: &str,
        _verifier: &str,
    ) -> Result<
        crate::connectors::gcalendar::store::GoogleOAuthTokens,
        crate::connectors::mail::MailError,
    > {
        Err(crate::connectors::mail::MailError::Unconfigured)
    }

    async fn refresh(
        &self,
        _config: &crate::connectors::mail::OAuthConfig,
        _refresh: &str,
    ) -> Result<
        crate::connectors::gcalendar::store::GoogleOAuthTokens,
        crate::connectors::mail::MailError,
    > {
        Err(crate::connectors::mail::MailError::Unconfigured)
    }

    async fn account(
        &self,
        _provider: crate::connectors::mail::Provider,
        _access: &str,
    ) -> Result<String, crate::connectors::mail::MailError> {
        Err(crate::connectors::mail::MailError::Unconfigured)
    }

    async fn inbox(
        &self,
        _provider: crate::connectors::mail::Provider,
        _access: &str,
    ) -> Result<Vec<crate::connectors::mail::MailMessage>, crate::connectors::mail::MailError> {
        Err(crate::connectors::mail::MailError::Unconfigured)
    }

    async fn send(
        &self,
        _provider: crate::connectors::mail::Provider,
        _access: &str,
        _draft: &crate::connectors::mail::MailDraft,
    ) -> Result<(), crate::connectors::mail::MailError> {
        Err(crate::connectors::mail::MailError::Unconfigured)
    }
}

#[async_trait]
impl TelegramTransport for DeterministicTelegramTransport {
    async fn get_me(
        &self,
        _token: &TelegramBotToken,
    ) -> Result<TelegramBotIdentity, TelegramTransportError> {
        Ok(TelegramBotIdentity {
            id: "900001".to_string(),
            username: Some("anima_test_bot".to_string()),
            display_name: Some("Anima Test Bot".to_string()),
        })
    }

    async fn get_updates(
        &self,
        _token: &TelegramBotToken,
        offset: i64,
    ) -> Result<TelegramUpdateBatch, TelegramTransportError> {
        if offset <= 1 {
            return Ok(TelegramUpdateBatch {
                updates: vec![crate::connectors::telegram::TelegramTextUpdate {
                    update_id: 1,
                    text: "pair deterministic Telegram chat".to_string(),
                    sender: TelegramSenderMetadata {
                        id: "700001".to_string(),
                        username: Some("local_owner".to_string()),
                        display_name: Some("Local Owner".to_string()),
                    },
                    chat: deterministic_chat(),
                }],
                next_update_id: 2,
            });
        }
        Ok(TelegramUpdateBatch {
            updates: Vec::new(),
            next_update_id: offset,
        })
    }

    async fn send_message(
        &self,
        _token: &TelegramBotToken,
        _chat_id: &str,
        _text: &str,
    ) -> Result<Vec<TelegramSentMessage>, TelegramTransportError> {
        Ok(vec![TelegramSentMessage {
            message_id: "800001".to_string(),
            chat: deterministic_chat(),
        }])
    }
}

fn deterministic_chat() -> TelegramChatMetadata {
    TelegramChatMetadata {
        id: "424242".to_string(),
        kind: TelegramChatKind::Private,
        title: None,
        username: Some("local_owner".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectors::gcalendar::store::GoogleOAuthTokens;
    use crate::connectors::mail::client::MailTransport;
    use crate::connectors::mail::{MailDraft, MailError, MailMessage, OAuthConfig, Provider};
    use crate::connectors::oauth_apps::{OAuthAppCredentials, OAuthProvider};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use zeroize::Zeroizing;

    struct CountingMailTransport(Arc<AtomicUsize>);

    #[async_trait]
    impl MailTransport for CountingMailTransport {
        async fn exchange(
            &self,
            _config: &OAuthConfig,
            _code: &str,
            _verifier: &str,
        ) -> Result<GoogleOAuthTokens, MailError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(GoogleOAuthTokens::new(
                Zeroizing::new("access".to_string()),
                Zeroizing::new("refresh".to_string()),
                crate::connectors::gcalendar::now_ms() + 3_600_000,
            ))
        }

        async fn refresh(
            &self,
            _config: &OAuthConfig,
            _refresh: &str,
        ) -> Result<GoogleOAuthTokens, MailError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(MailError::Unconfigured)
        }

        async fn account(&self, _provider: Provider, _access: &str) -> Result<String, MailError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok("owner@example.com".to_string())
        }

        async fn inbox(
            &self,
            _provider: Provider,
            _access: &str,
        ) -> Result<Vec<MailMessage>, MailError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(MailError::Unconfigured)
        }

        async fn send(
            &self,
            _provider: Provider,
            _access: &str,
            _draft: &MailDraft,
        ) -> Result<(), MailError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(MailError::Unconfigured)
        }
    }

    #[tokio::test]
    async fn deterministic_runtime_routes_mail_through_its_injected_boundary() {
        let state = Arc::new(RwLock::new(DaemonState::new()));
        let agent_id = state
            .write()
            .await
            .create_agent(anima_core::AgentConfig {
                name: "mail-runtime".to_string(),
                model: "deterministic".to_string(),
                provider: None,
                bio: None,
                lore: None,
                knowledge: None,
                topics: None,
                adjectives: None,
                style: None,
                system: None,
                tools: None,
                plugins: None,
                settings: None,
            })
            .unwrap()
            .state
            .id;
        let calls = Arc::new(AtomicUsize::new(0));
        let runtime = deterministic_daemon_runtime_with_mail_transport(
            Arc::clone(&state),
            &DaemonConfig::default(),
            Arc::new(CountingMailTransport(Arc::clone(&calls))),
        );
        runtime
            .oauth_apps
            .put(
                OAuthProvider::Google,
                OAuthAppCredentials::new(OAuthProvider::Google, "client", "secret", None).unwrap(),
            )
            .await
            .unwrap();

        let (_, consent_url) = runtime
            .mail
            .begin_connect(&agent_id, Provider::Gmail)
            .await
            .unwrap();
        let nonce = reqwest::Url::parse(&consent_url)
            .unwrap()
            .query_pairs()
            .find(|(key, _)| key == "state")
            .unwrap()
            .1
            .into_owned();
        runtime
            .mail
            .complete_connect(Provider::Gmail, &nonce, "code")
            .await
            .unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
