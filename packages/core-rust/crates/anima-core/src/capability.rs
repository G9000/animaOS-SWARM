use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use jsonschema::JSONSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use uuid::Uuid;

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

/// The identity of a logical capability invocation, independent of its attempts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogicalInvocation {
    pub id: Uuid,
    pub run_id: Uuid,
    pub logical_step_id: String,
    pub capability_id: String,
    pub manifest_version: u32,
    pub normalized_arguments: Value,
    pub idempotency_key: String,
}

impl LogicalInvocation {
    pub fn new(
        run_id: Uuid,
        logical_step_id: impl Into<String>,
        capability_id: impl Into<String>,
        manifest_version: u32,
        arguments: Value,
    ) -> Self {
        let logical_step_id = logical_step_id.into();
        let capability_id = capability_id.into();
        let normalized_arguments = canonicalize_json(arguments);
        let canonical_arguments =
            serde_json::to_string(&normalized_arguments).expect("serde_json::Value must serialize");
        let name = format!(
            "run={run_id}\u{1f}step={logical_step_id}\u{1f}capability={capability_id}\u{1f}version={manifest_version}\u{1f}arguments={canonical_arguments}"
        );
        let id = Uuid::new_v5(&CAPABILITY_INVOCATION_NAMESPACE, name.as_bytes());

        Self {
            id,
            run_id,
            logical_step_id,
            capability_id,
            manifest_version,
            normalized_arguments,
            idempotency_key: format!("anima-core:{id}"),
        }
    }
}

/// One append-only execution record for a logical invocation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityAttempt {
    pub id: Uuid,
    pub number: u32,
    pub logical_invocation_id: Uuid,
}

impl CapabilityAttempt {
    pub fn new(invocation: &LogicalInvocation, number: u32) -> Self {
        let name = format!("attempt={}\u{1f}number={number}", invocation.id);
        Self {
            id: Uuid::new_v5(&CAPABILITY_INVOCATION_NAMESPACE, name.as_bytes()),
            number,
            logical_invocation_id: invocation.id,
        }
    }
}

/// Portable references supplied to a host executor. No credential values are carried here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CapabilityExecutionContext {
    pub invocation: LogicalInvocation,
    pub attempt: CapabilityAttempt,
    pub normalized_arguments: Value,
    pub owner_reference: Option<String>,
    pub agent_reference: Option<String>,
    pub session_reference: Option<String>,
    pub run_reference: String,
    pub workspace_reference: Option<String>,
    pub deadline_reference: Option<String>,
    pub cancellation_reference: Option<String>,
    pub secret_references: Vec<String>,
}

impl CapabilityExecutionContext {
    pub fn for_attempt(invocation: LogicalInvocation, attempt: CapabilityAttempt) -> Self {
        let run_reference = format!("run:{}", invocation.run_id);
        Self {
            normalized_arguments: invocation.normalized_arguments.clone(),
            invocation,
            attempt,
            owner_reference: None,
            agent_reference: None,
            session_reference: None,
            run_reference,
            workspace_reference: None,
            deadline_reference: None,
            cancellation_reference: None,
            secret_references: Vec::new(),
        }
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
pub struct CapabilityError {
    pub code: CapabilityErrorCode,
    pub message: String,
    pub retryable: bool,
}

impl CapabilityError {
    pub fn new(code: CapabilityErrorCode, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code,
            message: message.into(),
            retryable,
        }
    }

    fn validation(message: &'static str) -> Self {
        Self::new(CapabilityErrorCode::Validation, message, false)
    }

    fn unavailable() -> Self {
        Self::new(
            CapabilityErrorCode::Unavailable,
            "the requested capability executor is unavailable",
            false,
        )
    }

    fn output_validation() -> Self {
        Self::new(
            CapabilityErrorCode::OutputValidation,
            "capability output does not satisfy its registered schema",
            false,
        )
    }
}

impl fmt::Display for CapabilityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
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
    AuthoritativeAbsence,
    RecoveryRequired,
}

/// The action a host recovery orchestrator must take next.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RecoveryAction {
    RetrySameKey { idempotency_key: String },
    Completed(CapabilityResult),
    Pending,
    AuthoritativeAbsence,
    RecoveryRequired,
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

/// A host-agnostic pairing of the exact portable manifest catalog and host executors.
#[derive(Clone)]
pub struct CapabilityRegistry {
    catalog: ManifestCatalog,
    executors: BTreeMap<(String, u32), Arc<dyn CapabilityExecutor>>,
}

impl CapabilityRegistry {
    pub fn new(catalog: ManifestCatalog) -> Self {
        Self {
            catalog,
            executors: BTreeMap::new(),
        }
    }

    pub fn manifest(&self, id: &str, version: u32) -> Option<&CapabilityManifest> {
        self.catalog.manifest(id, version)
    }

    pub fn executor(&self, id: &str, version: u32) -> Option<Arc<dyn CapabilityExecutor>> {
        self.executors.get(&(id.to_owned(), version)).cloned()
    }

