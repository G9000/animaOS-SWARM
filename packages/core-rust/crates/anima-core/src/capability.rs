use std::collections::BTreeMap;
use std::fmt;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use async_trait::async_trait;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use uuid::Uuid;

#[path = "capability/invocation.rs"]
mod invocation;
use invocation::{canonicalize_arguments, validate_argument_bounds};
#[path = "capability/manifest.rs"]
mod manifest;
use manifest::validate_manifest;
#[path = "capability/schema.rs"]
mod schema;
use schema::{compile_schema, validate_instance};

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
    /// A repeated call has the same effect without an externally supplied key.
    InherentlyIdempotent,
    /// A repeated call is safe only when it keeps the logical invocation key.
    KeyedIdempotent,
    /// The executor can determine the authoritative outcome before retrying.
    Reconcilable,
    /// An uncertain invocation must be reviewed by a recovery workflow.
    NonRetryable,
    /// Legacy portable manifests may use this generic retry declaration.
    None,
    /// Legacy portable manifests may use this generic retry declaration.
    Retry,
    /// Legacy portable manifests may use this compensation declaration.
    Compensate,
    /// Legacy portable manifests may require manual handling.
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
    InvalidManifestId,
    InvalidManifestVersion,
    InvalidManifestSchemaVersion,
    InvalidSchemaDigest,
    InvalidSecretReferenceName,
    TooManySecretReferences,
    InvalidRuntimeCompatibility,
    InvalidProfileId,
    InvalidProfileVersion,
    DuplicateProfileCapabilityId { id: String },
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
            Self::InvalidManifestId => write!(formatter, "manifest ID must not be blank"),
            Self::InvalidManifestVersion => write!(formatter, "manifest version must be nonzero"),
            Self::InvalidManifestSchemaVersion => {
                write!(formatter, "manifest schema version must be nonzero")
            }
            Self::InvalidSchemaDigest => {
                write!(formatter, "manifest schema digest must not be blank")
            }
            Self::InvalidSecretReferenceName => {
                write!(formatter, "manifest secret reference names are invalid")
            }
            Self::TooManySecretReferences => {
                write!(formatter, "manifest has too many secret references")
            }
            Self::InvalidRuntimeCompatibility => {
                write!(formatter, "runtime compatibility bounds are invalid")
            }
            Self::InvalidProfileId => write!(formatter, "profile ID must not be blank"),
            Self::InvalidProfileVersion => write!(formatter, "profile version must be nonzero"),
            Self::DuplicateProfileCapabilityId { id } => {
                write!(formatter, "profile includes capability {id} more than once")
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

#[derive(Serialize, Deserialize)]
struct ManifestCatalogSnapshot {
    manifests: Vec<CapabilityManifest>,
    profiles: Vec<CapabilityProfile>,
}

impl Serialize for ManifestCatalog {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        ManifestCatalogSnapshot {
            manifests: self.manifests.values().cloned().collect(),
            profiles: self.profiles.values().cloned().collect(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ManifestCatalog {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let snapshot = ManifestCatalogSnapshot::deserialize(deserializer)?;
        let mut catalog = Self::default();
        for manifest in snapshot.manifests {
            catalog
                .register_manifest(manifest)
                .map_err(serde::de::Error::custom)?;
        }
        for profile in snapshot.profiles {
            catalog
                .register_profile(profile)
                .map_err(serde::de::Error::custom)?;
        }
        Ok(catalog)
    }
}

impl ManifestCatalog {
    pub fn register_manifest(
        &mut self,
        manifest: CapabilityManifest,
    ) -> Result<(), ManifestCatalogError> {
        validate_manifest(&manifest)?;
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
        if profile.id.trim().is_empty() {
            return Err(ManifestCatalogError::InvalidProfileId);
        }
        if profile.version == 0 {
            return Err(ManifestCatalogError::InvalidProfileVersion);
        }
        let mut capability_ids = std::collections::BTreeSet::new();
        for entry in &profile.entries {
            if !capability_ids.insert(entry.capability_id.clone()) {
                return Err(ManifestCatalogError::DuplicateProfileCapabilityId {
                    id: entry.capability_id.clone(),
                });
            }
        }
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

/// A stable namespace for durable capability invocation identities.
pub const CAPABILITY_INVOCATION_NAMESPACE: Uuid =
    Uuid::from_u128(0x43a6_aa38_239d_5bf5_963d_45dc_8731_c2ef);
pub const MAX_CAPABILITY_ARGUMENT_BYTES: usize = 64 * 1024;
pub const MAX_CAPABILITY_ARGUMENT_DEPTH: usize = 64;
pub const MAX_CAPABILITY_ARGUMENT_NODES: usize = 10_000;
pub const MAX_CAPABILITY_ID_BYTES: usize = 256;
pub const MAX_CAPABILITY_SECRET_REFERENCES: usize = (u16::MAX as usize) + 1;

/// Errors raised before an argument value enters schema validation or an executor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogicalInvocationError {
    ArgumentsTooLarge,
    ArgumentsTooDeep,
    ArgumentsTooManyNodes,
    CanonicalizationFailed,
    IdentifierTooLarge,
}

impl fmt::Display for LogicalInvocationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ArgumentsTooLarge => "capability arguments exceed the byte limit",
            Self::ArgumentsTooDeep => "capability arguments exceed the nesting limit",
            Self::ArgumentsTooManyNodes => "capability arguments exceed the node limit",
            Self::CanonicalizationFailed => "capability arguments cannot be canonicalized",
            Self::IdentifierTooLarge => "capability invocation identifier exceeds the byte limit",
        })
    }
}

