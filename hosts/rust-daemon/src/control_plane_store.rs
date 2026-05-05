use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anima_core::AgentRuntimeSnapshot;
use anima_swarm::{SwarmConfig, SwarmState};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};

const CONTROL_PLANE_STORE_VERSION: u32 = 1;
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
    ensure_parent_dir(path)?;
    let payload = serde_json::to_string_pretty(snapshot).map_err(serde_error)?;
    fs::write(path, payload)
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

fn serde_error(error: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

fn postgres_error(error: sqlx::Error) -> io::Error {
    io::Error::new(io::ErrorKind::Other, error)
}

impl ControlPlaneSnapshot {
    pub(crate) fn new(agents: Vec<AgentRuntimeSnapshot>, swarms: Vec<StoredSwarmSnapshot>) -> Self {
        Self {
            version: CONTROL_PLANE_STORE_VERSION,
            agents,
            swarms,
        }
    }
}
