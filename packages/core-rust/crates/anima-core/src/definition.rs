use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::capability::ManifestCatalog;

pub const SUPPORTED_DEFINITION_SCHEMA_VERSION: u32 = 1;

/// An exact versioned profile reference.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileRef {
    pub profile_id: String,
    pub profile_version: u32,
}

/// A definition-local configuration override for a single exact manifest version.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CapabilityOverride {
    pub capability_id: String,
    pub manifest_version: u32,
    pub configuration: Value,
}

/// The durable capability pin stored in a published definition.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResolvedCapability {
    pub capability_id: String,
    pub manifest_version: u32,
    pub schema_digest: String,
    pub override_config: Option<CapabilityOverride>,
    pub approval_policy_revision: u32,
}

/// A model selection policy that contains a credential reference, never credentials.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelPolicy {
    pub provider: String,
    pub model: String,
    pub credential_reference: Option<String>,
    pub temperature: Option<f64>,
}

/// Memory scope and retention policy for an agent definition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryPolicy {
    pub enabled: bool,
    pub namespace: String,
    pub retention_days: Option<u32>,
}

/// Limits applied to one runtime definition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeLimits {
    pub max_turns: u32,
    pub timeout_ms: u64,
    pub max_concurrent_tasks: u32,
}

/// Lifecycle behavior requested from a host for an agent definition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecyclePolicy {
    pub auto_start: bool,
    pub restart_on_failure: bool,
    pub max_restarts: u32,
}

/// An exact host feature/revision a definition requires, independent of an executor implementation.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct HostRequirement {
    pub id: String,
    pub revision: u32,
}

/// The mutable input used to publish a durable, resolved definition.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentDefinitionDraft {
    pub schema_version: u32,
    pub id: String,
    pub version: u32,
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub persona: String,
    pub system: String,
    pub model: ModelPolicy,
    pub source_profile: ProfileRef,
    pub capability_overrides: Vec<CapabilityOverride>,
    pub memory: MemoryPolicy,
    pub approval_policy_id: String,
    pub approval_policy_revision: u32,
    pub approval_restrictions: Vec<String>,
    pub limits: RuntimeLimits,
    pub lifecycle: LifecyclePolicy,
    pub host_requirements: Vec<HostRequirement>,
}

/// An immutable, self-contained definition suitable for durable storage and host consumption.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentDefinition {
    pub schema_version: u32,
    pub id: String,
    pub version: u32,
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub persona: String,
    pub system: String,
    pub model: ModelPolicy,
    pub source_profile: ProfileRef,
    pub resolved_capabilities: Vec<ResolvedCapability>,
    pub memory: MemoryPolicy,
    pub approval_policy_id: String,
    pub approval_policy_revision: u32,
    pub approval_restrictions: Vec<String>,
    pub limits: RuntimeLimits,
    pub lifecycle: LifecyclePolicy,
    pub host_requirements: Vec<HostRequirement>,
}

/// Errors produced while resolving and publishing agent definitions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DefinitionValidationError {
    UnsupportedSchemaVersion {
        schema_version: u32,
    },
    UnknownProfile {
        id: String,
        version: u32,
    },
    UnknownManifest {
        id: String,
        version: u32,
    },
    DuplicateCapabilityId {
        id: String,
    },
    MissingHostRequirement {
        id: String,
        revision: u32,
    },
    NonIncreasingVersion {
        id: String,
        previous_version: u32,
        attempted_version: u32,
    },
}

impl fmt::Display for DefinitionValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSchemaVersion { schema_version } => {
                write!(
                    formatter,
                    "definition schema version {schema_version} is unsupported"
                )
            }
            Self::UnknownProfile { id, version } => {
                write!(formatter, "profile {id}@{version} is not registered")
            }
            Self::UnknownManifest { id, version } => {
                write!(formatter, "manifest {id}@{version} is not registered")
            }
            Self::DuplicateCapabilityId { id } => {
                write!(formatter, "capability {id} resolves more than once")
            }
            Self::MissingHostRequirement { id, revision } => {
                write!(formatter, "host requirement {id}@{revision} is unavailable")
            }
            Self::NonIncreasingVersion {
                id,
                previous_version,
                attempted_version,
            } => write!(
                formatter,
                "definition {id} version {attempted_version} must exceed {previous_version}"
            ),
        }
    }
}

impl std::error::Error for DefinitionValidationError {}

/// Resolves drafts through a portable catalog and retains owned published snapshots.
#[derive(Clone, Debug, Default)]
pub struct DefinitionPublisher {
    available_host_requirements: BTreeSet<HostRequirement>,
    definitions: BTreeMap<(String, u32), AgentDefinition>,
    latest_versions: BTreeMap<String, u32>,
}

