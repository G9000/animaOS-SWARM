use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The broad role a capability plays for an agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CapabilityKind {
    Knowledge,
    Workspace,
    Communication,
    Automation,
    Custom,
}

/// The expected harm if a capability is used incorrectly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RiskLevel {
    None,
    Low,
    Medium,
    High,
    Critical,
}

/// The declared recovery path after a capability invocation fails.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecoveryMode {
    None,
    Retry,
    Compensate,
    Manual,
}

/// Compatibility boundaries between a manifest and a runtime schema.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeCompatibility {
    pub minimum_runtime_schema_version: u32,
    pub maximum_runtime_schema_version: u32,
    pub manifest_schema_version: u32,
}

/// A portable, host-independent description of an agent capability.
///
/// Secret-related fields carry reference names only; this vocabulary never stores secret values.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CapabilityManifest {
    pub id: String,
    pub version: u32,
    pub kind: CapabilityKind,
    pub label: String,
    pub description: String,
    pub input_schema: Value,
    pub output_schema: Value,
    pub side_effects: bool,
    pub risk_level: RiskLevel,
    pub host_permissions: Vec<String>,
    pub secret_references: Vec<String>,
    pub environment_requirements: Vec<String>,
    pub timeout_ms: u64,
    pub cancellation_supported: bool,
    pub max_retries: u32,
    pub idempotent: bool,
    pub recovery_mode: RecoveryMode,
    pub supports_streaming: bool,
    pub supports_artifacts: bool,
    pub supports_citations: bool,
    pub schema_digest: String,
    pub compatibility: RuntimeCompatibility,
}

/// A versioned reference to a manifest included in a capability profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityProfileEntry {
    pub capability_id: String,
    pub manifest_version: u32,
}

/// A portable, versioned grouping of capability manifests.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityProfile {
    pub id: String,
    pub version: u32,
    pub label: String,
    pub description: String,
    pub entries: Vec<CapabilityProfileEntry>,
}

/// Errors produced while registering or resolving portable catalog records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManifestCatalogError {
    DuplicateManifest { id: String, version: u32 },
    DuplicateProfile { id: String, version: u32 },
    UnknownManifest { id: String, version: u32 },
}

impl fmt::Display for ManifestCatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateManifest { id, version } => {
                write!(formatter, "manifest {id}@{version} is already registered")
            }
            Self::DuplicateProfile { id, version } => {
                write!(formatter, "profile {id}@{version} is already registered")
            }
            Self::UnknownManifest { id, version } => {
                write!(formatter, "manifest {id}@{version} is not registered")
            }
        }
    }
}

impl std::error::Error for ManifestCatalogError {}

/// An owned, executor-free catalog of portable manifest and profile versions.
#[derive(Clone, Debug, Default)]
pub struct ManifestCatalog {
    manifests: BTreeMap<(String, u32), CapabilityManifest>,
    profiles: BTreeMap<(String, u32), CapabilityProfile>,
}

impl ManifestCatalog {
    pub fn register_manifest(
        &mut self,
        manifest: CapabilityManifest,
    ) -> Result<(), ManifestCatalogError> {
        let key = (manifest.id.clone(), manifest.version);
        if self.manifests.contains_key(&key) {
            return Err(ManifestCatalogError::DuplicateManifest {
                id: manifest.id,
                version: manifest.version,
            });
        }

        self.manifests.insert(key, manifest);
        Ok(())
    }

    pub fn register_profile(
        &mut self,
        mut profile: CapabilityProfile,
    ) -> Result<(), ManifestCatalogError> {
        let key = (profile.id.clone(), profile.version);
        if self.profiles.contains_key(&key) {
            return Err(ManifestCatalogError::DuplicateProfile {
                id: profile.id,
                version: profile.version,
            });
        }

        for entry in &profile.entries {
            if self
                .manifest(&entry.capability_id, entry.manifest_version)
                .is_none()
            {
                return Err(ManifestCatalogError::UnknownManifest {
                    id: entry.capability_id.clone(),
                    version: entry.manifest_version,
                });
            }
        }
        profile.entries.sort_by(|left, right| {
            left.capability_id
                .cmp(&right.capability_id)
                .then(left.manifest_version.cmp(&right.manifest_version))
        });

        self.profiles.insert(key, profile);
        Ok(())
    }

    pub fn manifest(&self, id: &str, version: u32) -> Option<&CapabilityManifest> {
        self.manifests.get(&(id.to_owned(), version))
    }

    pub fn profile(&self, id: &str, version: u32) -> Option<&CapabilityProfile> {
        self.profiles.get(&(id.to_owned(), version))
    }
}
