use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anima_memory::{MemoryManager, RecentMemoryOptions};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tracing::{info, warn};

use super::{DaemonConfig, PersistenceMode, SharedDaemonState};
use crate::control_plane_store::{
    load_control_plane_snapshot, write_pre_upgrade_backup, ControlPlaneStoreConfig,
    CONTROL_PLANE_STORE_VERSION,
};
use crate::history::{
    HistoryService, HistoryStore, MemoryHistoryStore, PostgresHistoryStore, SqliteHistoryStore,
};
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
    let history_sqlite = non_empty_env_path(HISTORY_SQLITE_FILE_ENV)?;

    let default_embedding_store = configure_memory_store(state, memory_store).await?;
    configure_memory_embeddings(state, default_embedding_store).await?;
    configure_control_plane_and_history(state, control_plane_store, history_sqlite).await
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
    if let Some(loaded) = &snapshot {
        if loaded.version < CONTROL_PLANE_STORE_VERSION {
            // Spec §13.3 step 1: back up the untouched snapshot before anything
            // is written in the new version.
            let backup = write_pre_upgrade_backup(&config, loaded.version).await?;
            info!(
                backup = %backup,
                from_version = loaded.version,
                to_version = CONTROL_PLANE_STORE_VERSION,
                "saved the pre-upgrade control-plane backup"
            );
        }
    }
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
        // Spec §13.3 step 5: tools added since these agents were created.
        let granted = guard.apply_pending_tool_grants(crate::sessions::migration::TOOL_GRANTS);
        if !granted.is_empty() {
            info!(
                agents = granted.len(),
                "granted newly added tools to existing agents"
            );
        }
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

/// The SQLite history file (spec §13.1); defaults beside the control plane.
pub(crate) const HISTORY_SQLITE_FILE_ENV: &str = "ANIMAOS_RS_HISTORY_SQLITE_FILE";

/// Opens the history store before the control plane is loaded or saved: a
/// history file that cannot be opened refuses boot while the snapshot is still
/// untouched, so an older daemon can start on it (spec §16).
async fn configure_control_plane_and_history(
    state: &SharedDaemonState,
    control_plane: Option<ControlPlaneStoreConfig>,
    history_sqlite: Option<PathBuf>,
) -> io::Result<()> {
    let store = history_store_for(control_plane.as_ref(), history_sqlite).await?;
    configure_control_plane_store(state, control_plane).await?;
    let label = store.label();
    state.write().await.set_history(HistoryService::new(store));
    info!(history_store = label, "runtime history store configured");
    Ok(())
}

/// The history store that goes with the control-plane store: SQLite beside a
/// JSON control plane (or at the explicit path), Postgres tables in Postgres
/// mode, and bounded memory tables in ephemeral mode.
pub(crate) async fn history_store_for(
    control_plane: Option<&ControlPlaneStoreConfig>,
    sqlite_override: Option<PathBuf>,
) -> io::Result<Arc<dyn HistoryStore>> {
    let sqlite_path = match (control_plane, sqlite_override) {
        (None, Some(_)) => {
            warn!("ANIMAOS_RS_HISTORY_SQLITE_FILE is ignored without a durable control plane; history stays in memory");
            return Ok(Arc::new(MemoryHistoryStore::new()));
        }
        (None, None) => return Ok(Arc::new(MemoryHistoryStore::new())),
        (Some(_), Some(path)) => path,
        (Some(ControlPlaneStoreConfig::Json(file)), None) => default_history_sqlite_path(file),
        (Some(ControlPlaneStoreConfig::Postgres(pool)), None) => {
            return Ok(Arc::new(PostgresHistoryStore::new(pool.clone())))
        }
    };
    let store = SqliteHistoryStore::open(sqlite_path)
        .await
        .map_err(|error| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("failed to open the history store: {error}"),
            )
        })?;
    Ok(Arc::new(store))
}

