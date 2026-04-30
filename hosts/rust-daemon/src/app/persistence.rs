use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use anima_memory::MemoryManager;
use sqlx::postgres::PgPoolOptions;
use tracing::{info, warn};

use super::{PersistenceMode, SharedDaemonState};
use crate::postgres::SqlxPostgresAdapter;

pub(super) async fn configure_persistence(
    state: &SharedDaemonState,
    persistence_mode: PersistenceMode,
) -> io::Result<()> {
    configure_memory_file(state).await?;

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

async fn configure_memory_file(state: &SharedDaemonState) -> io::Result<()> {
    let Some(path) = memory_file_from_env()? else {
        return Ok(());
    };

    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }

    let mut manager = MemoryManager::with_storage_file(path.clone());
    manager.load()?;
    let loaded_count = manager.size();
    state.write().await.replace_memory(manager);

    info!(
        memory_file = %path.display(),
        loaded_count,
        "runtime memory file configured"
    );
    Ok(())
}

fn memory_file_from_env() -> io::Result<Option<PathBuf>> {
    let Some(value) = std::env::var_os("ANIMAOS_RS_MEMORY_FILE") else {
        return Ok(None);
    };
    if value.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "ANIMAOS_RS_MEMORY_FILE must not be empty",
        ));
    }
    Ok(Some(PathBuf::from(value)))
}
