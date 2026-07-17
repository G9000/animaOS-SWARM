#[cfg(test)]
mod tests;

use anima_core::{AgentConfig, ModelAdapter, ModelGenerateRequest, ModelGenerateResponse};
use anima_model_adapters::{
    provider_definitions, ProviderAdapterConfig, ProviderCredential, ProviderModelAdapter,
};
use async_trait::async_trait;

use crate::model::DeterministicModelAdapter;

fn first_env_value(names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .and_then(|value| (!value.trim().is_empty()).then_some(value))
    })
}

fn provider_config_from_env() -> ProviderAdapterConfig {
    ProviderAdapterConfig {
        providers: provider_definitions()
            .iter()
            .map(|definition| {
                let api_key = first_env_value(definition.api_key_envs);
                let base_url = first_env_value(definition.base_url_envs)
                    .unwrap_or_else(|| definition.default_base_url.to_owned());
                (
                    definition.id.to_owned(),
                    ProviderCredential { api_key, base_url },
                )
            })
            .collect(),
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ProviderSummary {
    pub id: &'static str,
    pub label: &'static str,
    pub requires_key: bool,
    pub configured: bool,
    pub api_key_envs: &'static [&'static str],
}

pub(crate) fn provider_summaries() -> Vec<ProviderSummary> {
    provider_summaries_for_config(&provider_config_from_env())
}

fn provider_summaries_for_config(config: &ProviderAdapterConfig) -> Vec<ProviderSummary> {
    let mut summaries = Vec::with_capacity(provider_definitions().len() + 1);
    summaries.push(ProviderSummary {
        id: "deterministic",
        label: "Deterministic (mock)",
        requires_key: false,
        configured: true,
        api_key_envs: &[],
    });
    summaries.extend(provider_definitions().iter().map(|definition| {
        let configured = !definition.requires_key
            || config
                .providers
                .get(definition.id)
                .and_then(|credential| credential.api_key.as_deref())
                .is_some_and(|api_key| !api_key.trim().is_empty());
        ProviderSummary {
            id: definition.id,
            label: definition.label,
            requires_key: definition.requires_key,
            configured,
            api_key_envs: definition.api_key_envs,
        }
    }));
    summaries
}

#[derive(Clone)]
pub(crate) struct RuntimeModelAdapter {
    providers: ProviderModelAdapter,
}

impl RuntimeModelAdapter {
    pub(crate) fn from_env() -> Self {
        Self::from_config(provider_config_from_env())
    }

    #[cfg(test)]
    fn with_config(config: ProviderAdapterConfig) -> Self {
        Self::from_config(config)
    }

    fn from_config(config: ProviderAdapterConfig) -> Self {
        Self {
            providers: ProviderModelAdapter::new(config),
        }
    }
}

#[async_trait]
impl ModelAdapter for RuntimeModelAdapter {
    fn provider(&self) -> &str {
        "runtime"
    }

    async fn generate(
        &self,
        config: &AgentConfig,
        request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String> {
        let provider = config
            .provider
            .as_deref()
            .map(str::trim)
            .filter(|provider| !provider.is_empty())
            .unwrap_or("deterministic")
            .to_ascii_lowercase();

        match provider.as_str() {
            "deterministic" | "test" => DeterministicModelAdapter.generate(config, request).await,
            _ => self.providers.generate(config, request).await,
        }
    }
}
