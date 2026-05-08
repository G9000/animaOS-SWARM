pub(crate) mod lifecycle;
pub(crate) mod persistence;

use std::io;
use std::sync::Arc;
use std::time::Duration;

use anima_core::DatabaseAdapter;
use axum::Router;
use tokio::net::TcpListener;
use tokio::sync::RwLock;

use self::{lifecycle::shutdown_signal, persistence::configure_persistence};
use crate::events::{EventFanout, DEFAULT_EVENT_BUFFER};
use crate::routes;
use crate::runtime_model::RuntimeModelAdapter;
use crate::state::DaemonState;
use crate::tools::DEFAULT_MAX_BACKGROUND_PROCESSES;

pub(crate) type SharedDaemonState = Arc<RwLock<DaemonState>>;

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
    routes::router(state, config)
}

pub async fn app_with_configured_persistence(config: DaemonConfig) -> io::Result<Router> {
    let event_fanout = EventFanout::new(config.event_buffer);
    let state = Arc::new(RwLock::new(DaemonState::with_events_and_limits(
        event_fanout,
        config.max_background_processes,
    )));
    configure_persistence(&state, &config).await?;
    Ok(app_with_state(state, config))
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
    axum::serve(listener, app_with_state(state, config))
        .with_graceful_shutdown(shutdown_signal())
        .await
}