impl std::error::Error for LogicalInvocationError {}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct LogicalInvocationSeed {
    run_id: Uuid,
    logical_step_id: String,
    capability_id: String,
    manifest_version: u32,
    normalized_arguments: Value,
}

/// The identity of a logical capability invocation, independent of its attempts.
#[derive(Clone, Debug, PartialEq)]
pub struct LogicalInvocation {
    id: Uuid,
    seed: LogicalInvocationSeed,
}

impl LogicalInvocation {
    pub fn new(
        run_id: Uuid,
        logical_step_id: impl Into<String>,
        capability_id: impl Into<String>,
        manifest_version: u32,
        arguments: Value,
    ) -> Result<Self, LogicalInvocationError> {
        let logical_step_id = logical_step_id.into();
        let capability_id = capability_id.into();
        if logical_step_id.is_empty()
            || capability_id.is_empty()
            || logical_step_id.len() > MAX_CAPABILITY_ID_BYTES
            || capability_id.len() > MAX_CAPABILITY_ID_BYTES
        {
            return Err(LogicalInvocationError::IdentifierTooLarge);
        }
        validate_argument_bounds(&arguments)?;
        let normalized_arguments = canonicalize_arguments(arguments)?;
        Self::from_seed(LogicalInvocationSeed {
            run_id,
            logical_step_id,
            capability_id,
            manifest_version,
            normalized_arguments,
        })
    }

    pub fn id(&self) -> Uuid {
        self.id
    }

    pub fn idempotency_key(&self) -> String {
        format!("anima-core:{}", self.id)
    }

    pub fn run_id(&self) -> Uuid {
        self.seed.run_id
    }

    pub fn logical_step_id(&self) -> &str {
        &self.seed.logical_step_id
    }

    pub fn capability_id(&self) -> &str {
        &self.seed.capability_id
    }

    pub fn manifest_version(&self) -> u32 {
        self.seed.manifest_version
    }

    pub fn normalized_arguments(&self) -> &Value {
        &self.seed.normalized_arguments
    }

    /// A stable, non-reversible binding for the already JCS-normalized argument value.
    ///
    /// This is intentionally separate from `id()`: policy records can bind an exact action's
    /// arguments without serializing the argument document itself.
    pub fn canonical_argument_digest(&self) -> Uuid {
        let bytes = serde_jcs::to_vec(&self.seed.normalized_arguments)
            .expect("normalized JSON values are always canonicalizable");
        Uuid::new_v5(&CAPABILITY_INVOCATION_NAMESPACE, &bytes)
    }

    fn from_seed(seed: LogicalInvocationSeed) -> Result<Self, LogicalInvocationError> {
        #[derive(Serialize)]
        struct VersionedSeed<'a> {
            format: &'static str,
            run_id: Uuid,
            logical_step_id: &'a str,
            capability_id: &'a str,
            manifest_version: u32,
            normalized_arguments: &'a Value,
        }
        let bytes = serde_jcs::to_vec(&VersionedSeed {
            format: "anima-core.capability-invocation.v1",
            run_id: seed.run_id,
            logical_step_id: &seed.logical_step_id,
            capability_id: &seed.capability_id,
            manifest_version: seed.manifest_version,
            normalized_arguments: &seed.normalized_arguments,
        })
        .map_err(|_| LogicalInvocationError::CanonicalizationFailed)?;
        Ok(Self {
            id: Uuid::new_v5(&CAPABILITY_INVOCATION_NAMESPACE, &bytes),
            seed,
        })
    }
}

