use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use anima_memory::{MemoryManager, RecentMemoryOptions};
use sqlx::postgres::PgPoolOptions;
use tracing::{info, warn};

use super::{PersistenceMode, SharedDaemonState};
use crate::memory_embeddings::MemoryEmbeddingRuntime;
use crate::postgres::SqlxPostgresAdapter;

pub(super) async fn configure_persistence(
    state: &SharedDaemonState,
    persistence_mode: PersistenceMode,
) -> io::Result<()> {
    let default_embedding_store = configure_memory_store(state).await?;
    configure_memory_embeddings(state, default_embedding_store).await?;

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

async fn configure_memory_store(state: &SharedDaemonState) -> io::Result<Option<PathBuf>> {
    let Some(config) = memory_store_from_env()? else {
        return Ok(None);
    };
    let path = config.path();

    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }

    let mut manager = match &config {
        MemoryStoreConfig::Json(path) => MemoryManager::with_storage_file(path.clone()),
        MemoryStoreConfig::Sqlite(path) => MemoryManager::with_sqlite_file(path.clone()),
    };
    manager.load()?;
    let loaded_count = manager.size();
    state.write().await.replace_memory(manager);

    info!(
        memory_file = %path.display(),
        storage = config.storage_label(),
        loaded_count,
        "runtime memory store configured"
    );
    Ok(config.embedding_store_default())
}

async fn configure_memory_embeddings(
    state: &SharedDaemonState,
    default_sqlite_path: Option<PathBuf>,
) -> io::Result<()> {
    let mut embeddings = MemoryEmbeddingRuntime::from_env(default_sqlite_path)?;
    let memories = {
        let memory = { state.read().await.memory_handle() };
        let memory_guard = memory.read().await;
        memory_guard.get_recent(RecentMemoryOptions {
            limit: Some(usize::MAX),
            ..RecentMemoryOptions::default()
        })
    };
    let report = embeddings
        .rebuild_from_memories(&memories)
        .map_err(|error| io::Error::new(io::ErrorKind::Other, error))?;
    let status = embeddings.status();
    state.write().await.replace_memory_embeddings(embeddings);

    info!(
        enabled = status.enabled,
        provider = %status.provider,
        model = %status.model,
        dimension = status.dimension,
        vector_count = status.vector_count,
        persisted = status.persisted,
        loaded_vectors = report.loaded_vectors,
        rebuilt_vectors = report.rebuilt_vectors,
        removed_stale_vectors = report.removed_stale_vectors,
        "runtime memory embeddings configured"
    );
    Ok(())
}

enum MemoryStoreConfig {
    Json(PathBuf),
    Sqlite(PathBuf),
}

impl MemoryStoreConfig {
    fn path(&self) -> &PathBuf {
        match self {
            Self::Json(path) | Self::Sqlite(path) => path,
        }
    }

    const fn storage_label(&self) -> &'static str {
        match self {
            Self::Json(_) => "json",
            Self::Sqlite(_) => "sqlite",
        }
    }

    fn embedding_store_default(&self) -> Option<PathBuf> {
        match self {
            Self::Json(_) => None,
            Self::Sqlite(path) => Some(path.clone()),
        }
    }
}

fn memory_store_from_env() -> io::Result<Option<MemoryStoreConfig>> {
    let json = non_empty_env_path("ANIMAOS_RS_MEMORY_FILE")?;
    let sqlite = non_empty_env_path("ANIMAOS_RS_MEMORY_SQLITE_FILE")?;

    match (json, sqlite) {
        (Some(_), Some(_)) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "set only one of ANIMAOS_RS_MEMORY_FILE or ANIMAOS_RS_MEMORY_SQLITE_FILE",
        )),
        (Some(path), None) => Ok(Some(MemoryStoreConfig::Json(path))),
        (None, Some(path)) => Ok(Some(MemoryStoreConfig::Sqlite(path))),
        (None, None) => Ok(None),
    }
}

fn non_empty_env_path(name: &'static str) -> io::Result<Option<PathBuf>> {
    let Some(value) = std::env::var_os(name) else {
        return Ok(None);
    };
    if value.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name} must not be empty"),
        ));
    }
    Ok(Some(PathBuf::from(value)))
}
