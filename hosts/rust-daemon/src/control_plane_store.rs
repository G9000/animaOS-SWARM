use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anima_core::AgentRuntimeSnapshot;
use anima_swarm::{SwarmConfig, SwarmState};
use atomicwrites::{AllowOverwrite, AtomicFile};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};

use crate::connectors::gcalendar::{CalendarPendingWriteRecord, GoogleCalendarConnectorRecord};
use crate::connectors::{
    TelegramConnectorRecord, TelegramCredentialCleanupIntent, TelegramInboundRecord,
    TelegramOutboundRecord,
};
use crate::schedules::ScheduledPromptRecord;

/// Snapshot format version. Version 5 adds sessions (companion console M2);
/// older daemons refuse it, so the first start writes a backup (spec §13.3).
pub(crate) const CONTROL_PLANE_STORE_VERSION: u32 = 5;
/// JSON snapshots written before the format was versioned.
const UNVERSIONED_SNAPSHOT_VERSION: u32 = 1;
/// Suffix of the JSON backup taken before the sessions upgrade (spec §13.3).
pub(crate) const PRE_SESSIONS_BACKUP_SUFFIX: &str = ".pre-sessions.bak";
const CONTROL_PLANE_SNAPSHOT_KEY: &str = "control_plane";

#[derive(Clone, Debug)]
pub(crate) enum ControlPlaneStoreConfig {
    Json(PathBuf),
    Postgres(PgPool),
}

impl ControlPlaneStoreConfig {
    pub(crate) fn file_path(&self) -> Option<&PathBuf> {
        match self {
            Self::Json(path) => Some(path),
            Self::Postgres(_) => None,
        }
    }

