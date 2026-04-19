use std::io;
use std::sync::{Arc, Mutex};

use axum::Router;
use tokio::net::TcpListener;

use crate::events::{EventFanout, DEFAULT_EVENT_BUFFER};
use crate::postgres::SqlxPostgresAdapter;
use crate::routes;
use crate::runtime_model::RuntimeModelAdapter;
use crate::state::DaemonState;
use sqlx::postgres::PgPoolOptions;

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
    let state = Arc::new(Mutex::new(DaemonState::with_model_adapter_and_events(
        Arc::new(RuntimeModelAdapter::from_env()),
        event_fanout,
    )));

    if let Ok(database_url) = std::env::var("DATABASE_URL") {
        match PgPoolOptions::new()
            .max_connections(10)
            .connect(&database_url)
            .await
        {
            Ok(pool) => match sqlx::migrate!("./migrations").run(&pool).await {
                Ok(_) => {
                    let adapter = Arc::new(SqlxPostgresAdapter::new(pool));
                    state
                        .lock()
                        .expect("state mutex should not be poisoned")
                        .set_database(adapter);

                    println!("anima-daemon: Postgres connected, migrations applied");
                }
                Err(e) => {
                    eprintln!("anima-daemon: migration failed: {e} — running without persistence");
                }
            },
            Err(e) => {
                eprintln!(
                    "anima-daemon: Postgres connection failed: {e} — running without persistence"
                );
            }
        }
    } else {
        eprintln!("anima-daemon: DATABASE_URL not set — running without persistence");
    }

    serve_with_state(listener, state, config).await
}

pub(crate) async fn serve_with_state(
    listener: TcpListener,
    state: SharedDaemonState,
    config: DaemonConfig,
) -> io::Result<()> {
    axum::serve(listener, app_with_state(state, config)).await
}
