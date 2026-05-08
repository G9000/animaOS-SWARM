use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use anima_memory::{MemoryManager, RecentMemoryOptions};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tracing::{info, warn};

use super::{DaemonConfig, PersistenceMode, SharedDaemonState};
use crate::control_plane_store::{load_control_plane_snapshot, ControlPlaneStoreConfig};
use crate::memory_embeddings::MemoryEmbeddingRuntime;
use crate::memory_store::{load_memory_snapshot, MemoryStoreConfig};
use crate::postgres::SqlxPostgresAdapter;
use crate::state::{memory_query_expander_from_env, memory_text_analyzer_from_env};

pub(crate) async fn configure_persistence(
    state: &SharedDaemonState,
    config: &DaemonConfig,
) -> io::Result<()> {
    let postgres_pool = configure_database(state, config).await?;
    let memory_store = memory_store_from_env(postgres_pool.as_ref())?;
    let control_plane_store = control_plane_store_from_env(postgres_pool.as_ref())?;

    let default_embedding_store = configure_memory_store(state, memory_store).await?;
    configure_memory_embeddings(state, default_embedding_store).await?;
    configure_control_plane_store(state, control_plane_store).await?;
    Ok(())
}

async fn configure_database(
    state: &SharedDaemonState,
    config: &DaemonConfig,
) -> io::Result<Option<PgPool>> {
    match config.persistence_mode {
        PersistenceMode::Memory => {
            if std::env::var_os("DATABASE_URL").is_some() {
                warn!(
                    "DATABASE_URL is set but ANIMAOS_RS_PERSISTENCE_MODE=memory; starting without Postgres persistence"
                );
            } else {
                info!("starting in memory persistence mode");
            }
            Ok(None)
        }
        PersistenceMode::Postgres => {
            let database_url = std::env::var("DATABASE_URL").map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "DATABASE_URL must be set when ANIMAOS_RS_PERSISTENCE_MODE=postgres",
                )
            })?;
            let pool = PgPoolOptions::new()
                .max_connections(config.db_max_connections)
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

            let adapter = Arc::new(SqlxPostgresAdapter::new(pool.clone()));
            state.write().await.set_database(adapter);
            info!("Postgres connected, migrations applied");
            Ok(Some(pool))
        }
    }
}

async fn configure_control_plane_store(
    state: &SharedDaemonState,
    config: Option<ControlPlaneStoreConfig>,
) -> io::Result<()> {
    let Some(config) = config else {
        state.write().await.set_control_plane_store(None);
        return Ok(());
    };

    if let Some(path) = config.file_path() {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            tokio::fs::create_dir_all(parent).await?;
        }
    }

    let snapshot = load_control_plane_snapshot(&config).await?;
    let (restored_agents, restored_swarms) = if let Some(snapshot) = snapshot {
        state
            .write()
            .await
            .restore_control_plane_snapshot(snapshot)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
    } else {
        (0, 0)
    };

    {
        let mut guard = state.write().await;
        guard.set_control_plane_store(Some(config.clone()));
        guard.control_plane_persist_request()
    }
    .save()
    .await?;

    info!(
        control_plane_store = %config.location_label(),
        storage = config.storage_label(),
        restored_agents,
        restored_swarms,
        "runtime control plane store configured"
    );
    Ok(())
}

async fn configure_memory_store(
    state: &SharedDaemonState,
    config: Option<MemoryStoreConfig>,
) -> io::Result<Option<PathBuf>> {
    let Some(config) = config else {
        state.write().await.set_memory_store(None);
        return Ok(None);
    };

    if let Some(path) = config.file_path() {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            tokio::fs::create_dir_all(parent).await?;
        }
    }

    let query_expander = memory_query_expander_from_env();
    let text_analyzer = memory_text_analyzer_from_env();
    let mut manager = match query_expander {
        Some(query_expander) => {
            MemoryManager::with_text_analyzer_and_query_expander(text_analyzer, query_expander)
        }
        None => MemoryManager::with_text_analyzer(text_analyzer),
    };
    if let Some(snapshot) = load_memory_snapshot(&config).await? {
        manager.replace_snapshot(snapshot);
    }
    let loaded_count = manager.size();
    {
        let mut guard = state.write().await;
        guard.replace_memory(manager);
        guard.set_memory_store(Some(config.clone()));
    }

    info!(
        memory_store = %config.location_label(),
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

fn memory_store_from_env(postgres_pool: Option<&PgPool>) -> io::Result<Option<MemoryStoreConfig>> {
    let json = non_empty_env_path("ANIMAOS_RS_MEMORY_FILE")?;
    let sqlite = non_empty_env_path("ANIMAOS_RS_MEMORY_SQLITE_FILE")?;

    match (json, sqlite) {
        (Some(_), Some(_)) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "set only one of ANIMAOS_RS_MEMORY_FILE or ANIMAOS_RS_MEMORY_SQLITE_FILE",
        )),
        (Some(path), None) => Ok(Some(MemoryStoreConfig::Json(path))),
        (None, Some(path)) => Ok(Some(MemoryStoreConfig::Sqlite(path))),
        (None, None) => Ok(postgres_pool.cloned().map(MemoryStoreConfig::Postgres)),
    }
}

fn control_plane_store_from_env(
    postgres_pool: Option<&PgPool>,
) -> io::Result<Option<ControlPlaneStoreConfig>> {
    let Some(path) = non_empty_env_path("ANIMAOS_RS_CONTROL_PLANE_FILE")? else {
        return Ok(postgres_pool
            .cloned()
            .map(ControlPlaneStoreConfig::Postgres));
    };
    Ok(Some(ControlPlaneStoreConfig::Json(path)))
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