    pub fn register_executor(
        &mut self,
        executor: Arc<dyn CapabilityExecutor>,
    ) -> Result<(), CapabilityRegistryError> {
        let executor_manifest = executor.manifest();
        let key = (executor_manifest.id.clone(), executor_manifest.version);
        let Some(registered_manifest) = self.catalog.manifest(&key.0, key.1) else {
            return Err(CapabilityRegistryError::ManifestExecutorMismatch {
                id: key.0,
                version: key.1,
            });
        };
        if registered_manifest.input_schema != executor_manifest.input_schema
            || registered_manifest.output_schema != executor_manifest.output_schema
        {
            return Err(CapabilityRegistryError::ManifestExecutorMismatch {
                id: executor_manifest.id.clone(),
                version: executor_manifest.version,
            });
        }
        if self.executors.contains_key(&key) {
            return Err(CapabilityRegistryError::DuplicateExecutor {
                id: key.0,
                version: key.1,
            });
        }
        compile_schema(&registered_manifest.input_schema).map_err(|_| {
            CapabilityRegistryError::InvalidInputSchema {
                id: registered_manifest.id.clone(),
                version: registered_manifest.version,
            }
        })?;
        compile_schema(&registered_manifest.output_schema).map_err(|_| {
            CapabilityRegistryError::InvalidOutputSchema {
                id: registered_manifest.id.clone(),
                version: registered_manifest.version,
            }
        })?;

        self.executors.insert(key, executor);
        Ok(())
    }

    pub async fn execute(
        &self,
        context: CapabilityExecutionContext,
    ) -> Result<CapabilityResult, CapabilityError> {
        let manifest = self.manifest_for_context(&context)?;
        validate_instance(&manifest.input_schema, &context.normalized_arguments, false)?;
        let executor = self
            .executor(&manifest.id, manifest.version)
            .ok_or_else(CapabilityError::unavailable)?;
        let mut result = executor.execute(context).await?;
        validate_instance(&manifest.output_schema, &result.output, true)?;
        result.output = canonicalize_json(result.output);
        Ok(result)
    }

    pub async fn recover(
        &self,
        context: CapabilityExecutionContext,
    ) -> Result<RecoveryAction, CapabilityError> {
        let manifest = self.manifest_for_context(&context)?;
        match manifest.recovery_mode {
            RecoveryMode::InherentlyIdempotent
            | RecoveryMode::KeyedIdempotent
            | RecoveryMode::Retry => Ok(RecoveryAction::RetrySameKey {
                idempotency_key: context.invocation.idempotency_key,
            }),
            RecoveryMode::Reconcilable | RecoveryMode::Compensate => {
                validate_instance(&manifest.input_schema, &context.normalized_arguments, false)?;
                let executor = self
                    .executor(&manifest.id, manifest.version)
                    .ok_or_else(CapabilityError::unavailable)?;
                match executor.reconcile(context).await? {
                    ReconcileOutcome::Completed(mut result) => {
                        validate_instance(&manifest.output_schema, &result.output, true)?;
                        result.output = canonicalize_json(result.output);
                        Ok(RecoveryAction::Completed(result))
                    }
                    ReconcileOutcome::Pending => Ok(RecoveryAction::Pending),
                    ReconcileOutcome::AuthoritativeAbsence => {
                        Ok(RecoveryAction::AuthoritativeAbsence)
                    }
                    ReconcileOutcome::RecoveryRequired => Ok(RecoveryAction::RecoveryRequired),
                }
            }
            RecoveryMode::NonRetryable | RecoveryMode::None | RecoveryMode::Manual => {
                Ok(RecoveryAction::RecoveryRequired)
            }
        }
    }

    fn manifest_for_context(
        &self,
        context: &CapabilityExecutionContext,
    ) -> Result<&CapabilityManifest, CapabilityError> {
        self.manifest(
            &context.invocation.capability_id,
            context.invocation.manifest_version,
        )
        .ok_or_else(CapabilityError::unavailable)
    }
}

fn compile_schema(schema: &Value) -> Result<JSONSchema, ()> {
    JSONSchema::compile(schema).map_err(|_| ())
}

fn validate_instance(
    schema: &Value,
    instance: &Value,
    output: bool,
) -> Result<(), CapabilityError> {
    let validator = compile_schema(schema).map_err(|_| {
        if output {
            CapabilityError::output_validation()
        } else {
            CapabilityError::validation("capability input schema is invalid")
        }
    })?;
    if validator.is_valid(instance) {
        Ok(())
    } else if output {
        Err(CapabilityError::output_validation())
    } else {
        Err(CapabilityError::validation(
            "capability input does not satisfy its registered schema",
        ))
    }
}

fn canonicalize_json(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(canonicalize_json).collect()),
        Value::Object(values) => {
            let mut normalized = BTreeMap::new();
            for (key, value) in values {
                normalized.insert(key, canonicalize_json(value));
            }
            Value::Object(normalized.into_iter().collect())
        }
        scalar => scalar,
    }
}

fn validate_manifest(manifest: &CapabilityManifest) -> Result<(), ManifestCatalogError> {
    if manifest.id.trim().is_empty() {
        return Err(ManifestCatalogError::InvalidManifestId);
    }
    if manifest.version == 0 {
        return Err(ManifestCatalogError::InvalidManifestVersion);
    }
    if manifest.compatibility.manifest_schema_version == 0 {
        return Err(ManifestCatalogError::InvalidManifestSchemaVersion);
    }
    if manifest.schema_digest.trim().is_empty() {
        return Err(ManifestCatalogError::InvalidSchemaDigest);
    }
    if manifest.compatibility.minimum_runtime_schema_version == 0
        || manifest.compatibility.maximum_runtime_schema_version == 0
        || manifest.compatibility.minimum_runtime_schema_version
            > manifest.compatibility.maximum_runtime_schema_version
    {
        return Err(ManifestCatalogError::InvalidRuntimeCompatibility);
    }
    Ok(())
}
