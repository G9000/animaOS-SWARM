use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceConfigResponse {
    pub(crate) root_path: String,
    pub(crate) company_name: String,
    pub(crate) mission: String,
    pub(crate) values: Vec<String>,
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
// Consumed by PUT /api/workspace and POST /api/workspace/bootstrap (Tasks 4/6).
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