impl Serialize for LogicalInvocation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.seed.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for LogicalInvocation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let seed = LogicalInvocationSeed::deserialize(deserializer)?;
        let invocation = Self::new(
            seed.run_id,
            seed.logical_step_id,
            seed.capability_id,
            seed.manifest_version,
            seed.normalized_arguments,
        )
        .map_err(serde::de::Error::custom)?;
        Ok(invocation)
    }
}

/// One append-only execution record for a logical invocation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CapabilityAttempt {
    id: Uuid,
    number: u32,
    logical_invocation_id: Uuid,
}

impl CapabilityAttempt {
    pub fn new(
        invocation: &LogicalInvocation,
        number: u32,
    ) -> Result<Self, CapabilityContextError> {
        if number == 0 {
            return Err(CapabilityContextError::InvalidAttemptNumber);
        }
        let id = attempt_id(invocation.id(), number);
        Ok(Self {
            id,
            number,
            logical_invocation_id: invocation.id(),
        })
    }

    pub fn id(&self) -> Uuid {
        self.id
    }

    pub fn number(&self) -> u32 {
        self.number
    }

    pub fn logical_invocation_id(&self) -> Uuid {
        self.logical_invocation_id
    }
}

#[derive(Deserialize)]
struct CapabilityAttemptSnapshot {
    id: Uuid,
    number: u32,
    logical_invocation_id: Uuid,
}

impl<'de> Deserialize<'de> for CapabilityAttempt {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let snapshot = CapabilityAttemptSnapshot::deserialize(deserializer)?;
        if snapshot.number == 0
            || snapshot.id != attempt_id(snapshot.logical_invocation_id, snapshot.number)
        {
            return Err(serde::de::Error::custom(
                CapabilityContextError::AttemptIdentityMismatch,
            ));
        }
        Ok(Self {
            id: snapshot.id,
            number: snapshot.number,
            logical_invocation_id: snapshot.logical_invocation_id,
        })
    }
}

fn attempt_id(logical_invocation_id: Uuid, number: u32) -> Uuid {
    let name = format!("attempt={logical_invocation_id}:number={number}");
    Uuid::new_v5(&CAPABILITY_INVOCATION_NAMESPACE, name.as_bytes())
}

/// An opaque non-secret resource handle. The field carrying it supplies its meaning.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilityReferenceId(Uuid);

impl CapabilityReferenceId {
    pub fn new(handle: Uuid) -> Self {
        Self(handle)
    }

    pub fn handle(&self) -> Uuid {
        self.0
    }
}

impl fmt::Debug for CapabilityReferenceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CapabilityReferenceId(REDACTED)")
    }
}

/// An index into the manifest's declared secret references, never a name or value.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilitySecretReferenceId(u16);

impl CapabilitySecretReferenceId {
    pub fn from_manifest_index(index: u16) -> Self {
        Self(index)
    }

    pub fn manifest_index(self) -> u16 {
        self.0
    }
}

impl fmt::Debug for CapabilitySecretReferenceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CapabilitySecretReferenceId(REDACTED)")
    }
}

/// Opaque resource references passed to an executor.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityExecutionReferences {
    owner: Option<CapabilityReferenceId>,
    agent: Option<CapabilityReferenceId>,
    session: Option<CapabilityReferenceId>,
    run: CapabilityReferenceId,
    workspace: Option<CapabilityReferenceId>,
    deadline: Option<CapabilityReferenceId>,
    cancellation: Option<CapabilityReferenceId>,
    secrets: Vec<CapabilitySecretReferenceId>,
}

impl CapabilityExecutionReferences {
    fn for_run(run_id: Uuid) -> Self {
        Self {
            owner: None,
            agent: None,
            session: None,
            run: CapabilityReferenceId::new(run_id),
            workspace: None,
            deadline: None,
            cancellation: None,
            secrets: Vec::new(),
        }
    }

