use std::io;
use std::sync::{Arc, Mutex};

use axum::Router;
use tokio::net::TcpListener;

use crate::events::{EventFanout, DEFAULT_EVENT_BUFFER};
use crate::routes;
use crate::state::DaemonState;

pub(crate) type SharedDaemonState = Arc<Mutex<DaemonState>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DaemonConfig {
    pub max_request_bytes: usize,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            max_request_bytes: 64 * 1024,
        }
    }
}

pub fn app() -> Router {
    app_with_config(DaemonConfig::default())
}

pub fn app_with_config(config: DaemonConfig) -> Router {
    let event_fanout = EventFanout::new(DEFAULT_EVENT_BUFFER);
    app_with_state(
        Arc::new(Mutex::new(DaemonState::with_events(event_fanout))),
        config,
    )
}

pub(crate) fn app_with_state(state: SharedDaemonState, config: DaemonConfig) -> Router {
    routes::router(state, config)
}

pub async fn serve(listener: TcpListener, config: DaemonConfig) -> io::Result<()> {
    let event_fanout = EventFanout::new(DEFAULT_EVENT_BUFFER);
    serve_with_state(
        listener,
        Arc::new(Mutex::new(DaemonState::with_events(event_fanout))),
        config,
    )
    .await
}

pub(crate) async fn serve_with_state(
    listener: TcpListener,
    state: SharedDaemonState,
    config: DaemonConfig,
) -> io::Result<()> {
    axum::serve(listener, app_with_state(state, config)).await
}
