//! Storage-neutral memory contracts.
//!
//! Hosts own persistence, encryption, indexing, and access-control enforcement. These types
//! only describe portable, validated intent and never carry credentials or storage details.

use std::fmt;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

const MAX_ID_BYTES: usize = 256;
const MAX_TEXT_BYTES: usize = 32 * 1024;
const MAX_EXPLANATION_BYTES: usize = 4 * 1024;
const MAX_SUPERSEDES: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Fact,
    Preference,
    Episode,
    Procedure,
    Reflection,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum MemoryScope {
    Owner(String),
    Agent(String),
    Session(String),
    Workspace(String),
}

impl MemoryScope {
    pub fn owner(id: impl Into<String>) -> Result<Self, MemoryPortError> {
        Ok(Self::Owner(required(id.into(), "owner scope ID")?))
    }

    pub fn agent(id: impl Into<String>) -> Result<Self, MemoryPortError> {
        Ok(Self::Agent(required(id.into(), "agent scope ID")?))
    }

    pub fn session(id: impl Into<String>) -> Result<Self, MemoryPortError> {
        Ok(Self::Session(required(id.into(), "session scope ID")?))
    }

    pub fn workspace(id: impl Into<String>) -> Result<Self, MemoryPortError> {
        Ok(Self::Workspace(required(id.into(), "workspace scope ID")?))
    }

    /// Authorization is deliberately exact: a session is not implicitly an owner or workspace.
    pub fn is_authorized_by(&self, authorized: &Self) -> bool {
        self == authorized
    }