    pub fn with_owner(mut self, reference: CapabilityReferenceId) -> Self {
        self.owner = Some(reference);
        self
    }

    pub fn with_agent(mut self, reference: CapabilityReferenceId) -> Self {
        self.agent = Some(reference);
        self
    }

    pub fn with_session(mut self, reference: CapabilityReferenceId) -> Self {
        self.session = Some(reference);
        self
    }

    pub fn with_workspace(mut self, reference: CapabilityReferenceId) -> Self {
        self.workspace = Some(reference);
        self
    }

    pub fn with_deadline(mut self, reference: CapabilityReferenceId) -> Self {
        self.deadline = Some(reference);
        self
    }

    pub fn with_cancellation(mut self, reference: CapabilityReferenceId) -> Self {
        self.cancellation = Some(reference);
        self
    }

    pub fn with_secrets(mut self, references: Vec<CapabilitySecretReferenceId>) -> Self {
        self.secrets = references;
        self
    }

    pub fn owner(&self) -> Option<&CapabilityReferenceId> {
        self.owner.as_ref()
    }
    pub fn agent(&self) -> Option<&CapabilityReferenceId> {
        self.agent.as_ref()
    }
    pub fn session(&self) -> Option<&CapabilityReferenceId> {
        self.session.as_ref()
    }
    pub fn run(&self) -> &CapabilityReferenceId {
        &self.run
    }
    pub fn workspace(&self) -> Option<&CapabilityReferenceId> {
        self.workspace.as_ref()
    }
    pub fn deadline(&self) -> Option<&CapabilityReferenceId> {
        self.deadline.as_ref()
    }
    pub fn cancellation(&self) -> Option<&CapabilityReferenceId> {
        self.cancellation.as_ref()
    }
    pub fn secret_handles(&self) -> &[CapabilitySecretReferenceId] {
        &self.secrets
    }

    fn secrets_are_declared_by(&self, manifest: &CapabilityManifest) -> bool {
        self.secrets.iter().all(|reference| {
            usize::from(reference.manifest_index()) < manifest.secret_references.len()
        })
    }
}

impl fmt::Debug for CapabilityExecutionReferences {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CapabilityExecutionReferences")
            .field("owner_present", &self.owner.is_some())
            .field("agent_present", &self.agent.is_some())
            .field("session_present", &self.session.is_some())
            .field("workspace_present", &self.workspace.is_some())
            .field("deadline_present", &self.deadline.is_some())
            .field("cancellation_present", &self.cancellation.is_some())
            .field("secret_reference_count", &self.secrets.len())
            .finish()
    }
}

/// Portable references supplied to a host executor. No credential values are carried here.
#[derive(Clone, PartialEq, Eq)]
pub struct ExecutionFencingToken(Uuid);

impl ExecutionFencingToken {
    /// Opaque value to pass to destinations that support fencing.
    pub fn destination_value(&self) -> String {
        self.0.to_string()
    }
}

impl fmt::Debug for ExecutionFencingToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ExecutionFencingToken(REDACTED)")
    }
}

#[derive(Clone)]
pub struct ExecutionFence {
    inner: Arc<ExecutionFenceState>,
}

struct ExecutionFenceState {
    token: ExecutionFencingToken,
    logical_invocation_id: Uuid,
    attempt_number: u32,
    lease_kind: CapabilityLeaseKind,
    idempotency_key: String,
    lineage: Arc<dyn CapabilityLineageStore>,
    valid: AtomicBool,
    cancelled: AtomicBool,
}

impl ExecutionFence {
    fn new(
        token: Uuid,
        logical_invocation_id: Uuid,
        attempt_number: u32,
        lease_kind: CapabilityLeaseKind,
        idempotency_key: String,
        lineage: Arc<dyn CapabilityLineageStore>,
    ) -> Self {
        Self {
            inner: Arc::new(ExecutionFenceState {
                token: ExecutionFencingToken(token),
                logical_invocation_id,
                attempt_number,
                lease_kind,
                idempotency_key,
                lineage,
                valid: AtomicBool::new(true),
                cancelled: AtomicBool::new(false),
            }),
        }
    }

