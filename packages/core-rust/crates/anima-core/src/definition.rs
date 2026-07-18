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
///
/// Overrides that do not name a capability in the source profile intentionally add that capability
/// to the resolved definition, provided the exact manifest version exists in the catalog.
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
    #[serde(with = "finite_option_f64")]
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
    #[serde(default)]
    pub allows_concurrent_sessions: bool,
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
///
/// Resolved capabilities are canonically ordered by `capability_id`; deserialization rejects
/// otherwise-valid snapshots with a noncanonical order so tampering is observable.
#[derive(Clone, Debug, PartialEq, Serialize)]
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

#[derive(Deserialize)]
struct AgentDefinitionWire {
    schema_version: u32,
    id: String,
    version: u32,
    name: String,
    display_name: String,
    description: String,
    persona: String,
    system: String,
    model: ModelPolicy,
    source_profile: ProfileRef,
    resolved_capabilities: Vec<ResolvedCapability>,
    memory: MemoryPolicy,
    approval_policy_id: String,
    approval_policy_revision: u32,
    approval_restrictions: Vec<String>,
    limits: RuntimeLimits,
    lifecycle: LifecyclePolicy,
    host_requirements: Vec<HostRequirement>,
}

/// Errors produced while resolving and publishing agent definitions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    InvalidTemperature,
    InvalidDefinitionId,
    InvalidDefinitionVersion,
    InvalidProfileReference,
    InvalidResolvedCapability {
        id: String,
    },
    InvalidSchemaDigest {
        id: String,
    },
    InconsistentOverride {
        id: String,
    },
    InconsistentPolicyRevision {
        id: String,
    },
    InvalidApprovalPolicy,
    InvalidHostRequirement {
        id: String,
        revision: u32,
    },
    DuplicateHostRequirement {
        id: String,
        revision: u32,
    },
    NonCanonicalCapabilityOrder,
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
            Self::InvalidTemperature => write!(formatter, "model temperature must be finite"),
            Self::InvalidDefinitionId => write!(formatter, "definition ID must not be blank"),
            Self::InvalidDefinitionVersion => {
                write!(formatter, "definition version must be nonzero")
            }
            Self::InvalidProfileReference => {
                write!(formatter, "profile ID and version must be present")
            }
            Self::InvalidResolvedCapability { id } => {
                write!(formatter, "resolved capability {id:?} has invalid pins")
            }
            Self::InvalidSchemaDigest { id } => {
                write!(
                    formatter,
                    "resolved capability {id:?} has a blank schema digest"
                )
            }
            Self::InconsistentOverride { id } => {
                write!(
                    formatter,
                    "override does not match resolved capability {id}"
                )
            }
            Self::InconsistentPolicyRevision { id } => {
                write!(
                    formatter,
                    "resolved capability {id} has an inconsistent policy revision"
                )
            }
            Self::InvalidApprovalPolicy => {
                write!(formatter, "approval policy ID and revision must be present")
            }
            Self::InvalidHostRequirement { id, revision } => {
                write!(formatter, "host requirement {id}@{revision} is invalid")
            }
            Self::DuplicateHostRequirement { id, revision } => {
                write!(formatter, "host requirement {id}@{revision} is duplicated")
            }
            Self::NonCanonicalCapabilityOrder => {
                write!(
                    formatter,
                    "resolved capabilities must be ordered by capability ID"
                )
            }
        }
    }
}

impl std::error::Error for DefinitionValidationError {}

impl AgentDefinition {
    /// Validates the portable pins and policies stored in this already-resolved definition.
    pub fn validate(&self) -> Result<(), DefinitionValidationError> {
        validate_definition(self)
    }
}

impl<'de> Deserialize<'de> for AgentDefinition {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = AgentDefinitionWire::deserialize(deserializer)?;
        let mut definition = Self {
            schema_version: wire.schema_version,
            id: wire.id,
            version: wire.version,
            name: wire.name,
            display_name: wire.display_name,
            description: wire.description,
            persona: wire.persona,
            system: wire.system,
            model: wire.model,
            source_profile: wire.source_profile,
            resolved_capabilities: wire.resolved_capabilities,
            memory: wire.memory,
            approval_policy_id: wire.approval_policy_id,
            approval_policy_revision: wire.approval_policy_revision,
            approval_restrictions: wire.approval_restrictions,
            limits: wire.limits,
            lifecycle: wire.lifecycle,
            host_requirements: wire.host_requirements,
        };
        canonicalize_host_requirements(&mut definition.host_requirements)
            .map_err(serde::de::Error::custom)?;
        definition.validate().map_err(serde::de::Error::custom)?;
        Ok(definition)
    }
}

