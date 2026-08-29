use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anima_core::AgentRuntimeSnapshot;
use anima_swarm::{SwarmConfig, SwarmState};
use atomicwrites::{AllowOverwrite, AtomicFile};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};

use crate::connectors::{
    TelegramConnectorRecord, TelegramCredentialCleanupIntent, TelegramInboundRecord,
    TelegramOutboundRecord,
};
use crate::schedules::ScheduledPromptRecord;

const CONTROL_PLANE_STORE_VERSION: u32 = 3;
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
        snapshot.version = CONTROL_PLANE_STORE_VERSION;
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ControlPlaneSnapshot;

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
        assert_eq!(loaded.version, 3);
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

        assert_eq!(payload["version"], 3);
        assert_eq!(payload["connectors"], serde_json::json!([]));
        assert_eq!(payload["credentialCleanup"], serde_json::json!([]));
        assert_eq!(payload["inbound"], serde_json::json!([]));
        assert_eq!(payload["outbound"], serde_json::json!([]));
        assert_eq!(payload["schedules"], serde_json::json!([]));
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
}
