use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anima_core::{AgentConfig, ToolDescriptor};

use super::agencies::{AgencyYamlAgent, AgencyYamlConfig};
use super::contracts::{
    AgentRuntimeSnapshotResponse, WorkspaceBootstrapRequest, WorkspaceBootstrapResponse,
    WorkspaceConfigRequest, WorkspaceConfigResponse, WorkspaceResponse,
};
use super::profile::profile_preset;
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

pub(crate) async fn handle_put_workspace(
    body: Vec<u8>,
    state: &SharedDaemonState,
) -> Result<WorkspaceResponse, ApiError> {
    let request: WorkspaceConfigRequest = super::parse_json_body(body)?;
    let root_existed = Path::new(request.root_path.trim()).exists();
    let config = validate_workspace_request(&request)?;

    if request.validate_only {
        let currently_configured = {
            let guard = state.read().await;
            guard.workspace.is_some()
        };
        return Ok(WorkspaceResponse {
            configured: currently_configured,
            workspace: Some(config_response(&config)),
            default_root: default_root_label(),
            root_path_exists: Some(root_existed),
        });
    }

    let persist_request = {
        let mut guard = state.write().await;
        guard.workspace = Some(config);
        guard.control_plane_persist_request()
    };
    persist_request
        .save()
        .await
        .map_err(|error| ApiError::service_unavailable(error.to_string()))?;

    handle_get_workspace(state).await
}

pub(crate) async fn handle_bootstrap_workspace(
    body: Vec<u8>,
    state: &SharedDaemonState,
) -> Result<WorkspaceBootstrapResponse, ApiError> {
    let request: WorkspaceBootstrapRequest = super::parse_json_body(body)?;

    // Refuse to bootstrap twice: a configured workspace means this daemon was
    // already bootstrapped.
    {
        let guard = state.read().await;
        if guard.workspace.is_some() {
            return Err(ApiError::conflict("workspace is already bootstrapped"));
        }
    }

    // 1. Validate EVERYTHING before any side effect. Workspace validation runs
    //    with validate_only forced off so the root directory is created and
    //    canonicalized up front.
    let mut workspace_request = request.workspace;
    workspace_request.validate_only = false;
    let workspace_config = validate_workspace_request(&workspace_request)?;

    // A leftover agency file at the target root also means the workspace was
    // bootstrapped before (possibly by an earlier daemon run).
    let yaml_path = workspace_config.root_path.join("anima.yaml");
    if yaml_path.is_file() {
        return Err(ApiError::conflict(format!(
            "anima.yaml already exists at {}",
            yaml_path.display()
        )));
    }

    let agent = request.agent;
    let name = agent.name.trim().to_string();
    if name.is_empty() {
        return Err(ApiError::bad_request_static("agent.name is required"));
    }
    // The preset shaped the already-generated profile fields (bio, system,
    // style, adjectives) submitted with this request, so only its validity
    // is checked here.
    if profile_preset(agent.preset_id.trim()).is_none() {
        return Err(ApiError::bad_request(format!(
            "unknown presetId: {}",
            agent.preset_id.trim()
        )));
    }
    let system = agent.system.trim().to_string();
    if system.is_empty() {
        return Err(ApiError::bad_request_static("agent.system is required"));
    }
    let model = agent.model.trim().to_string();
    if model.is_empty() {
        return Err(ApiError::bad_request_static("agent.model is required"));
    }
    let tool_names = agent
        .tools
        .iter()
        .map(|tool| tool.trim().to_string())
        .collect::<Vec<_>>();
    if tool_names.is_empty() {
        return Err(ApiError::bad_request_static(
            "agent.tools must not be empty",
        ));
    }

    // 2. Build the AgentConfig with name-only tool descriptors; create_agent
    //    canonicalizes them against the registry before mutating state.
    let non_blank = |value: Option<String>| {
        value
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };
    // The CLI agency loader requires a truthy orchestrator bio, so a blank or
    // omitted bio is rejected rather than serialized as an empty field.
    let bio = non_blank(agent.bio)
        .ok_or_else(|| ApiError::bad_request_static("agent.bio is required"))?;
    let style = non_blank(agent.style);
    let provider = non_blank(agent.provider);
    let adjectives = agent
        .adjectives
        .map(|items| {
            items
                .iter()
                .map(|item| item.trim().to_string())
                .filter(|item| !item.is_empty())
                .collect::<Vec<_>>()
        })
        .filter(|items| !items.is_empty());

    let tools = tool_names
        .iter()
        .map(|tool| ToolDescriptor {
            name: tool.clone(),
            description: String::new(),
            parameters_schema: BTreeMap::new(),
            examples: None,
        })
        .collect::<Vec<_>>();

    let config = AgentConfig {
        name: name.clone(),
        model: model.clone(),
        bio: Some(bio.clone()),
        lore: None,
        knowledge: None,
        topics: None,
        adjectives: adjectives.clone(),
        style: style.clone(),
        provider: provider.clone(),
        system: Some(system.clone()),
        tools: Some(tools),
        plugins: None,
        settings: None,
    };

    // 3. One state write: create the agent (tool slugs are validated before
    //    any mutation inside create_agent) and set the workspace config.
    let (snapshot, persist_request) = {
        let mut guard = state.write().await;
        let snapshot = guard.create_agent(config).map_err(ApiError::bad_request)?;
        guard.workspace = Some(workspace_config.clone());
        (snapshot, guard.control_plane_persist_request())
    };

    // 4. Write anima.yaml at the workspace root AFTER the agent exists,
    //    atomically (tmp file + rename within the same directory, i.e. the
    //    same filesystem). On IO failure roll back the state write so
    //    bootstrap stays atomic.
    let agency_yaml = AgencyYamlConfig::single_orchestrator(
        workspace_config.company_name.clone(),
        workspace_config.mission.clone(),
        workspace_config.values.clone(),
        provider.unwrap_or_default(),
        model.clone(),
        AgencyYamlAgent::orchestrator(
            name,
            bio,
            style,
            system,
            Some(model),
            Some(tool_names),
            adjectives,
        ),
    );
    let tmp_path = workspace_config.root_path.join("anima.yaml.tmp");
    let write_result = serde_yaml::to_string(&agency_yaml)
        .map_err(|error| format!("failed to serialize agency yaml: {error}"))
        .and_then(|yaml| {
            std::fs::write(&tmp_path, yaml)
                .and_then(|()| std::fs::rename(&tmp_path, &yaml_path))
                .map_err(|error| format!("failed to write anima.yaml: {error}"))
        });
    if let Err(message) = write_result {
        rollback_bootstrap(state, &snapshot.state.id, &yaml_path, &tmp_path).await;
        return Err(ApiError::service_unavailable(message));
    }

    // 5. Persist the control-plane snapshot last; a failure here rolls back
    //    exactly like a yaml-write failure.
    if let Err(error) = persist_request.save().await {
        rollback_bootstrap(state, &snapshot.state.id, &yaml_path, &tmp_path).await;
        return Err(ApiError::service_unavailable(error.to_string()));
    }

    Ok(WorkspaceBootstrapResponse {
        workspace: config_response(&workspace_config),
        agent: AgentRuntimeSnapshotResponse::from(&snapshot),
    })
}

