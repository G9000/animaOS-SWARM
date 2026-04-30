use serde::Serialize;
use utoipa::ToSchema;

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderResponse {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) requires_key: bool,
    pub(crate) configured: bool,
    pub(crate) api_key_envs: Vec<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct ProvidersEnvelope {
    pub(crate) providers: Vec<ProviderResponse>,
}
