pub(crate) mod lifecycle;
pub(crate) mod persistence;

use std::io;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use tokio::net::TcpListener;
use tokio::sync::RwLock;

use self::{lifecycle::shutdown_signal, persistence::configure_persistence};
use crate::events::{EventFanout, DEFAULT_EVENT_BUFFER};
use crate::routes;
use crate::runtime_model::RuntimeModelAdapter;
use crate::state::DaemonState;

pub(crate) type SharedDaemonState = Arc<RwLock<DaemonState>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PersistenceMode {
    Memory,
    Postgres,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DaemonConfig {
    pub max_request_bytes: usize,
    pub request_timeout: Duration,
    pub persistence_mode: PersistenceMode,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            max_request_bytes: 64 * 1024,
            request_timeout: Duration::from_secs(30),
            persistence_mode: PersistenceMode::Memory,
        }
    }
}

pub fn app() -> Router {
    app_with_config(DaemonConfig::default())
}

pub fn app_with_config(config: DaemonConfig) -> Router {
    let event_fanout = EventFanout::new(DEFAULT_EVENT_BUFFER);
    app_with_state(
        Arc::new(RwLock::new(DaemonState::with_events(event_fanout))),
        config,
    )
}

pub(crate) fn app_with_state(state: SharedDaemonState, config: DaemonConfig) -> Router {
    routes::router(state, config)
}

pub async fn serve(listener: TcpListener, config: DaemonConfig) -> io::Result<()> {
    let event_fanout = EventFanout::new(DEFAULT_EVENT_BUFFER);
    let state = Arc::new(RwLock::new(DaemonState::with_model_adapter_and_events(
        Arc::new(RuntimeModelAdapter::from_env()),
        event_fanout,
    )));

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