/// Undo a partially applied bootstrap: drop the agent and workspace state,
/// remove any written agency yaml files, and persist the rolled-back state on
/// a best-effort basis.
async fn rollback_bootstrap(
    state: &SharedDaemonState,
    agent_id: &str,
    yaml_path: &Path,
    tmp_path: &Path,
) {
    std::fs::remove_file(tmp_path).ok();
    std::fs::remove_file(yaml_path).ok();
    let rollback_request = {
        let mut guard = state.write().await;
        guard.remove_agent(agent_id);
        guard.workspace = None;
        guard.control_plane_persist_request()
    };
    rollback_request.save().await.ok();
}

pub(super) fn validate_workspace_request(
    request: &WorkspaceConfigRequest,
) -> Result<WorkspaceConfig, ApiError> {
    let company = request.company_name.trim();
    if company.is_empty() {
        return Err(ApiError::bad_request_static("companyName is required"));
    }
    let mission = request.mission.trim();
    if mission.is_empty() {
        return Err(ApiError::bad_request_static("mission is required"));
    }
    let root = validate_root_path(&request.root_path, request.validate_only)?;
    let values = request
        .values
        .iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .take(5)
        .collect();
    Ok(WorkspaceConfig {
        root_path: root,
        company_name: company.to_string(),
        mission: mission.to_string(),
        values,
    })
}

fn validate_root_path(raw: &str, validate_only: bool) -> Result<PathBuf, ApiError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ApiError::bad_request_static("rootPath is required"));
    }
    let candidate = PathBuf::from(trimmed);
    if !candidate.is_absolute() {
        return Err(ApiError::bad_request_static(
            "rootPath must be an absolute path",
        ));
    }
    if candidate.exists() {
        if !candidate.is_dir() {
            return Err(ApiError::bad_request_static("rootPath is not a directory"));
        }
    } else {
        let ancestor = candidate
            .ancestors()
            .skip(1)
            .find(|path| path.exists())
            .ok_or_else(|| ApiError::bad_request_static("rootPath has no existing ancestor"))?;
        if !ancestor.is_dir() {
            return Err(ApiError::bad_request_static(
                "rootPath's nearest existing ancestor is not a directory",
            ));
        }
        if !validate_only {
            std::fs::create_dir_all(&candidate).map_err(|error| {
                ApiError::bad_request(format!("rootPath could not be created: {error}"))
            })?;
        }
    }
    if validate_only && !candidate.exists() {
        // Nothing to canonicalize yet; return as-is.
        return Ok(candidate);
    }
    candidate
        .canonicalize()
        .map_err(|error| ApiError::bad_request(format!("rootPath could not be resolved: {error}")))
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
    // Delegates to the same resolution the tools use so the label never
    // diverges. Tools reject a set-but-empty ANIMAOS_WORKSPACE_ROOT; the
    // pre-fill label falls back to the launch directory in that case.
    crate::tools::workspace_root_path("workspace", None)
        .ok()
        .or_else(|| std::env::current_dir().ok())
        .map(|path| path.display().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::default_root_label;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        name: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(name: &'static str, value: &str) -> Self {
            let previous = std::env::var_os(name);
            std::env::set_var(name, value);
            Self { name, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(previous) = &self.previous {
                std::env::set_var(self.name, previous);
            } else {
                std::env::remove_var(self.name);
            }
        }
    }

    #[test]
    fn default_root_label_falls_back_to_launch_dir_when_env_empty() {
        let _lock = ENV_LOCK.lock().expect("env lock should not poison");
        let _guard = EnvGuard::set("ANIMAOS_WORKSPACE_ROOT", "");
        let expected = std::env::current_dir()
            .expect("current dir resolves")
            .display()
            .to_string();
        assert_eq!(default_root_label(), expected);
    }

    #[test]
    fn default_root_label_uses_env_path_when_set() {
        let _lock = ENV_LOCK.lock().expect("env lock should not poison");
        let _guard = EnvGuard::set("ANIMAOS_WORKSPACE_ROOT", "C:\\anima\\workspace");
        assert_eq!(default_root_label(), "C:\\anima\\workspace");
    }
}