    pub(crate) const fn storage_label(&self) -> &'static str {
        match self {
            Self::Json(_) => "json",
            Self::Postgres(_) => "postgres",
        }
    }

    pub(crate) fn location_label(&self) -> String {
        match self {
            Self::Json(path) => path.display().to_string(),
            Self::Postgres(_) => "postgres:host_snapshots/control_plane".into(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceConfig {
    pub(crate) root_path: PathBuf,
    pub(crate) company_name: String,
    pub(crate) mission: String,
    #[serde(default)]
    pub(crate) values: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ControlPlaneSnapshot {
    #[serde(default)]
    pub(crate) version: u32,
    #[serde(default)]
    pub(crate) agents: Vec<AgentRuntimeSnapshot>,
    #[serde(default)]
    pub(crate) swarms: Vec<StoredSwarmSnapshot>,
    #[serde(default)]
    pub(crate) connectors: Vec<TelegramConnectorRecord>,
    #[serde(default)]
    pub(crate) credential_cleanup: Vec<TelegramCredentialCleanupIntent>,
    #[serde(default)]
    pub(crate) inbound: Vec<TelegramInboundRecord>,
    #[serde(default)]
    pub(crate) outbound: Vec<TelegramOutboundRecord>,
    #[serde(default)]
    pub(crate) schedules: Vec<ScheduledPromptRecord>,
    #[serde(default)]
    pub(crate) jobs: Vec<crate::jobs::AgentJobRecord>,
    #[serde(default)]
    pub(crate) goals: Vec<crate::jobs::GoalRecord>,
    #[serde(default)]
    pub(crate) calendar_connectors: Vec<GoogleCalendarConnectorRecord>,
    #[serde(default)]
    pub(crate) calendar_writes: Vec<CalendarPendingWriteRecord>,
    #[serde(default)]
    pub(crate) mail_records: Vec<crate::connectors::mail::MailRecord>,
    #[serde(default)]
    pub(crate) mail_drafts: Vec<crate::connectors::mail::MailDraft>,
    #[serde(default)]
    pub(crate) workspace: Option<WorkspaceConfig>,
    #[serde(default)]
    pub(crate) runs: Vec<crate::runs::RunRecord>,
    #[serde(default)]
    pub(crate) sessions: Vec<crate::sessions::SessionRecord>,
    #[serde(default)]
    pub(crate) pending_history_deletions: Vec<crate::history::HistoryDeletion>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StoredSwarmSnapshot {
    pub(crate) config: SwarmConfig,
    pub(crate) state: SwarmState,
}

pub(crate) async fn save_control_plane_snapshot(
    config: Option<&ControlPlaneStoreConfig>,
    snapshot: &ControlPlaneSnapshot,
) -> io::Result<()> {
    let Some(config) = config else {
        return Ok(());
    };

    match config {
        ControlPlaneStoreConfig::Json(path) => save_json_snapshot(path, snapshot),
        ControlPlaneStoreConfig::Postgres(pool) => save_postgres_snapshot(pool, snapshot).await,
    }
}

pub(crate) async fn load_control_plane_snapshot(
    config: &ControlPlaneStoreConfig,
) -> io::Result<Option<ControlPlaneSnapshot>> {
    match config {
        ControlPlaneStoreConfig::Json(path) => load_json_snapshot(path),
        ControlPlaneStoreConfig::Postgres(pool) => load_postgres_snapshot(pool).await,
    }
}

fn save_json_snapshot(path: &Path, snapshot: &ControlPlaneSnapshot) -> io::Result<()> {
    save_json_snapshot_with_writer(path, snapshot, |file, payload| {
        file.write_all(payload)?;
        file.sync_all()
    })
}

fn save_json_snapshot_with_writer(
    path: &Path,
    snapshot: &ControlPlaneSnapshot,
    write: impl FnOnce(&mut File, &[u8]) -> io::Result<()>,
) -> io::Result<()> {
    ensure_parent_dir(path)?;
    let payload = serde_json::to_string_pretty(snapshot).map_err(serde_error)?;
    AtomicFile::new(path, AllowOverwrite)
        .write(|file| write(file, payload.as_bytes()))
        .map_err(atomic_write_error)?;
    sync_snapshot_parent(path)
}

fn load_json_snapshot(path: &Path) -> io::Result<Option<ControlPlaneSnapshot>> {
    if !path.exists() {
        return Ok(None);
    }
    let contents = fs::read_to_string(path)?;
    if contents.trim().is_empty() {
        return Ok(None);
    }

    let mut snapshot =
        serde_json::from_str::<ControlPlaneSnapshot>(&contents).map_err(serde_error)?;
    if snapshot.version == 0 {
        snapshot.version = UNVERSIONED_SNAPSHOT_VERSION;
    }
    if snapshot.version > CONTROL_PLANE_STORE_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "unsupported control plane store version: {}",
                snapshot.version
            ),
        ));
    }

    Ok(Some(snapshot))
}

/// Where the JSON snapshot is backed up before the sessions upgrade.
pub(crate) fn pre_sessions_backup_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(PRE_SESSIONS_BACKUP_SUFFIX);
    path.with_file_name(name)
}

/// The `host_snapshots` key of the Postgres backup of a `version` snapshot.
pub(crate) fn postgres_backup_key(version: u32) -> String {
    format!("{CONTROL_PLANE_SNAPSHOT_KEY}.backup.{version}")
}

/// Saves the loaded snapshot, unchanged, before the upgrade rewrites it
/// (spec §13.3 step 1), and returns where the backup is.
pub(crate) async fn write_pre_upgrade_backup(
    config: &ControlPlaneStoreConfig,
    loaded_version: u32,
) -> io::Result<String> {
    match config {
        ControlPlaneStoreConfig::Json(path) => {
            backup_json_snapshot(path).map(|backup| backup.display().to_string())
        }
        ControlPlaneStoreConfig::Postgres(pool) => {
            backup_postgres_snapshot(pool, loaded_version).await
        }
    }
}

fn backup_json_snapshot(path: &Path) -> io::Result<PathBuf> {
    let bytes = fs::read(path)?;
    let backup = pre_sessions_backup_path(path);
    AtomicFile::new(&backup, AllowOverwrite)
        .write(|file| {
            file.write_all(&bytes)?;
            file.sync_all()
        })
        .map_err(atomic_write_error)?;
    sync_snapshot_parent(&backup)?;
    Ok(backup)
}

