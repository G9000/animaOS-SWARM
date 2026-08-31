use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anima_core::{AgentConfig, AgentRuntimeSnapshot, ToolDescriptor};

use super::agencies::{load_agency_yaml, AgencyYamlAgent, AgencyYamlConfig};
use super::contracts::{
    AgentRuntimeSnapshotResponse, WorkspaceBootstrapRequest, WorkspaceBootstrapResponse,
    WorkspaceConfigRequest, WorkspaceConfigResponse, WorkspaceInspectAgentPreview,
    WorkspaceInspectResponse, WorkspaceResponse, WorkspaceResumeRequest, WorkspaceResumeResponse,
};
use super::profile::profile_preset;
use super::ApiError;
use crate::app::SharedDaemonState;
use crate::control_plane_store::WorkspaceConfig;
use crate::runtime_model::provider_summaries;

/// Read-only inspection of a candidate workspace root: reports whether an
/// anima.yaml exists there and, when it does, a preview of the agency it
/// describes. Used by onboarding to offer resuming an existing workspace.
/// A relative rootPath resolves against the daemon's current working
/// directory; the web console always sends absolute paths.
pub(crate) async fn handle_inspect_workspace(
    root_path: &str,
) -> Result<WorkspaceInspectResponse, ApiError> {
    let not_found = || WorkspaceInspectResponse {
        found: false,
        company_name: None,
        mission: None,
        values: None,
        orchestrator: None,
        workers: None,
        provider_available: None,
    };
    let trimmed = root_path.trim();
    if trimmed.is_empty() {
        return Err(ApiError::bad_request_static("rootPath is required"));
    }
    let candidate = PathBuf::from(trimmed);
    if !candidate.is_dir() {
        return Ok(not_found());
    }
    let yaml_path = candidate.join("anima.yaml");
    if !yaml_path.is_file() {
        return Ok(not_found());
    }
    let config = load_agency_yaml(&yaml_path)?;
    let provider = {
        let provider = config.provider.trim();
        if provider.is_empty() {
            "openai".to_string()
        } else {
            provider.to_string()
        }
    };
    // providerAvailable mirrors the providers catalog: unknown or
    // unconfigured providers (and the deterministic adapter) report false.
    let provider_available = provider_summaries()
        .into_iter()
        .find(|summary| summary.id == provider)
        .map(|summary| summary.configured && summary.id != "deterministic")
        .unwrap_or(false);
    let preview = |agent: &AgencyYamlAgent, include_bio: bool| WorkspaceInspectAgentPreview {
        name: agent.name.clone(),
        bio: if include_bio {
            Some(agent.bio.clone())
        } else {
            None
        },
        provider: provider.clone(),
        model: agent.model.clone().unwrap_or_else(|| config.model.clone()),
    };
    Ok(WorkspaceInspectResponse {
        found: true,
        company_name: Some(config.name.clone()),
        mission: config
            .mission
            .clone()
            .or_else(|| Some(config.description.clone()))
            .filter(|mission| !mission.trim().is_empty()),
        values: config.values.clone(),
        orchestrator: Some(preview(&config.orchestrator, true)),
        workers: Some(
            config
                .agents
                .iter()
                .map(|agent| preview(agent, false))
                .collect(),
        ),
        provider_available: Some(provider_available),
    })
}

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