    /// Authoritatively validates this exact fence without renewing it. Hosts must await this
    /// immediately before dispatching each irreversible external side effect.
    pub async fn ensure_valid(&self) -> Result<(), CapabilityError> {
        if !self.is_valid() {
            return Err(CapabilityError::cancelled());
        }
        match self
            .inner
            .lineage
            .validate_effect_fence(
                self.inner.logical_invocation_id,
                self.inner.attempt_number,
                self.inner.lease_kind,
                self.inner.token.0,
            )
            .await
        {
            Ok(true) if self.is_valid() => Ok(()),
            Ok(_) => {
                self.cancel();
                Err(CapabilityError::cancelled())
            }
            Err(error) => {
                self.cancel();
                Err(error)
            }
        }
    }

    pub fn fencing_token(&self) -> &ExecutionFencingToken {
        &self.inner.token
    }

    /// Stable key shared by every attempt of this logical invocation.
    pub fn idempotency_key(&self) -> &str {
        &self.inner.idempotency_key
    }

    pub fn is_valid(&self) -> bool {
        self.inner.valid.load(Ordering::Acquire)
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }

    fn cancel(&self) {
        self.inner.valid.store(false, Ordering::Release);
        self.inner.cancelled.store(true, Ordering::Release);
    }
}

impl fmt::Debug for ExecutionFence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExecutionFence")
            .field("valid", &self.is_valid())
            .field("cancelled", &self.is_cancelled())
            .finish_non_exhaustive()
    }
}

impl PartialEq for ExecutionFence {
    fn eq(&self, other: &Self) -> bool {
        self.inner.token == other.inner.token
    }
}

impl Eq for ExecutionFence {}

#[derive(Clone, PartialEq)]
pub struct CapabilityExecutionContext {
    invocation: LogicalInvocation,
    attempt: CapabilityAttempt,
    references: CapabilityExecutionReferences,
    execution_fence: Option<ExecutionFence>,
}

impl CapabilityExecutionContext {
    pub fn for_attempt(
        invocation: LogicalInvocation,
        attempt: CapabilityAttempt,
    ) -> Result<Self, CapabilityContextError> {
        Self::try_new(invocation, attempt)
    }

    pub fn invocation(&self) -> &LogicalInvocation {
        &self.invocation
    }

    pub fn attempt(&self) -> &CapabilityAttempt {
        &self.attempt
    }

    pub fn normalized_arguments(&self) -> &Value {
        self.invocation.normalized_arguments()
    }

    pub fn references(&self) -> &CapabilityExecutionReferences {
        &self.references
    }

    pub fn execution_fence(&self) -> Option<&ExecutionFence> {
        self.execution_fence.as_ref()
    }

    fn with_execution_fence(mut self, execution_fence: ExecutionFence) -> Self {
        self.execution_fence = Some(execution_fence);
        self
    }

    pub fn with_references(
        mut self,
        references: CapabilityExecutionReferences,
    ) -> Result<Self, CapabilityContextError> {
        let expected_run = CapabilityReferenceId::new(self.invocation.run_id());
        if references.run != expected_run {
            return Err(CapabilityContextError::RunReferenceMismatch);
        }
        self.references = references;
        Ok(self)
    }

    fn try_new(
        invocation: LogicalInvocation,
        attempt: CapabilityAttempt,
    ) -> Result<Self, CapabilityContextError> {
        if attempt.number == 0 {
            return Err(CapabilityContextError::InvalidAttemptNumber);
        }
        let expected_attempt = CapabilityAttempt::new(&invocation, attempt.number)?;
        if attempt.logical_invocation_id != invocation.id || attempt.id != expected_attempt.id {
            return Err(CapabilityContextError::AttemptIdentityMismatch);
        }
        Ok(Self {
            references: CapabilityExecutionReferences::for_run(invocation.run_id()),
            invocation,
            attempt,
            execution_fence: None,
        })
    }
}

/// Errors produced while constructing or restoring a durable execution context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityContextError {
    InvalidAttemptNumber,
    AttemptIdentityMismatch,
    RunReferenceMismatch,
    InvalidReference,
    InvalidSecretReference,
}

impl fmt::Display for CapabilityContextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAttemptNumber => formatter.write_str("attempt number must be nonzero"),
            Self::AttemptIdentityMismatch => {
                formatter.write_str("attempt does not belong to the logical invocation")
            }
            Self::RunReferenceMismatch => {
                formatter.write_str("run reference does not match the logical invocation")
            }
            Self::InvalidReference => formatter.write_str("reference ID is invalid"),
            Self::InvalidSecretReference => formatter.write_str("secret reference ID is invalid"),
        }
    }
}

