use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgencyGenerateRequest {
    pub(crate) name: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) team_size: Option<u64>,
    pub(crate) provider: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) model_pool: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgencyCreateRequest {
    pub(crate) name: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) team_size: Option<u64>,
    pub(crate) provider: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) model_pool: Option<Vec<String>>,
    pub(crate) output_dir: Option<String>,
    #[serde(default)]
    pub(crate) seed_memories: bool,
    #[serde(default)]
    pub(crate) overwrite: bool,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentDefinitionResponse {
    pub(crate) name: String,
    pub(crate) position: Option<String>,
    pub(crate) role: String,
    pub(crate) bio: Option<String>,
    pub(crate) lore: Option<String>,
    pub(crate) adjectives: Option<Vec<String>>,
    pub(crate) topics: Option<Vec<String>>,
    pub(crate) knowledge: Option<Vec<String>>,
    pub(crate) style: Option<String>,
    pub(crate) system: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) tools: Option<Vec<String>>,
    pub(crate) collaborates_with: Option<Vec<String>>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgencyGenerateResponse {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) team_size: u64,
    pub(crate) mission: Option<String>,
    pub(crate) values: Option<Vec<String>>,
    pub(crate) agents: Vec<AgentDefinitionResponse>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgencyCreateResponse {
    pub(crate) agency: AgencyGenerateResponse,
    pub(crate) output_dir: String,
    pub(crate) files: Vec<String>,
    pub(crate) seed_memory_count: u64,
    pub(crate) seeded_agents: u64,
}
