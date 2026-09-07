use std::io::Write;
use std::path::{Path, PathBuf};

use anima_core::AgentConfigUpdate;
use atomicwrites::{AllowOverwrite, AtomicFile};
use serde_yaml::Value;

use super::ApiError;

pub(super) struct WorkspaceAgentYamlUpdate {
    path: PathBuf,
    before: String,
    after: String,
}

impl WorkspaceAgentYamlUpdate {
    pub(super) fn prepare(
        root: &Path,
        previous_name: &str,
        patch: &mut AgentConfigUpdate,
    ) -> Result<Option<Self>, ApiError> {
        if patch.name.is_none() && patch.model.is_none() && patch.provider.is_none() {
            return Ok(None);
        }
        let path = root.join("anima.yaml");
        let before = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(sync_error(error)),
        };
        // Validate the supported contract, but mutate the generic value so custom
        // fields and unrelated agents survive serialization.
        serde_yaml::from_str::<super::agencies::AgencyYamlConfig>(&before).map_err(sync_error)?;
        let mut yaml: Value = serde_yaml::from_str(&before).map_err(sync_error)?;
        let mut matches = Vec::new();
        let mut names = Vec::new();
        for (index, agent) in std::iter::once(&yaml["orchestrator"])
            .chain(yaml["agents"].as_sequence().into_iter().flatten())
            .enumerate()
        {
            let name = agent["name"].as_str().unwrap_or_default().trim();
            names.push(name.to_lowercase());
            if name == previous_name.trim() {
                matches.push(index);
            }
        }
        if matches.is_empty() {
            return Ok(None); // Agents created outside the workspace roster remain runtime-only.
        }
        if matches.len() != 1 {
            return Err(ApiError::conflict(
                "anima.yaml contains ambiguous agent names",
            ));
        }
        let index = matches[0];
        if let Some(name) = &patch.name {
            if names
                .iter()
                .enumerate()
                .any(|(other, existing)| other != index && existing == &name.trim().to_lowercase())
            {
                return Err(ApiError::conflict(
                    "anima.yaml already contains the new agent name",
                ));
            }
        }
        let default_provider = yaml["provider"]
            .as_str()
            .map(str::trim)
            .filter(|provider| !provider.is_empty())
            .unwrap_or("openai")
            .to_string();
        let agent = if index == 0 {
            &mut yaml["orchestrator"]
        } else {
            &mut yaml["agents"][index - 1]
        };
        if let Some(name) = &patch.name {
            agent["name"] = Value::String(name.clone());
        }
        if let Some(model) = &patch.model {
            agent["model"] = Value::String(model.clone());
        }
        if let Some(provider) = &patch.provider {
            if provider.is_empty() {
                agent
                    .as_mapping_mut()
                    .expect("validated agent mapping")
                    .remove(Value::String("provider".into()));
                patch.provider = Some(default_provider);
            } else {
                agent["provider"] = Value::String(provider.clone());
            }
        }
        let after = serde_yaml::to_string(&yaml).map_err(sync_error)?;
        Ok(Some(Self {
            path,
            before,
            after,
        }))
    }

    pub(super) fn apply(&self) -> Result<(), ApiError> {
        self.replace(&self.before, &self.after)
    }

    pub(super) fn rollback(&self) -> Result<(), ApiError> {
        self.replace(&self.after, &self.before)
    }

    fn replace(&self, expected: &str, replacement: &str) -> Result<(), ApiError> {
        if std::fs::read_to_string(&self.path).map_err(sync_error)? != expected {
            return Err(ApiError::conflict(
                "anima.yaml changed during the agent update; reload and try again",
            ));
        }
        AtomicFile::new(&self.path, AllowOverwrite)
            .write(|file| {
                file.write_all(replacement.as_bytes())?;
                file.sync_all()
            })
            .map_err(sync_error)
    }
}

fn sync_error(error: impl std::fmt::Display) -> ApiError {
    ApiError::service_unavailable(format!("Could not synchronize anima.yaml: {error}"))
}
