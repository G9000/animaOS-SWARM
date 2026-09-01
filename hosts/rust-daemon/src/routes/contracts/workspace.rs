use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceInspectAgentPreview {
    pub(crate) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) bio: Option<String>, // orchestrator only; workers omit bio/system
    pub(crate) provider: String,
    pub(crate) model: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceInspectResponse {
    pub(crate) found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) company_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) mission: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) values: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) orchestrator: Option<WorkspaceInspectAgentPreview>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) workers: Option<Vec<WorkspaceInspectAgentPreview>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) provider_available: Option<bool>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct WorkspaceInspectQuery {
    // Defaulted so a fully-absent rootPath deserializes to "" and reaches the
    // handler's "rootPath is required" 400, instead of axum rejecting the
    // query with a plain-text QueryRejection that bypasses the ErrorBody
    // envelope.
    #[serde(rename = "rootPath", default)]
    pub(crate) root_path: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceConfigResponse {
    pub(crate) root_path: String,
    pub(crate) company_name: String,
    pub(crate) mission: String,
    pub(crate) values: Vec<String>,
    pub(crate) has_avatar: bool,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceResponse {
    pub(crate) configured: bool,
    pub(crate) workspace: Option<WorkspaceConfigResponse>,
    /// Effective root the daemon would use without configuration
    /// (env var or launch directory). Lets onboarding pre-fill the field.
    pub(crate) default_root: String,
    /// Present only in validate-only responses: whether the validated root
    /// already exists on disk. Drives the "will be created" UI copy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) root_path_exists: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
// Consumed by PUT /api/workspace and POST /api/workspace/bootstrap.
pub(crate) struct WorkspaceConfigRequest {
    pub(crate) root_path: String,
    pub(crate) company_name: String,
    pub(crate) mission: String,
    #[serde(default)]
    pub(crate) values: Vec<String>,
    /// When true, validate only — do not persist or mutate daemon state.
    #[serde(default)]
    pub(crate) validate_only: bool,
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BootstrapAgentRequest {
    pub(crate) name: String,
    pub(crate) preset_id: String,
    #[serde(default)]
    pub(crate) bio: Option<String>,
    #[serde(default)]
    pub(crate) adjectives: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) style: Option<String>,
    pub(crate) system: String,
    #[serde(default)]
    pub(crate) provider: Option<String>,
    pub(crate) model: String,
    #[serde(default)]
    pub(crate) tools: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceBootstrapRequest {
    pub(crate) workspace: WorkspaceConfigRequest,
    pub(crate) agent: BootstrapAgentRequest,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceBootstrapResponse {
    pub(crate) workspace: WorkspaceConfigResponse,
    pub(crate) agent: super::AgentRuntimeSnapshotResponse,
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceResumeRequest {
    pub(crate) root_path: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceResumeResponse {
    pub(crate) workspace: WorkspaceConfigResponse,
    pub(crate) orchestrator: super::AgentRuntimeSnapshotResponse,
    pub(crate) workers: Vec<super::AgentRuntimeSnapshotResponse>,
    /// Names kept because agents with those names already existed (skip rule).
    pub(crate) skipped: Vec<String>,
}