async fn backup_postgres_snapshot(pool: &PgPool, version: u32) -> io::Result<String> {
    let key = postgres_backup_key(version);
    sqlx::query(
        r#"
        INSERT INTO host_snapshots (key, version, payload, updated_at)
        SELECT $1, version, payload, now() FROM host_snapshots WHERE key = $2
        ON CONFLICT (key)
        DO UPDATE SET
            version = EXCLUDED.version,
            payload = EXCLUDED.payload,
            updated_at = EXCLUDED.updated_at
        "#,
    )
    .bind(&key)
    .bind(CONTROL_PLANE_SNAPSHOT_KEY)
    .execute(pool)
    .await
    .map_err(postgres_error)?;
    Ok(format!("postgres:host_snapshots/{key}"))
}

async fn save_postgres_snapshot(pool: &PgPool, snapshot: &ControlPlaneSnapshot) -> io::Result<()> {
    let payload = serde_json::to_value(snapshot).map_err(serde_error)?;
    sqlx::query(
        r#"
        INSERT INTO host_snapshots (key, version, payload, updated_at)
        VALUES ($1, $2, $3, now())
        ON CONFLICT (key)
        DO UPDATE SET
            version = EXCLUDED.version,
            payload = EXCLUDED.payload,
            updated_at = EXCLUDED.updated_at
        "#,
    )
    .bind(CONTROL_PLANE_SNAPSHOT_KEY)
    .bind(CONTROL_PLANE_STORE_VERSION as i32)
    .bind(payload)
    .execute(pool)
    .await
    .map_err(postgres_error)?;
    Ok(())
}

async fn load_postgres_snapshot(pool: &PgPool) -> io::Result<Option<ControlPlaneSnapshot>> {
    let Some(row) = sqlx::query("SELECT version, payload FROM host_snapshots WHERE key = $1")
        .bind(CONTROL_PLANE_SNAPSHOT_KEY)
        .fetch_optional(pool)
        .await
        .map_err(postgres_error)?
    else {
        return Ok(None);
    };
    let version: i32 = row.get("version");
    if version > CONTROL_PLANE_STORE_VERSION as i32 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported control plane store version: {version}"),
        ));
    }
    let payload: serde_json::Value = row.get("payload");
    let mut snapshot =
        serde_json::from_value::<ControlPlaneSnapshot>(payload).map_err(serde_error)?;
    if snapshot.version == 0 {
        snapshot.version = version.max(1) as u32;
    }
    if snapshot.version > CONTROL_PLANE_STORE_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "unsupported control plane store version: {}",
                snapshot.version
            ),
        ));
    }
    Ok(Some(snapshot))
}

fn ensure_parent_dir(path: &Path) -> io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

#[cfg(unix)]
fn snapshot_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

#[cfg(unix)]
fn sync_snapshot_parent(path: &Path) -> io::Result<()> {
    File::open(snapshot_parent(path))?.sync_all()
}

#[cfg(not(unix))]
fn sync_snapshot_parent(_path: &Path) -> io::Result<()> {
    Ok(())
}

fn atomic_write_error(error: atomicwrites::Error<io::Error>) -> io::Error {
    match error {
        atomicwrites::Error::Internal(error) | atomicwrites::Error::User(error) => error,
    }
}

