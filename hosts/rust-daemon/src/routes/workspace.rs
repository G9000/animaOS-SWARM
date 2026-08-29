use super::contracts::{WorkspaceConfigResponse, WorkspaceResponse};
use super::ApiError;
use crate::app::SharedDaemonState;
use crate::control_plane_store::WorkspaceConfig;

pub(crate) async fn handle_get_workspace(
    state: &SharedDaemonState,
) -> Result<WorkspaceResponse, ApiError> {
    let workspace = {
        let guard = state.read().await;
        guard.workspace.clone()
    };
    Ok(WorkspaceResponse {
        configured: workspace.is_some(),
        workspace: workspace.as_ref().map(config_response),
        default_root: default_root_label(),
        root_path_exists: None,
    })
}

pub(super) fn config_response(config: &WorkspaceConfig) -> WorkspaceConfigResponse {
    WorkspaceConfigResponse {
        root_path: config.root_path.display().to_string(),
        company_name: config.company_name.clone(),
        mission: config.mission.clone(),
        values: config.values.clone(),
    }
}

pub(super) fn default_root_label() -> String {
    match std::env::var("ANIMAOS_WORKSPACE_ROOT") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => std::env::current_dir()
            .map(|path| path.display().to_string())
            .unwrap_or_default(),
    }
}
