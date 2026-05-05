use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anima_core::AgentRuntimeSnapshot;
use anima_swarm::{SwarmConfig, SwarmState};
use serde::{Deserialize, Serialize};

const CONTROL_PLANE_STORE_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ControlPlaneStoreConfig {
    Json(PathBuf),
}

impl ControlPlaneStoreConfig {
    pub(crate) fn path(&self) -> &PathBuf {
        match self {
            Self::Json(path) => path,
        }
    }

    pub(crate) const fn storage_label(&self) -> &'static str {
        match self {
            Self::Json(_) => "json",
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

pub(crate) fn save_control_plane_snapshot(
    config: Option<&ControlPlaneStoreConfig>,
    snapshot: &ControlPlaneSnapshot,
) -> io::Result<()> {
    let Some(config) = config else {
        return Ok(());
    };

    match config {
        ControlPlaneStoreConfig::Json(path) => save_json_snapshot(path, snapshot),
    }
}

pub(crate) fn load_control_plane_snapshot(
    config: &ControlPlaneStoreConfig,
) -> io::Result<Option<ControlPlaneSnapshot>> {
    match config {
        ControlPlaneStoreConfig::Json(path) => load_json_snapshot(path),
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

impl ControlPlaneSnapshot {
    pub(crate) fn new(agents: Vec<AgentRuntimeSnapshot>, swarms: Vec<StoredSwarmSnapshot>) -> Self {
        Self {
            version: CONTROL_PLANE_STORE_VERSION,
            agents,
            swarms,
        }
    }
}
