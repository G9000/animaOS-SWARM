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
    let calendar = crate::connectors::gcalendar::CalendarManager::new(
        &state,
        agent_runs.clone(),
        Arc::new(crate::connectors::gcalendar::store::OsKeyringGoogleCredentialStore::new()),
        Arc::new(google_transport),
        crate::connectors::gcalendar::GoogleOAuthConfig::from_env(),
    );
    let scheduler = SchedulerService::new(state, agent_runs.clone(), connectors.clone());
    Ok(DaemonRuntime {
        run_limiter,
        agent_runs,
        connectors,
        calendar,
        scheduler,
    })
}

fn deterministic_daemon_runtime(state: SharedDaemonState, config: &DaemonConfig) -> DaemonRuntime {
    let run_limiter = Arc::new(Semaphore::new(config.max_concurrent_runs));
    let agent_runs = AgentRunCoordinator::new(Arc::clone(&state), Arc::clone(&run_limiter));
    let connectors = ConnectorManager::new(
        Arc::clone(&state),
        agent_runs.clone(),
        Arc::new(InMemoryCredentialStore::default()),
        Arc::new(DeterministicTelegramTransport),
    );
    let calendar = crate::connectors::gcalendar::CalendarManager::new(
        &state,
        agent_runs.clone(),
        Arc::new(crate::connectors::gcalendar::store::InMemoryGoogleCredentialStore::default()),
        Arc::new(crate::connectors::gcalendar::client::UnconfiguredGoogleTransport),
        None,
    );
    let scheduler = SchedulerService::new(state, agent_runs.clone(), connectors.clone());
    DaemonRuntime {
        run_limiter,
        agent_runs,
        connectors,
        calendar,
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
        runtime.scheduler,
        bind_is_loopback,
    )
}

/// Deterministic Telegram boundary paired with the deterministic model in the
/// public test/embedding router helpers. Production `serve` always uses the
/// fixed-origin `TelegramClient` and OS credential vault.
struct DeterministicTelegramTransport;

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
