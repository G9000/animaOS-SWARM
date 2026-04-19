use std::io;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use sqlx::postgres::PgPoolOptions;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::events::{EventFanout, DEFAULT_EVENT_BUFFER};
use crate::postgres::SqlxPostgresAdapter;
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

async fn configure_persistence(
    state: &SharedDaemonState,
    persistence_mode: PersistenceMode,
) -> io::Result<()> {
    match persistence_mode {
        PersistenceMode::Memory => {
            if std::env::var_os("DATABASE_URL").is_some() {
                warn!(
                    "DATABASE_URL is set but ANIMAOS_RS_PERSISTENCE_MODE=memory; starting without Postgres persistence"
                );
            } else {
                info!("starting in memory persistence mode");
            }
            Ok(())
        }
        PersistenceMode::Postgres => {
            let database_url = std::env::var("DATABASE_URL").map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "DATABASE_URL must be set when ANIMAOS_RS_PERSISTENCE_MODE=postgres",
                )
            })?;
            let pool = PgPoolOptions::new()
                .max_connections(10)
                .connect(&database_url)
                .await
                .map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::Other,
                        format!("failed to connect to Postgres: {error}"),
                    )
                })?;

            sqlx::migrate!("./migrations")
                .run(&pool)
                .await
                .map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::Other,
                        format!("failed to run Postgres migrations: {error}"),
                    )
                })?;

            let adapter = Arc::new(SqlxPostgresAdapter::new(pool));
            state.write().await.set_database(adapter);
            info!("Postgres connected, migrations applied");
            Ok(())
        }
    }
}

async fn shutdown_signal() {
    match tokio::signal::ctrl_c().await {
        Ok(()) => info!("shutdown signal received"),
        Err(error) => warn!("failed to install shutdown signal handler: {error}"),
    }
}