pub(crate) fn default_history_sqlite_path(control_plane_file: &Path) -> PathBuf {
    control_plane_file.with_file_name("history.sqlite")
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "anima-persistence-{label}-{}",
            uuid::Uuid::new_v4()
        ))
    }

    #[test]
    fn the_default_history_file_sits_beside_the_control_plane_file() {
        assert_eq!(
            default_history_sqlite_path(Path::new("/data/control-plane.json")),
            PathBuf::from("/data/history.sqlite")
        );
        assert_eq!(
            default_history_sqlite_path(Path::new("control-plane.json")),
            PathBuf::from("history.sqlite")
        );
    }

    #[tokio::test]
    async fn the_history_store_follows_the_control_plane_store() {
        let dir = temp_dir("history");
        let control = ControlPlaneStoreConfig::Json(dir.join("control-plane.json"));

        let beside = history_store_for(Some(&control), None).await.unwrap();
        assert_eq!(beside.label(), "sqlite");
        assert!(dir.join("history.sqlite").exists());
        let custom = dir.join("custom").join("history.db");
        let chosen = history_store_for(Some(&control), Some(custom.clone()))
            .await
            .unwrap();
        assert_eq!(chosen.label(), "sqlite");
        assert!(custom.exists());
        let ephemeral = history_store_for(None, Some(custom)).await.unwrap();
        assert!(
            ephemeral.is_ephemeral(),
            "without a durable control plane history stays in memory"
        );
        assert_eq!(
            history_store_for(None, None).await.unwrap().label(),
            "memory"
        );
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://anima@127.0.0.1:1/anima")
            .unwrap();
        assert_eq!(
            history_store_for(Some(&ControlPlaneStoreConfig::Postgres(pool)), None)
                .await
                .unwrap()
                .label(),
            "postgres"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_history_file_that_cannot_open_refuses_boot_before_the_snapshot_is_touched() {
        let dir = temp_dir("unopenable");
        std::fs::create_dir_all(&dir).unwrap();
        let control_file = dir.join("control-plane.json");
        let original = r#"{"version":4,"agents":[],"swarms":[]}"#;
        std::fs::write(&control_file, original).unwrap();
        // A regular file where the history file's directory should be.
        let not_a_directory = dir.join("not-a-directory");
        std::fs::write(&not_a_directory, "").unwrap();
        let state = Arc::new(tokio::sync::RwLock::new(crate::state::DaemonState::new()));

        let error = configure_control_plane_and_history(
            &state,
            Some(ControlPlaneStoreConfig::Json(control_file.clone())),
            Some(not_a_directory.join("history.sqlite")),
        )
        .await
        .expect_err("an unopenable history store refuses boot");

        assert!(
            error
                .to_string()
                .starts_with("failed to open the history store"),
            "{error}"
        );
        assert_eq!(
            std::fs::read_to_string(&control_file).unwrap(),
            original,
            "an older daemon can still start on the untouched snapshot"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_saved_history_deletion_survives_a_restart_and_is_replayed_at_boot() {
        use crate::agent_runs::{AgentRunCoordinator, AgentRunRequest, RunRoom};
        use crate::history::{HistoryDeletion, MessagePageQuery};
        use crate::runs::RunSource;
        use anima_core::primitives::now_millis;

        let dir = temp_dir("deletion-restart");
        let control = ControlPlaneStoreConfig::Json(dir.join("control-plane.json"));
        let page = |agent_id: &str, session_id: &str| MessagePageQuery {
            agent_id: agent_id.into(),
            session_id: session_id.into(),
            before: None,
            limit: 10,
            include_hidden: true,
        };
        let turn = |agent_id: &str, room: &str| AgentRunRequest {
            agent_id: agent_id.into(),
            content: anima_core::Content {
                text: "remember this".into(),
                ..anima_core::Content::default()
            },
            room: RunRoom::Stable(room.into()),
            idempotency_key: None,
            source: RunSource::Api,
            source_ref: None,
        };

        // First boot: two mirrored chats, one of them deleted with its history
        // deletion saved, and the daemon stops before the store applies it.
        let first = Arc::new(tokio::sync::RwLock::new(crate::state::DaemonState::new()));
        configure_control_plane_and_history(&first, Some(control.clone()), None)
            .await
            .unwrap();
        let agent_id = first
            .write()
            .await
            .create_agent(anima_core::AgentConfig {
                name: "historian".into(),
                model: "deterministic".into(),
                bio: None,
                lore: None,
                knowledge: None,
                topics: None,
                adjectives: None,
                style: None,
                provider: None,
                system: None,
                tools: None,
                plugins: None,
                settings: None,
            })
            .unwrap()
            .state
            .id;
        let coordinator =
            AgentRunCoordinator::new(Arc::clone(&first), Arc::new(tokio::sync::Semaphore::new(1)));
        for room in ["chat:doomed", "chat:kept"] {
            coordinator.run(turn(&agent_id, room)).await.unwrap();
        }
        let history = first.read().await.history.clone();
        history
            .flush_once(
                &first,
                &coordinator.control_plane_transactions(),
                now_millis(),
            )
            .await
            .unwrap();
        assert_eq!(
            history
                .store()
                .page_messages(&page(&agent_id, "chat:doomed"))
                .await
                .unwrap()
                .len(),
            2
        );
        {
            let _transaction = coordinator.control_plane_transaction().await;
            let persist = {
                let mut guard = first.write().await;
                guard
                    .agents
                    .get_mut(&agent_id)
                    .unwrap()
                    .retain_messages(|message| message.room_id != "chat:doomed");
                guard.record_history_deletion(HistoryDeletion::session(&agent_id, "chat:doomed"));
                guard.control_plane_persist_request()
            };
            persist.save().await.unwrap();
        }
        drop((coordinator, history, first));

        // Second boot over the same files replays the deletion.
        let second = Arc::new(tokio::sync::RwLock::new(crate::state::DaemonState::new()));
        configure_control_plane_and_history(&second, Some(control.clone()), None)
            .await
            .unwrap();
        assert_eq!(
            second.read().await.pending_history_deletions.len(),
            1,
            "the deletion was saved with the session's removal"
        );
        let history = second.read().await.history.clone();
        let report = history
            .flush_once(&second, &tokio::sync::Mutex::new(()), now_millis())
            .await
            .unwrap();

        assert_eq!(report.deletions, 1);
        let store = history.store();
        assert!(
            store
                .page_messages(&page(&agent_id, "chat:doomed"))
                .await
                .unwrap()
                .is_empty(),
            "the replayed deletion removed the rows"
        );
        assert_eq!(
            store
                .page_messages(&page(&agent_id, "chat:kept"))
                .await
                .unwrap()
                .len(),
            2
        );
        let saved = load_control_plane_snapshot(&control)
            .await
            .unwrap()
            .unwrap();
        assert!(
            saved.pending_history_deletions.is_empty(),
            "the entry is cleared in a normal save"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    fn upgrader() -> anima_core::AgentConfig {
        anima_core::AgentConfig {
            name: "upgrader".into(),
            model: "deterministic".into(),
            bio: None,
            lore: None,
            knowledge: None,
            topics: None,
            adjectives: None,
            style: None,
            provider: None,
            system: None,
            tools: None,
            plugins: None,
            settings: None,
        }
    }

    /// A snapshot file as an older daemon wrote it: no sessions, and the
    /// given version field (or none).
    fn older_snapshot_file(version: Option<u32>) -> String {
        let mut source = crate::state::DaemonState::new();
        source.create_agent(upgrader()).unwrap();
        let mut value = serde_json::to_value(source.control_plane_snapshot()).unwrap();
        let object = value.as_object_mut().unwrap();
        object.remove("sessions");
        match version {
            Some(version) => {
                object.insert("version".into(), version.into());
            }
            None => {
                object.remove("version");
            }
        }
        serde_json::to_string_pretty(&value).unwrap()
    }

    #[tokio::test]
    async fn upgrading_any_older_snapshot_writes_the_backup_before_saving_version_five() {
        for version in [None, Some(1), Some(2), Some(3), Some(4)] {
            let dir = temp_dir("upgrade");
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("control-plane.json");
            let original = older_snapshot_file(version);
            std::fs::write(&path, &original).unwrap();
            let state = Arc::new(tokio::sync::RwLock::new(crate::state::DaemonState::new()));

            configure_control_plane_store(
                &state,
                Some(ControlPlaneStoreConfig::Json(path.clone())),
            )
            .await
            .unwrap();

            let backup = crate::control_plane_store::pre_sessions_backup_path(&path);
            assert_eq!(
                std::fs::read_to_string(&backup).unwrap(),
                original,
                "{version:?}: the backup is the untouched original"
            );
            let saved: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(saved["version"], 5, "{version:?}");
            assert_eq!(state.read().await.agent_count(), 1);
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[tokio::test]
    async fn a_current_snapshot_loads_without_a_backup() {
        let dir = temp_dir("current");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("control-plane.json");
        let config = ControlPlaneStoreConfig::Json(path.clone());
        let fresh = Arc::new(tokio::sync::RwLock::new(crate::state::DaemonState::new()));
        configure_control_plane_store(&fresh, Some(config.clone()))
            .await
            .unwrap();
        assert!(path.exists(), "a fresh start saves a version-5 snapshot");

        let restarted = Arc::new(tokio::sync::RwLock::new(crate::state::DaemonState::new()));
        configure_control_plane_store(&restarted, Some(config))
            .await
            .unwrap();

        assert!(!crate::control_plane_store::pre_sessions_backup_path(&path).exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    // Controller ruling (pre-flight audit): the backup is written only when the
    // loaded version is below CONTROL_PLANE_STORE_VERSION, so a later version
    // bump can never clobber it. This simulates a backup already on disk from a
    // prior upgrade and proves loading a current (v5) snapshot never rewrites it.
    #[tokio::test]
    async fn loading_a_current_snapshot_leaves_an_existing_pre_upgrade_backup_untouched() {
        let dir = temp_dir("current-with-backup");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("control-plane.json");
        let config = ControlPlaneStoreConfig::Json(path.clone());
        let fresh = Arc::new(tokio::sync::RwLock::new(crate::state::DaemonState::new()));
        configure_control_plane_store(&fresh, Some(config.clone()))
            .await
            .unwrap();
        assert!(path.exists(), "a fresh start saves a version-5 snapshot");

        let backup = crate::control_plane_store::pre_sessions_backup_path(&path);
        let preexisting_backup = "{\"version\":3,\"agents\":[],\"swarms\":[]}";
        std::fs::write(&backup, preexisting_backup).unwrap();

        let restarted = Arc::new(tokio::sync::RwLock::new(crate::state::DaemonState::new()));
        configure_control_plane_store(&restarted, Some(config))
            .await
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(&backup).unwrap(),
            preexisting_backup,
            "loading an already-current snapshot must not rewrite an existing backup"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    // Review finding (fix round 1, Important): the backup write's `?` in
    // `configure_control_plane_store` is the only thing standing between a
    // failed backup and an unprotected upgrade. Force the write to fail
    // deterministically (no permission tricks): put a directory where the
    // backup file needs to go, so `AtomicFile`'s rename onto that path fails.
    #[tokio::test]
    async fn a_failed_backup_write_refuses_boot_and_leaves_the_snapshot_untouched() {
        let dir = temp_dir("backup-failure");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("control-plane.json");
        let original = older_snapshot_file(Some(4));
        std::fs::write(&path, &original).unwrap();

        let backup_path = crate::control_plane_store::pre_sessions_backup_path(&path);
        std::fs::create_dir_all(&backup_path).unwrap();

        let state = Arc::new(tokio::sync::RwLock::new(crate::state::DaemonState::new()));
        configure_control_plane_store(&state, Some(ControlPlaneStoreConfig::Json(path.clone())))
            .await
            .expect_err("a backup write that cannot replace a directory must refuse boot");

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            original,
            "the original snapshot must be untouched when the backup fails"
        );
        let residue = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".atomicwrite")
            });
        assert!(
            !residue,
            "the atomic writer's temp file must not survive a failed rename"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
