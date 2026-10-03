//! Fixtures for the skills tests: a temporary workspace, skill text, and a
//! control-plane store that cannot be saved.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Mutex;

use super::{compose_skill_file, SkillContent, SkillService};
use crate::app::SharedDaemonState;
use crate::control_plane_store::{ControlPlaneStoreConfig, WorkspaceConfig};
use crate::state::DaemonState;

pub(crate) fn temp_workspace(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "anima-skills-{label}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

/// The canonical `SKILL.md` `content(name)` saves.
pub(crate) fn skill_text(name: &str) -> String {
    compose_skill_file(name, &format!("About {name}"), &format!("Do {name}."))
}

/// Writes `skills/<slug>/SKILL.md` by hand, as the owner or a tool would.
pub(crate) fn write_skill(root: &Path, slug: &str, text: &str) -> PathBuf {
    let folder = root.join("skills").join(slug);
    std::fs::create_dir_all(&folder).unwrap();
    let path = folder.join("SKILL.md");
    std::fs::write(&path, text).unwrap();
    path
}

pub(crate) fn with_workspace(mut state: DaemonState, root: &Path) -> DaemonState {
    state.workspace = Some(WorkspaceConfig {
        root_path: root.to_path_buf(),
        company_name: "Acme".into(),
        mission: "Ship carefully".into(),
        values: vec![],
    });
    state
}

/// A service over `state` with its own control-plane transaction.
pub(crate) fn service(state: &SharedDaemonState) -> SkillService {
    SkillService::new(Arc::clone(state), Arc::new(Mutex::new(())))
}

pub(crate) fn content(name: &str) -> SkillContent {
    SkillContent {
        name: name.into(),
        description: format!("About {name}"),
        body: format!("Do {name}."),
        enabled: None,
    }
}

/// A JSON store whose path is a directory, so every save fails.
pub(crate) fn broken_store() -> ControlPlaneStoreConfig {
    let path = temp_workspace("broken-store");
    ControlPlaneStoreConfig::Json(path)
}