    pub fn id(&self) -> &str {
        match self {
            Self::Owner(id) | Self::Agent(id) | Self::Session(id) | Self::Workspace(id) => id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryProvenance {
    pub source: String,
    pub source_identity: String,
    pub observed_at_ms: u64,
}

impl MemoryProvenance {
    pub fn new(
        source: impl Into<String>,
        source_identity: impl Into<String>,
        observed_at_ms: u64,
    ) -> Result<Self, MemoryPortError> {
        Ok(Self {
            source: required(source.into(), "provenance source")?,
            source_identity: required(source_identity.into(), "provenance source identity")?,
            observed_at_ms,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionPolicy {
    Persistent,
    Deadline,
    Ephemeral,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryRetention {
    pub policy: RetentionPolicy,
    pub deadline_ms: Option<u64>,
}

impl Default for MemoryRetention {
    fn default() -> Self {
        Self {
            policy: RetentionPolicy::Persistent,
            deadline_ms: None,
        }
    }
}

impl MemoryRetention {
    pub fn deadline(deadline_ms: u64) -> Result<Self, MemoryPortError> {
        if deadline_ms == 0 {
            return Err(MemoryPortError::invalid(
                "retention deadline must be non-zero",
            ));
        }
        Ok(Self {
            policy: RetentionPolicy::Deadline,
            deadline_ms: Some(deadline_ms),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryWrite {
    pub id: String,
    pub kind: MemoryKind,
    pub scope: MemoryScope,
    pub content: String,
    pub confidence: f64,
    pub provenance: MemoryProvenance,
    #[serde(default)]
    pub retention: MemoryRetention,
}

impl MemoryWrite {
    pub fn new(
        id: impl Into<String>,
        kind: MemoryKind,
        scope: MemoryScope,
        content: impl Into<String>,
        confidence: f64,
        provenance: MemoryProvenance,
    ) -> Result<Self, MemoryPortError> {
        Ok(Self {
            id: required(id.into(), "memory ID")?,
            kind,
            scope,
            content: bounded(content.into(), "memory content", MAX_TEXT_BYTES)?,
            confidence: validated_confidence(confidence)?,
            provenance,
            retention: MemoryRetention::default(),
        })
    }

    pub fn with_retention_deadline(mut self, deadline_ms: u64) -> Result<Self, MemoryPortError> {
        self.retention = MemoryRetention::deadline(deadline_ms)?;
        Ok(self)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryRecord {
    pub id: String,
    pub kind: MemoryKind,
    pub scope: MemoryScope,
    pub content: String,
    pub confidence: f64,
    pub provenance: MemoryProvenance,
    pub revision: u64,
    pub supersedes: Vec<String>,
    pub corrects: Option<String>,
    pub forgotten_at_ms: Option<u64>,
    pub forget_reason: Option<String>,
    pub retention: MemoryRetention,
}

impl MemoryRecord {
    pub fn from_write(write: MemoryWrite, revision: u64) -> Result<Self, MemoryPortError> {
        if revision == 0 {
            return Err(MemoryPortError::invalid(
                "memory revision must be at least one",
            ));
        }
        Ok(Self {
            id: write.id,
            kind: write.kind,
            scope: write.scope,
            content: write.content,
            confidence: write.confidence,
            provenance: write.provenance,
            revision,
            supersedes: Vec::new(),
            corrects: None,
            forgotten_at_ms: None,
            forget_reason: None,
            retention: write.retention,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryRevision {
    pub memory_id: String,
    pub expected_revision: u64,
    pub supersedes: Vec<String>,
    pub corrects: Option<String>,
    pub forget_reason: Option<String>,
}

impl MemoryRevision {
    pub fn supersession(
        memory_id: impl Into<String>,
        expected_revision: u64,
        supersedes: Vec<String>,
    ) -> Result<Self, MemoryPortError> {
        Ok(Self {
            memory_id: required(memory_id.into(), "memory ID")?,
            expected_revision: validated_expected_revision(expected_revision)?,
            supersedes: bounded_ids(supersedes, "superseded memory ID")?,
            corrects: None,
            forget_reason: None,
        })
    }

    pub fn correction(
        memory_id: impl Into<String>,
        expected_revision: u64,
        corrects: impl Into<String>,
        _reason: impl Into<String>,
    ) -> Result<Self, MemoryPortError> {
        let corrects = required(corrects.into(), "corrected memory ID")?;
        let mut revision =
            Self::supersession(memory_id, expected_revision, vec![corrects.clone()])?;
        revision.corrects = Some(corrects);
        Ok(revision)
    }

    pub fn forget(
        memory_id: impl Into<String>,
        expected_revision: u64,
        reason: impl Into<String>,
    ) -> Result<Self, MemoryPortError> {
        Ok(Self {
            memory_id: required(memory_id.into(), "memory ID")?,
            expected_revision: validated_expected_revision(expected_revision)?,
            supersedes: Vec::new(),
            corrects: None,
            forget_reason: Some(bounded(
                reason.into(),
                "forget reason",
                MAX_EXPLANATION_BYTES,
            )?),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryQuery {
    pub text: String,
    pub authorized_scopes: Vec<MemoryScope>,
    pub limit: usize,
}

impl MemoryQuery {
    pub fn new(
        text: impl Into<String>,
        authorized_scopes: Vec<MemoryScope>,
        limit: usize,
    ) -> Result<Self, MemoryPortError> {
        if authorized_scopes.is_empty() {
            return Err(MemoryPortError::invalid(
                "at least one authorized scope is required",
            ));
        }
        if limit == 0 || limit > 1000 {
            return Err(MemoryPortError::invalid(
                "memory query limit must be between 1 and 1000",
            ));
        }
        Ok(Self {
            text: bounded(text.into(), "memory query text", MAX_TEXT_BYTES)?,
            authorized_scopes,
            limit,
        })
    }

    pub fn authorizes(&self, scope: &MemoryScope) -> bool {
        self.authorized_scopes
            .iter()
            .any(|authorized| scope.is_authorized_by(authorized))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryHit {
    pub record: MemoryRecord,
    pub score: f64,
    pub explanation: String,
}

impl MemoryHit {
    pub fn new(
        record: MemoryRecord,
        score: f64,
        explanation: impl Into<String>,
    ) -> Result<Self, MemoryPortError> {
        Ok(Self {
            record,
            score: finite_score(score)?,
            explanation: bounded(
                explanation.into(),
                "retrieval explanation",
                MAX_EXPLANATION_BYTES,
            )?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryPortErrorCode {
    InvalidInput,
    UnauthorizedScope,
    NotFound,
    Conflict,
    Unsupported,
    Backend,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryPortError {
    pub code: MemoryPortErrorCode,
    pub message: String,
}

impl MemoryPortError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: MemoryPortErrorCode::InvalidInput,
            message: message.into(),
        }
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self {
            code: MemoryPortErrorCode::Unsupported,
            message: message.into(),
        }
    }
}

impl fmt::Display for MemoryPortError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for MemoryPortError {}

#[async_trait]
pub trait MemoryPort: Send + Sync {
    async fn write(&self, write: MemoryWrite) -> Result<MemoryRecord, MemoryPortError>;
    async fn query(&self, query: MemoryQuery) -> Result<Vec<MemoryHit>, MemoryPortError>;
    /// Applies a CAS-guarded lifecycle mutation. Implementations must reject stale revisions.
    async fn revise(&self, revision: MemoryRevision) -> Result<MemoryRecord, MemoryPortError>;
}

fn required(value: String, field: &str) -> Result<String, MemoryPortError> {
    bounded(value, field, MAX_ID_BYTES)
}

fn bounded(value: String, field: &str, max_bytes: usize) -> Result<String, MemoryPortError> {
    let value = value.trim().to_owned();
    if value.is_empty() || value.len() > max_bytes {
        return Err(MemoryPortError::invalid(format!(
            "{field} must be non-empty and at most {max_bytes} bytes"
        )));
    }
    Ok(value)
}

fn validated_confidence(value: f64) -> Result<f64, MemoryPortError> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(value)
    } else {
        Err(MemoryPortError::invalid(
            "confidence must be finite and between 0 and 1",
        ))
    }
}

fn finite_score(value: f64) -> Result<f64, MemoryPortError> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(MemoryPortError::invalid("retrieval score must be finite"))
    }
}

fn validated_expected_revision(value: u64) -> Result<u64, MemoryPortError> {
    if value == 0 {
        Err(MemoryPortError::invalid(
            "expected revision must be at least one",
        ))
    } else {
        Ok(value)
    }
}

fn bounded_ids(values: Vec<String>, field: &str) -> Result<Vec<String>, MemoryPortError> {
    if values.len() > MAX_SUPERSEDES {
        return Err(MemoryPortError::invalid("too many superseded memories"));
    }
    values
        .into_iter()
        .map(|value| required(value, field))
        .collect()
}