impl std::error::Error for CapabilityContextError {}

#[derive(Serialize)]
struct CapabilityExecutionContextSerialization<'a> {
    invocation: &'a LogicalInvocation,
    attempt: &'a CapabilityAttempt,
    references: &'a CapabilityExecutionReferences,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CapabilityExecutionContextDeserialization {
    invocation: LogicalInvocation,
    attempt: CapabilityAttempt,
    references: CapabilityExecutionReferences,
}

impl Serialize for CapabilityExecutionContext {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        CapabilityExecutionContextSerialization {
            invocation: &self.invocation,
            attempt: &self.attempt,
            references: &self.references,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for CapabilityExecutionContext {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let snapshot = CapabilityExecutionContextDeserialization::deserialize(deserializer)?;
        let mut context = Self::try_new(snapshot.invocation, snapshot.attempt)
            .map_err(serde::de::Error::custom)?;
        if snapshot.references.run != context.references.run {
            return Err(serde::de::Error::custom(
                CapabilityContextError::RunReferenceMismatch,
            ));
        }
        context.references = snapshot.references;
        Ok(context)
    }
}

impl fmt::Debug for CapabilityExecutionContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CapabilityExecutionContext")
            .field("logical_invocation_id", &self.invocation.id())
            .field("attempt_number", &self.attempt.number())
            .field("references", &self.references)
            .field("execution_fence_present", &self.execution_fence.is_some())
            .finish()
    }
}

/// A portable capability output that has passed the registered output schema.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CapabilityResult {
    pub output: Value,
}

impl CapabilityResult {
    pub fn new(output: Value) -> Self {
        Self { output }
    }
}

/// Stable safe error classifications for capability boundaries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityErrorCode {
    Validation,
    Unavailable,
    Timeout,
    Cancelled,
    Execution,
    OutputValidation,
    Reconciliation,
}

/// A serializable capability error with deliberately safe, portable diagnostics only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityError {
    code: CapabilityErrorCode,
}

impl CapabilityError {
    pub fn validation() -> Self {
        Self::from_code(CapabilityErrorCode::Validation)
    }

    pub fn unavailable() -> Self {
        Self::from_code(CapabilityErrorCode::Unavailable)
    }

    pub fn timeout() -> Self {
        Self::from_code(CapabilityErrorCode::Timeout)
    }

    pub fn cancelled() -> Self {
        Self::from_code(CapabilityErrorCode::Cancelled)
    }

    pub fn execution() -> Self {
        Self::from_code(CapabilityErrorCode::Execution)
    }

    pub fn output_validation() -> Self {
        Self::from_code(CapabilityErrorCode::OutputValidation)
    }

    pub fn reconciliation() -> Self {
        Self::from_code(CapabilityErrorCode::Reconciliation)
    }

    pub fn code(&self) -> CapabilityErrorCode {
        self.code
    }

    pub fn retryable(&self) -> bool {
        matches!(
            self.code,
            CapabilityErrorCode::Timeout
                | CapabilityErrorCode::Execution
                | CapabilityErrorCode::Reconciliation
        )
    }

    pub fn message(&self) -> &'static str {
        match self.code {
            CapabilityErrorCode::Validation => "capability validation failed",
            CapabilityErrorCode::Unavailable => "the requested capability executor is unavailable",
            CapabilityErrorCode::Timeout => "capability execution timed out",
            CapabilityErrorCode::Cancelled => "capability execution was cancelled",
            CapabilityErrorCode::Execution => "capability execution failed",
            CapabilityErrorCode::OutputValidation => "capability output validation failed",
            CapabilityErrorCode::Reconciliation => "capability reconciliation failed",
        }
    }

    fn from_code(code: CapabilityErrorCode) -> Self {
        Self { code }
    }
}

impl fmt::Display for CapabilityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message())
    }
}

impl fmt::Display for CapabilityErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let code = match self {
            Self::Validation => "validation",
            Self::Unavailable => "unavailable",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::Execution => "execution",
            Self::OutputValidation => "output_validation",
            Self::Reconciliation => "reconciliation",
        };
        formatter.write_str(code)
    }
}

impl std::error::Error for CapabilityError {}

