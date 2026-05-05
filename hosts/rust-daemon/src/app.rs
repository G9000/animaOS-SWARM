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
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            max_request_bytes: 64 * 1024,
            request_timeout: Duration::from_secs(30),
            persistence_mode: PersistenceMode::Memory,
            max_concurrent_runs: DEFAULT_MAX_CONCURRENT_RUNS,
            max_background_processes: DEFAULT_MAX_BACKGROUND_PROCESSES,
        }
    }
}

pub fn app() -> Router {
    app_with_config(DaemonConfig::default())
}

pub fn app_with_config(config: DaemonConfig) -> Router {
    let event_fanout = EventFanout::new(DEFAULT_EVENT_BUFFER);
    let state = Arc::new(RwLock::new(DaemonState::with_events_and_limits(
        event_fanout,
        config.max_background_processes,
    )));
    app_with_state(state, config)
}

pub fn app_with_database(db: Arc<dyn DatabaseAdapter>) -> Router {
    let event_fanout = EventFanout::new(DEFAULT_EVENT_BUFFER);
    let mut daemon_state =
        DaemonState::with_events_and_limits(event_fanout, DEFAULT_MAX_BACKGROUND_PROCESSES);
    daemon_state.set_database(db);
    let state = Arc::new(RwLock::new(daemon_state));
    app_with_state(state, DaemonConfig::default())
}

pub(crate) fn app_with_state(state: SharedDaemonState, config: DaemonConfig) -> Router {
    routes::router(state, config)
}

pub async fn app_with_configured_persistence(config: DaemonConfig) -> io::Result<Router> {
    let event_fanout = EventFanout::new(DEFAULT_EVENT_BUFFER);
    let state = Arc::new(RwLock::new(DaemonState::with_events_and_limits(
        event_fanout,
        config.max_background_processes,
    )));
    configure_persistence(&state, config.persistence_mode).await?;
    Ok(app_with_state(state, config))
}

pub async fn serve(listener: TcpListener, config: DaemonConfig) -> io::Result<()> {
    let event_fanout = EventFanout::new(DEFAULT_EVENT_BUFFER);
    let state = Arc::new(RwLock::new(
        DaemonState::with_model_adapter_and_events_and_limits(
            Arc::new(RuntimeModelAdapter::from_env()),
            event_fanout,
            config.max_background_processes,
        ),
    ));

    configure_persistence(&state, config.persistence_mode).await?;

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