fn serde_error(error: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

fn postgres_error(error: sqlx::Error) -> io::Error {
    io::Error::new(io::ErrorKind::Other, error)
}

impl ControlPlaneSnapshot {
    #[cfg(test)]
    pub(crate) fn new(agents: Vec<AgentRuntimeSnapshot>, swarms: Vec<StoredSwarmSnapshot>) -> Self {
        Self::with_connector_state(agents, swarms, vec![], vec![], vec![], vec![])
    }

    pub(crate) fn with_connector_state(
        agents: Vec<AgentRuntimeSnapshot>,
        swarms: Vec<StoredSwarmSnapshot>,
        connectors: Vec<TelegramConnectorRecord>,
        inbound: Vec<TelegramInboundRecord>,
        outbound: Vec<TelegramOutboundRecord>,
        schedules: Vec<ScheduledPromptRecord>,
    ) -> Self {
        Self::with_connector_state_and_cleanup(
            agents,
            swarms,
            connectors,
            vec![],
            inbound,
            outbound,
            schedules,
        )
    }

    pub(crate) fn with_connector_state_and_cleanup(
        agents: Vec<AgentRuntimeSnapshot>,
        swarms: Vec<StoredSwarmSnapshot>,
        connectors: Vec<TelegramConnectorRecord>,
        credential_cleanup: Vec<TelegramCredentialCleanupIntent>,
        inbound: Vec<TelegramInboundRecord>,
        outbound: Vec<TelegramOutboundRecord>,
        schedules: Vec<ScheduledPromptRecord>,
    ) -> Self {
        Self {
            version: CONTROL_PLANE_STORE_VERSION,
            agents,
            swarms,
            connectors,
            credential_cleanup,
            inbound,
            outbound,
            schedules,
            mail_records: vec![],
            jobs: vec![],
            goals: vec![],
            mail_drafts: vec![],
            calendar_connectors: vec![],
            calendar_writes: vec![],
            workspace: None,
            runs: vec![],
            sessions: vec![],
            pending_history_deletions: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ControlPlaneSnapshot, WorkspaceConfig};
    use std::path::PathBuf;

    #[test]
    fn v3_snapshot_without_workspace_loads_as_unconfigured() {
        let json = r#"{
            "version": 3,
            "agents": [],
            "swarms": [],
            "connectors": [],
            "credentialCleanup": [],
            "inbound": [],
            "outbound": [],
            "schedules": []
        }"#;
        let snapshot: ControlPlaneSnapshot =
            serde_json::from_str(json).expect("v3 snapshot parses");
        assert!(snapshot.workspace.is_none());
    }

    #[test]
    fn workspace_config_round_trips() {
        let config = WorkspaceConfig {
            root_path: PathBuf::from("C:\\workspaces\\northwind"),
            company_name: "Northwind Research".into(),
            mission: "Continuous equity research".into(),
            values: vec!["cite sources".into()],
        };
        let snapshot = ControlPlaneSnapshot {
            workspace: Some(config.clone()),
            ..Default::default()
        };
        let payload = serde_json::to_string(&snapshot).expect("serialize");
        let restored: ControlPlaneSnapshot = serde_json::from_str(&payload).expect("deserialize");
        assert_eq!(restored.workspace, Some(config));
    }

    #[test]
    fn json_snapshot_replaces_an_existing_snapshot_only_after_a_synced_temp_write() {
        let path = test_snapshot_path("atomic-replace");
        let mut previous = ControlPlaneSnapshot::new(vec![], vec![]);
        previous.version = 1;
        let replacement = ControlPlaneSnapshot::new(vec![], vec![]);
        let previous_payload = serde_json::to_string_pretty(&previous).expect("serializes");
        std::fs::write(&path, &previous_payload).expect("previous snapshot should be written");

        super::save_json_snapshot_with_writer(&path, &replacement, |file, payload| {
            use std::io::Write;

            file.write_all(payload)?;
            file.sync_all()?;
            assert_eq!(
                std::fs::read_to_string(&path)?,
                previous_payload,
                "the prior snapshot remains intact until the complete temp write finishes"
            );
            Ok(())
        })
        .expect("atomic replacement should succeed");

        let loaded = super::load_json_snapshot(&path)
            .expect("replacement should load")
            .expect("replacement should exist");
        assert_eq!(loaded.version, 5);
        assert_no_temp_residue(&path);
        let _ = std::fs::remove_dir_all(path.parent().expect("snapshot path has a parent"));
    }

    #[test]
    fn json_snapshot_replacement_failure_preserves_prior_snapshot_and_cleans_temp_file() {
        let path = test_snapshot_path("atomic-failure");
        let mut previous = ControlPlaneSnapshot::new(vec![], vec![]);
        previous.version = 1;
        let replacement = ControlPlaneSnapshot::new(vec![], vec![]);
        let previous_payload = serde_json::to_string_pretty(&previous).expect("serializes");
        std::fs::write(&path, &previous_payload).expect("previous snapshot should be written");

        let error = super::save_json_snapshot_with_writer(&path, &replacement, |file, _| {
            use std::io::Write;

            file.write_all(b"partial")?;
            Err(std::io::Error::other("simulated replacement failure"))
        })
        .expect_err("replacement failure should be returned");
        assert_eq!(error.kind(), std::io::ErrorKind::Other);
        assert_eq!(
            std::fs::read_to_string(&path).expect("prior snapshot remains readable"),
            previous_payload
        );
        assert_no_temp_residue(&path);
        let _ = std::fs::remove_dir_all(path.parent().expect("snapshot path has a parent"));
    }

    fn test_snapshot_path(label: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "anima-control-plane-{label}-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).expect("snapshot test directory should be created");
        directory.join("control-plane.json")
    }

    fn assert_no_temp_residue(path: &std::path::Path) {
        let parent = path.parent().expect("snapshot path should have a parent");
        let temp_prefix = ".atomicwrite";
        let residue = std::fs::read_dir(parent)
            .expect("snapshot parent should be readable")
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().starts_with(temp_prefix));
        assert!(!residue, "temporary snapshot files must be cleaned up");
    }

    #[test]
    fn snapshot_serializes_current_version_with_empty_connector_collections() {
        let snapshot = ControlPlaneSnapshot::new(vec![], vec![]);
        let payload = serde_json::to_value(snapshot).expect("snapshot should serialize");

        assert_eq!(payload["version"], 5);
        assert_eq!(payload["connectors"], serde_json::json!([]));
        assert_eq!(payload["credentialCleanup"], serde_json::json!([]));
        assert_eq!(payload["inbound"], serde_json::json!([]));
        assert_eq!(payload["outbound"], serde_json::json!([]));
        assert_eq!(payload["schedules"], serde_json::json!([]));
        assert_eq!(payload["runs"], serde_json::json!([]));
        assert_eq!(payload["sessions"], serde_json::json!([]));
        assert_eq!(payload["pendingHistoryDeletions"], serde_json::json!([]));
    }

    #[test]
    fn version_one_payload_round_trips_new_collections_as_empty() {
        let snapshot: ControlPlaneSnapshot = serde_json::from_value(serde_json::json!({
            "version": 1,
            "agents": [],
            "swarms": []
        }))
        .expect("version-one snapshot should deserialize");

        let payload = serde_json::to_value(snapshot).expect("snapshot should serialize");
        assert_eq!(payload["connectors"], serde_json::json!([]));
        assert_eq!(payload["credentialCleanup"], serde_json::json!([]));
        assert_eq!(payload["inbound"], serde_json::json!([]));
        assert_eq!(payload["outbound"], serde_json::json!([]));
        assert_eq!(payload["schedules"], serde_json::json!([]));
    }

    #[test]
    fn version_two_payload_defaults_cleanup_intents_to_empty() {
        let snapshot: ControlPlaneSnapshot = serde_json::from_value(serde_json::json!({
            "version": 2,
            "agents": [],
            "swarms": [],
            "connectors": [],
            "inbound": [],
            "outbound": [],
            "schedules": []
        }))
        .expect("version-two snapshot should deserialize");

        assert!(snapshot.credential_cleanup.is_empty());
    }

    #[test]
    fn version_one_json_file_loads_new_collections_as_empty() {
        let path = std::env::temp_dir().join(format!(
            "anima-control-plane-v1-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos()
        ));
        std::fs::write(&path, r#"{"version":1,"agents":[],"swarms":[]}"#)
            .expect("version-one snapshot should be written");

        let snapshot = super::load_json_snapshot(&path)
            .expect("version-one snapshot should load")
            .expect("version-one snapshot should exist");
        assert!(snapshot.connectors.is_empty());
        assert!(snapshot.credential_cleanup.is_empty());
        assert!(snapshot.inbound.is_empty());
        assert!(snapshot.outbound.is_empty());
        assert!(snapshot.schedules.is_empty());

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn version_one_postgres_payload_deserializes_new_collections_as_empty() {
        let snapshot: ControlPlaneSnapshot = serde_json::from_value(serde_json::json!({
            "agents": [],
            "swarms": []
        }))
        .expect("version-one postgres payload should deserialize");

        assert_eq!(
            snapshot.version, 0,
            "the row version supplies v1 when absent"
        );
        assert!(snapshot.connectors.is_empty());
        assert!(snapshot.credential_cleanup.is_empty());
        assert!(snapshot.inbound.is_empty());
        assert!(snapshot.outbound.is_empty());
        assert!(snapshot.schedules.is_empty());
    }

    #[test]
    fn version_four_snapshot_loads_with_an_empty_run_ledger() {
        let snapshot: ControlPlaneSnapshot = serde_json::from_value(serde_json::json!({
            "version": 4,
            "agents": [],
            "swarms": []
        }))
        .expect("version-four snapshot should deserialize");

        assert!(snapshot.runs.is_empty());
        assert!(snapshot.pending_history_deletions.is_empty());
    }

    #[test]
    fn unversioned_json_snapshots_load_as_legacy_version_one() {
        let path = test_snapshot_path("unversioned");
        std::fs::write(&path, r#"{"agents":[],"swarms":[]}"#).unwrap();

        let loaded = super::load_json_snapshot(&path).unwrap().unwrap();

        assert_eq!(loaded.version, 1, "an unversioned file predates sessions");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn the_backup_path_appends_the_suffix_to_the_file_name() {
        assert_eq!(
            super::pre_sessions_backup_path(std::path::Path::new("/data/control-plane.json")),
            std::path::PathBuf::from("/data/control-plane.json.pre-sessions.bak")
        );
        assert_eq!(super::postgres_backup_key(4), "control_plane.backup.4");
    }

    #[tokio::test]
    async fn the_pre_upgrade_backup_copies_the_exact_file_bytes_and_a_later_upgrade_replaces_it() {
        let path = test_snapshot_path("backup");
        let original = "{\n  \"version\": 4,\n  \"agents\": [],\n  \"swarms\": []\n}\n";
        std::fs::write(&path, original).unwrap();
        let config = super::ControlPlaneStoreConfig::Json(path.clone());

        let location = super::write_pre_upgrade_backup(&config, 4).await.unwrap();

        let backup = super::pre_sessions_backup_path(&path);
        assert_eq!(location, backup.display().to_string());
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
        std::fs::write(&path, "{\"version\":3}").unwrap();
        super::write_pre_upgrade_backup(&config, 3).await.unwrap();
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), "{\"version\":3}");
        assert_no_temp_residue(&path);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[ignore = "requires DATABASE_URL-backed Postgres"]
    #[sqlx::test(migrations = "./migrations")]
    async fn the_postgres_pre_upgrade_backup_copies_the_snapshot_row(pool: sqlx::PgPool) {
        use sqlx::Row;

        sqlx::query(
            "INSERT INTO host_snapshots (key, version, payload) VALUES ('control_plane', 4, '{\"version\":4,\"agents\":[]}')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let config = super::ControlPlaneStoreConfig::Postgres(pool.clone());

        let location = super::write_pre_upgrade_backup(&config, 4).await.unwrap();

        assert_eq!(location, "postgres:host_snapshots/control_plane.backup.4");
        let row = sqlx::query(
            "SELECT version, payload FROM host_snapshots WHERE key = 'control_plane.backup.4'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.get::<i32, _>("version"), 4);
        assert_eq!(
            row.get::<serde_json::Value, _>("payload"),
            serde_json::json!({"version": 4, "agents": []})
        );
    }
}