/// A host-owned executor paired to one exact portable manifest version.
#[async_trait]
pub trait CapabilityExecutor: Send + Sync {
    fn manifest(&self) -> &CapabilityManifest;

    /// Before every irreversible side effect, implementations must await
    /// `context.execution_fence().ensure_valid()` immediately before dispatch and pass both the
    /// fence's opaque `fencing_token()` and its logical `idempotency_key()` to destinations that
    /// support fencing or deduplication. If a destination supports neither, reconciliation must
    /// not claim `AuthoritativeAbsence` until the prior operation is provably terminal; otherwise
    /// it must return `RecoveryRequired`. The fence is not a generic exactly-once guarantee.
    async fn execute(
        &self,
        context: CapabilityExecutionContext,
    ) -> Result<CapabilityResult, CapabilityError>;

    async fn reconcile(
        &self,
        context: CapabilityExecutionContext,
    ) -> Result<ReconcileOutcome, CapabilityError>;
}

/// An exact executor's observation of an uncertain prior invocation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ReconcileOutcome {
    Completed(CapabilityResult),
    Pending,
    /// Strong guarantee that the prior operation is not in flight and cannot later complete, even
    /// if an abandoned worker resumes. Executors that cannot prove this must return `Pending` or
    /// `RecoveryRequired`.
    AuthoritativeAbsence,
    RecoveryRequired,
}

/// The action a host recovery orchestrator must take next.
#[derive(Clone, Debug, PartialEq)]
pub enum RecoveryAction {
    RetrySameKey {
        idempotency_key: String,
        authorization: CapabilityRetryAuthorization,
    },
    Completed(CapabilityResult),
    Pending,
    AuthoritativeAbsence,
    RecoveryRequired,
}

/// An opaque, one-time registry authorization for a specific retry attempt.
#[derive(Clone, PartialEq, Eq)]
pub struct CapabilityRetryAuthorization {
    nonce: Uuid,
}

impl fmt::Debug for CapabilityRetryAuthorization {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CapabilityRetryAuthorization(REDACTED)")
    }
}

/// A compact discriminator for recovery orchestration and public tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecoveryActionKind {
    RetrySameKey,
    Completed,
    Pending,
    AuthoritativeAbsence,
    RecoveryRequired,
}

impl RecoveryAction {
    pub fn kind(&self) -> RecoveryActionKind {
        match self {
            Self::RetrySameKey { .. } => RecoveryActionKind::RetrySameKey,
            Self::Completed(_) => RecoveryActionKind::Completed,
            Self::Pending => RecoveryActionKind::Pending,
            Self::AuthoritativeAbsence => RecoveryActionKind::AuthoritativeAbsence,
            Self::RecoveryRequired => RecoveryActionKind::RecoveryRequired,
        }
    }

    pub fn retry_authorization(&self) -> Option<&CapabilityRetryAuthorization> {
        match self {
            Self::RetrySameKey { authorization, .. } => Some(authorization),
            _ => None,
        }
    }
}

/// Registration failures for host executors. These errors never include host diagnostics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CapabilityRegistryError {
    DuplicateExecutor { id: String, version: u32 },
    ManifestExecutorMismatch { id: String, version: u32 },
    InvalidInputSchema { id: String, version: u32 },
    InvalidOutputSchema { id: String, version: u32 },
}

impl fmt::Display for CapabilityRegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateExecutor { id, version } => {
                write!(formatter, "executor {id}@{version} is already registered")
            }
            Self::ManifestExecutorMismatch { id, version } => {
                write!(formatter, "executor does not match manifest {id}@{version}")
            }
            Self::InvalidInputSchema { id, version } => {
                write!(
                    formatter,
                    "manifest {id}@{version} has an invalid input schema"
                )
            }
            Self::InvalidOutputSchema { id, version } => {
                write!(
                    formatter,
                    "manifest {id}@{version} has an invalid output schema"
                )
            }
        }
    }
}

impl std::error::Error for CapabilityRegistryError {}

#[path = "capability/lineage.rs"]
mod lineage;
pub use lineage::{CapabilityAttemptLineageState, CapabilityLeaseKind, CapabilityLineageStore};
#[path = "capability/registry.rs"]
mod registry;
pub use registry::CapabilityRegistry;

fn canonicalize_json(value: Value) -> Result<Value, CapabilityError> {
    canonicalize_arguments(value).map_err(|_| CapabilityError::output_validation())
}
