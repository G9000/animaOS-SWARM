use std::io;
use std::sync::Arc;

use sqlx::postgres::PgPoolOptions;
use tracing::{info, warn};

use super::{PersistenceMode, SharedDaemonState};
use crate::postgres::SqlxPostgresAdapter;

pub(super) async fn configure_persistence(
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