/// Resolves drafts through a portable catalog and retains owned published snapshots.
///
/// This process-local service deliberately does not implement `Serialize`: its publication history
/// and available host requirements are live process state. Portable data contracts are instead
/// `AgentDefinitionDraft`, `AgentDefinition`, and `DefinitionValidationError`; the catalog stays an
/// explicit argument to [`Self::publish`] so no live catalog reference is captured or persisted.
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
        mut draft: AgentDefinitionDraft,
    ) -> Result<AgentDefinition, DefinitionValidationError> {
        validate_draft(&draft)?;
        canonicalize_host_requirements(&mut draft.host_requirements)?;
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
        definition.validate()?;
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

fn validate_draft(draft: &AgentDefinitionDraft) -> Result<(), DefinitionValidationError> {
    if draft
        .model
        .temperature
        .is_some_and(|value| !value.is_finite())
    {
        return Err(DefinitionValidationError::InvalidTemperature);
    }
    if draft.id.trim().is_empty() {
        return Err(DefinitionValidationError::InvalidDefinitionId);
    }
    if draft.version == 0 {
        return Err(DefinitionValidationError::InvalidDefinitionVersion);
    }
    if draft.source_profile.profile_id.trim().is_empty()
        || draft.source_profile.profile_version == 0
    {
        return Err(DefinitionValidationError::InvalidProfileReference);
    }
    if draft.approval_policy_id.trim().is_empty() || draft.approval_policy_revision == 0 {
        return Err(DefinitionValidationError::InvalidApprovalPolicy);
    }
    Ok(())
}

fn validate_definition(definition: &AgentDefinition) -> Result<(), DefinitionValidationError> {
    if definition.schema_version != SUPPORTED_DEFINITION_SCHEMA_VERSION {
        return Err(DefinitionValidationError::UnsupportedSchemaVersion {
            schema_version: definition.schema_version,
        });
    }
    if definition
        .model
        .temperature
        .is_some_and(|value| !value.is_finite())
    {
        return Err(DefinitionValidationError::InvalidTemperature);
    }
    if definition.id.trim().is_empty() {
        return Err(DefinitionValidationError::InvalidDefinitionId);
    }
    if definition.version == 0 {
        return Err(DefinitionValidationError::InvalidDefinitionVersion);
    }
    if definition.source_profile.profile_id.trim().is_empty()
        || definition.source_profile.profile_version == 0
    {
        return Err(DefinitionValidationError::InvalidProfileReference);
    }
    if definition.approval_policy_id.trim().is_empty() || definition.approval_policy_revision == 0 {
        return Err(DefinitionValidationError::InvalidApprovalPolicy);
    }
    let mut capability_ids = BTreeSet::new();
    let mut previous_capability_id: Option<&str> = None;
    for capability in &definition.resolved_capabilities {
        if previous_capability_id
            .is_some_and(|previous| previous >= capability.capability_id.as_str())
        {
            return Err(DefinitionValidationError::NonCanonicalCapabilityOrder);
        }
        previous_capability_id = Some(&capability.capability_id);
        if capability.capability_id.trim().is_empty()
            || capability.manifest_version == 0
            || !capability_ids.insert(capability.capability_id.clone())
        {
            return Err(DefinitionValidationError::InvalidResolvedCapability {
                id: capability.capability_id.clone(),
            });
        }
        if capability.schema_digest.trim().is_empty() {
            return Err(DefinitionValidationError::InvalidSchemaDigest {
                id: capability.capability_id.clone(),
            });
        }
        if capability.approval_policy_revision == 0
            || capability.approval_policy_revision != definition.approval_policy_revision
        {
            return Err(DefinitionValidationError::InconsistentPolicyRevision {
                id: capability.capability_id.clone(),
            });
        }
        if let Some(override_config) = &capability.override_config {
            if override_config.capability_id != capability.capability_id
                || override_config.manifest_version != capability.manifest_version
            {
                return Err(DefinitionValidationError::InconsistentOverride {
                    id: capability.capability_id.clone(),
                });
            }
        }
    }
    validate_host_requirements(&definition.host_requirements)
}

fn canonicalize_host_requirements(
    requirements: &mut Vec<HostRequirement>,
) -> Result<(), DefinitionValidationError> {
    validate_host_requirements(requirements)?;
    requirements.sort();
    Ok(())
}

fn validate_host_requirements(
    requirements: &[HostRequirement],
) -> Result<(), DefinitionValidationError> {
    let mut unique = BTreeSet::new();
    for requirement in requirements {
        if requirement.id.trim().is_empty() || requirement.revision == 0 {
            return Err(DefinitionValidationError::InvalidHostRequirement {
                id: requirement.id.clone(),
                revision: requirement.revision,
            });
        }
        if !unique.insert(requirement.clone()) {
            return Err(DefinitionValidationError::DuplicateHostRequirement {
                id: requirement.id.clone(),
                revision: requirement.revision,
            });
        }
    }
    Ok(())
}

mod finite_option_f64 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &Option<f64>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(value) if value.is_finite() => serializer.serialize_some(value),
            Some(_) => Err(serde::ser::Error::custom(
                "model temperature must be finite",
            )),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Option::<f64>::deserialize(deserializer)?;
        if value.is_some_and(|value| !value.is_finite()) {
            return Err(serde::de::Error::custom("model temperature must be finite"));
        }
        Ok(value)
    }
}