impl DefinitionPublisher {
    pub fn new(host_requirements: impl IntoIterator<Item = HostRequirement>) -> Self {
        Self {
            available_host_requirements: host_requirements.into_iter().collect(),
            ..Self::default()
        }
    }

    pub fn publish(
        &mut self,
        catalog: &ManifestCatalog,
        draft: AgentDefinitionDraft,
    ) -> Result<AgentDefinition, DefinitionValidationError> {
        if draft.schema_version != SUPPORTED_DEFINITION_SCHEMA_VERSION {
            return Err(DefinitionValidationError::UnsupportedSchemaVersion {
                schema_version: draft.schema_version,
            });
        }
        if let Some(previous_version) = self.latest_versions.get(&draft.id) {
            if draft.version <= *previous_version {
                return Err(DefinitionValidationError::NonIncreasingVersion {
                    id: draft.id,
                    previous_version: *previous_version,
                    attempted_version: draft.version,
                });
            }
        }
        for requirement in &draft.host_requirements {
            if !self.available_host_requirements.contains(requirement) {
                return Err(DefinitionValidationError::MissingHostRequirement {
                    id: requirement.id.clone(),
                    revision: requirement.revision,
                });
            }
        }

        let profile = catalog
            .profile(
                &draft.source_profile.profile_id,
                draft.source_profile.profile_version,
            )
            .ok_or_else(|| DefinitionValidationError::UnknownProfile {
                id: draft.source_profile.profile_id.clone(),
                version: draft.source_profile.profile_version,
            })?;

        let mut overrides = BTreeMap::new();
        for override_config in draft.capability_overrides.iter().cloned() {
            let capability_id = override_config.capability_id.clone();
            if overrides
                .insert(capability_id.clone(), override_config)
                .is_some()
            {
                return Err(DefinitionValidationError::DuplicateCapabilityId { id: capability_id });
            }
        }

        let mut requested = Vec::with_capacity(profile.entries.len() + overrides.len());
        let mut capability_ids = BTreeSet::new();
        for entry in &profile.entries {
            if !capability_ids.insert(entry.capability_id.clone()) {
                return Err(DefinitionValidationError::DuplicateCapabilityId {
                    id: entry.capability_id.clone(),
                });
            }
            if let Some(override_config) = overrides.remove(&entry.capability_id) {
                requested.push((
                    entry.capability_id.clone(),
                    override_config.manifest_version,
                    Some(override_config),
                ));
            } else {
                requested.push((entry.capability_id.clone(), entry.manifest_version, None));
            }
        }
        requested.extend(overrides.into_values().map(|override_config| {
            (
                override_config.capability_id.clone(),
                override_config.manifest_version,
                Some(override_config),
            )
        }));

        let mut resolved_capabilities = Vec::with_capacity(requested.len());
        for (capability_id, manifest_version, override_config) in requested {
            let manifest = catalog
                .manifest(&capability_id, manifest_version)
                .ok_or_else(|| DefinitionValidationError::UnknownManifest {
                    id: capability_id.clone(),
                    version: manifest_version,
                })?;
            resolved_capabilities.push(ResolvedCapability {
                capability_id,
                manifest_version,
                schema_digest: manifest.schema_digest.clone(),
                override_config,
                approval_policy_revision: draft.approval_policy_revision,
            });
        }
        resolved_capabilities.sort_by(|left, right| left.capability_id.cmp(&right.capability_id));

        let definition = AgentDefinition {
            schema_version: draft.schema_version,
            id: draft.id,
            version: draft.version,
            name: draft.name,
            display_name: draft.display_name,
            description: draft.description,
            persona: draft.persona,
            system: draft.system,
            model: draft.model,
            source_profile: draft.source_profile,
            resolved_capabilities,
            memory: draft.memory,
            approval_policy_id: draft.approval_policy_id,
            approval_policy_revision: draft.approval_policy_revision,
            approval_restrictions: draft.approval_restrictions,
            limits: draft.limits,
            lifecycle: draft.lifecycle,
            host_requirements: draft.host_requirements,
        };
        self.latest_versions
            .insert(definition.id.clone(), definition.version);
        self.definitions.insert(
            (definition.id.clone(), definition.version),
            definition.clone(),
        );
        Ok(definition)
    }

    pub fn definition(&self, id: &str, version: u32) -> Option<&AgentDefinition> {
        self.definitions.get(&(id.to_owned(), version))
    }
}