/// Adopt an existing workspace folder: parse its anima.yaml, create the
/// orchestrator and workers it describes, and mark the workspace configured.
/// Atomic, like bootstrap: all validation happens before any mutation, a
/// mid-batch creation failure rolls back the whole batch, and a persist
/// failure restores the previous state. The anima.yaml file is never
/// modified — resume does not own it.
///
/// Conflict + idempotency policy: a workspace already configured for a
/// DIFFERENT root is a 409. Otherwise (same root, or unconfigured) agents
/// whose names already exist are skipped — the persisted agent is kept — and
/// only the missing agents are created, so re-resuming a root restores what
/// is gone without duplicating what survives. When nothing needed creating at
/// all, resume returns 409 as a "nothing to do" signal.
pub(crate) async fn handle_resume_workspace(
    body: Vec<u8>,
    state: &SharedDaemonState,
) -> Result<WorkspaceResumeResponse, ApiError> {
    let request: WorkspaceResumeRequest = super::parse_json_body(body)?;

    // 1. Validate the folder WITHOUT creating it (validate_only=true): resume
    //    never creates directories; a missing folder or yaml is a 400.
    let root = validate_root_path(&request.root_path, true)?;
    let yaml_path = root.join("anima.yaml");
    if !yaml_path.is_file() {
        return Err(ApiError::bad_request(format!(
            "no anima.yaml found at {}",
            root.display()
        )));
    }
    let agency = load_agency_yaml(&yaml_path)?;

    // 2. Build the workspace config; the yaml owns these fields.
    let workspace_config = WorkspaceConfig {
        root_path: root,
        company_name: agency.name.trim().to_string(),
        mission: agency
            .mission
            .clone()
            .unwrap_or_else(|| agency.description.clone())
            .trim()
            .to_string(),
        values: agency.values.clone().unwrap_or_default(),
    };
    if workspace_config.mission.is_empty() {
        return Err(ApiError::bad_request_static(
            "anima.yaml: mission or description is required",
        ));
    }

    // 3. Build every agent's config up front (orchestrator first) so
    //    validation errors surface before any state mutation. Tool slugs are
    //    validated by create_agent against the registry before it inserts.
    let to_agent_config =
        |agent: &AgencyYamlAgent, is_orchestrator: bool| -> Result<AgentConfig, ApiError> {
            let name = agent.name.trim().to_string();
            if name.is_empty() {
                return Err(ApiError::bad_request_static(
                    "anima.yaml: agent name is required",
                ));
            }
            let model = agent
                .model
                .clone()
                .unwrap_or_else(|| agency.model.clone())
                .trim()
                .to_string();
            if model.is_empty() {
                return Err(ApiError::bad_request(format!(
                    "anima.yaml: agent {name} has no model and the file sets no default"
                )));
            }
            let bio = agent.bio.trim().to_string();
            if is_orchestrator && bio.is_empty() {
                return Err(ApiError::bad_request_static(
                    "anima.yaml: orchestrator bio is required",
                ));
            }
            // Trim-consistent with the loader's orchestrator.system check:
            // a whitespace-only worker system would resume an agent with no
            // instructions, so reject it naming the agent.
            let system = agent.system.trim().to_string();
            if system.is_empty() {
                return Err(ApiError::bad_request(format!(
                    "anima.yaml: agent {name} system is required"
                )));
            }
            Ok(AgentConfig {
                name,
                model,
                bio: if bio.is_empty() { None } else { Some(bio) },
                lore: agent.lore.clone(),
                knowledge: agent.knowledge.clone(),
                topics: agent.topics.clone(),
                adjectives: agent.adjectives.clone(),
                style: agent.style.clone(),
                provider: {
                    let provider = agency.provider.trim().to_string();
                    if provider.is_empty() {
                        None
                    } else {
                        Some(provider)
                    }
                },
                system: Some(system),
                tools: agent.tools.as_ref().map(|tools| {
                    tools
                        .iter()
                        .map(|tool| ToolDescriptor {
                            name: tool.clone(),
                            description: String::new(),
                            parameters_schema: BTreeMap::new(),
                            examples: None,
                        })
                        .collect::<Vec<_>>()
                }),
                plugins: None,
                settings: None,
            })
        };
    // Reject yaml-internal duplicate names before any mutation; create_agent
    // does not enforce name uniqueness, so without this a duplicated name
    // would silently create two agents.
    let mut seen_names = std::collections::BTreeSet::new();
    let agent_configs = std::iter::once((&agency.orchestrator, true))
        .chain(agency.agents.iter().map(|agent| (agent, false)))
        .map(|(agent, is_orchestrator)| {
            to_agent_config(agent, is_orchestrator).and_then(|config| {
                if seen_names.insert(config.name.clone()) {
                    Ok(config)
                } else {
                    Err(ApiError::bad_request(format!(
                        "anima.yaml: duplicate agent name '{}'",
                        config.name
                    )))
                }
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    // 4. Conflict + skip rules, resolved inside one read: a workspace
    //    configured for a different root is a 409. Otherwise agents whose
    //    names already exist are skipped (the persisted agent is kept) and
    //    only the missing ones are created.
    let orchestrator_name = agent_configs[0].name.clone();
    let (configs_to_create, skipped, orchestrator_existed) = {
        let guard = state.read().await;
        if let Some(current) = &guard.workspace {
            if current.root_path != workspace_config.root_path {
                return Err(ApiError::conflict(format!(
                    "workspace is already configured for {}",
                    current.root_path.display()
                )));
            }
        }
        let existing_names: std::collections::BTreeSet<String> = guard
            .list_agents()
            .iter()
            .map(|snapshot| snapshot.state.name.clone())
            .collect();
        let orchestrator_existed = existing_names.contains(&orchestrator_name);
        let mut skipped = Vec::new();
        let configs_to_create = agent_configs
            .into_iter()
            .filter(|config| {
                if existing_names.contains(&config.name) {
                    skipped.push(config.name.clone());
                    false
                } else {
                    true
                }
            })
            .collect::<Vec<_>>();
        (configs_to_create, skipped, orchestrator_existed)
    };

    // Idempotency signal: a re-resume with the full roster already live has
    // nothing to do.
    if configs_to_create.is_empty() {
        return Err(ApiError::conflict(
            "all agents from anima.yaml already exist",
        ));
    }

    // 5. One write guard: create the missing agents in order (orchestrator
    //    first when it was not skipped), tracking created ids so a failure
    //    mid-batch rolls back the whole batch (create_agent validates tool
    //    slugs before inserting, but an earlier agent in the batch would
    //    already exist).
    let mut created: Vec<(String, AgentRuntimeSnapshot)> = Vec::new();
    let (previous_workspace, persist_request) = {
        let mut guard = state.write().await;
        let mut failure: Option<ApiError> = None;
        for config in configs_to_create {
            let agent_name = config.name.clone();
            match guard.create_agent(config) {
                Ok(snapshot) => created.push((snapshot.state.id.clone(), snapshot)),
                Err(message) => {
                    failure = Some(ApiError::bad_request(format!(
                        "anima.yaml: agent {agent_name}: {message}"
                    )));
                    break;
                }
            }
        }
        match failure {
            Some(error) => {
                for (id, _) in &created {
                    guard.remove_agent(id);
                }
                return Err(error);
            }
            None => {
                let previous_workspace = guard.workspace.clone();
                guard.workspace = Some(workspace_config.clone());
                (previous_workspace, guard.control_plane_persist_request())
            }
        }
    };

    // 6. Persist; on failure roll back to the previous state (None for a
    //    fresh adopt). The yaml file is never touched.
    if let Err(error) = persist_request.save().await {
        rollback_resume(state, &created, previous_workspace).await;
        return Err(ApiError::service_unavailable(error.to_string()));
    }

    let mut snapshots = created.into_iter().map(|(_, snapshot)| snapshot);
    let orchestrator = if orchestrator_existed {
        // The orchestrator was skipped by the name-skip rule: report the
        // existing agent, not the first created worker.
        let guard = state.read().await;
        guard
            .list_agents()
            .into_iter()
            .find(|snapshot| snapshot.state.name == orchestrator_name)
            // Safe today: the route wrapper holds control_plane_transaction() for the
            // whole handler, and every live remove_agent path takes the same mutex.
            .ok_or_else(|| {
                ApiError::conflict(format!(
                    "orchestrator '{orchestrator_name}' was deleted concurrently; retry resume"
                ))
            })?
    } else {
        snapshots
            .next()
            .expect("fresh adopt always creates the orchestrator first")
    };
    Ok(WorkspaceResumeResponse {
        workspace: config_response(&workspace_config),
        orchestrator: AgentRuntimeSnapshotResponse::from(&orchestrator),
        workers: snapshots
            .map(|snapshot| AgentRuntimeSnapshotResponse::from(&snapshot))
            .collect(),
        skipped,
    })
}

/// Undo a partially applied resume: remove only the agents this resume
/// created and restore the previous workspace config (Some when re-resuming
/// an already-configured root, None for a fresh adopt). Never touches the
/// yaml file — resume does not own it.
async fn rollback_resume(
    state: &SharedDaemonState,
    created: &[(String, AgentRuntimeSnapshot)],
    previous_workspace: Option<WorkspaceConfig>,
) {
    let rollback_request = {
        let mut guard = state.write().await;
        for (id, _) in created {
            guard.remove_agent(id);
        }
        guard.workspace = previous_workspace;
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
