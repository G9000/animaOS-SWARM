//! Storage-neutral memory contracts.
//!
//! Hosts own persistence, encryption, indexing, and access-control enforcement. These types
//! only describe portable, validated intent and never carry credentials or storage details.

use std::fmt;
use std::marker::PhantomData;

use async_trait::async_trait;
use serde::de::{self, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

const MAX_ID_BYTES: usize = 256;
const MAX_TEXT_BYTES: usize = 32 * 1024;
const MAX_EXPLANATION_BYTES: usize = 4 * 1024;
const MAX_SUPERSEDES: usize = 64;
pub const MAX_KNOWLEDGE_SCOPES: usize = 64;
pub const MAX_KNOWLEDGE_HITS: usize = 1_000;

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
#[serde(
    tag = "kind",
    content = "id",
    rename_all = "snake_case",
    try_from = "MemoryScopeWire"
)]
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
#[serde(try_from = "MemoryProvenanceWire")]
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
#[serde(try_from = "MemoryRetentionWire")]
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

    fn validated(
        policy: RetentionPolicy,
        deadline_ms: Option<u64>,
    ) -> Result<Self, MemoryPortError> {
        match (policy, deadline_ms) {
            (RetentionPolicy::Persistent | RetentionPolicy::Ephemeral, None) => Ok(Self {
                policy,
                deadline_ms,
            }),
            (RetentionPolicy::Deadline, Some(deadline_ms)) if deadline_ms > 0 => Ok(Self {
                policy,
                deadline_ms: Some(deadline_ms),
            }),
            _ => Err(MemoryPortError::invalid(
                "retention policy and deadline must be consistent",
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "MemoryWriteWire")]
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
#[serde(try_from = "MemoryRecordWire")]
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

    fn validated(
        write: MemoryWrite,
        revision: u64,
        supersedes: Vec<String>,
        corrects: Option<String>,
        forgotten_at_ms: Option<u64>,
        forget_reason: Option<String>,
    ) -> Result<Self, MemoryPortError> {
        let mut record = Self::from_write(write, revision)?;
        record.supersedes = bounded_ids(supersedes, "superseded memory ID")?;
        record.corrects = corrects
            .map(|id| required(id, "corrected memory ID"))
            .transpose()?;
        if record
            .corrects
            .as_ref()
            .is_some_and(|id| !record.supersedes.contains(id))
        {
            return Err(MemoryPortError::invalid(
                "corrected memory must be superseded",
            ));
        }
        match (forgotten_at_ms, forget_reason) {
            (Some(timestamp), Some(reason)) => {
                record.forgotten_at_ms = Some(timestamp);
                record.forget_reason =
                    Some(bounded(reason, "forget reason", MAX_EXPLANATION_BYTES)?);
            }
            (None, None) => {}
            _ => {
                return Err(MemoryPortError::invalid(
                    "forgotten timestamp and reason must be paired",
                ))
            }
        }
        Ok(record)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "MemoryRevisionWire")]
pub struct MemoryRevision {
    pub memory_id: String,
    pub expected_revision: u64,
    pub supersedes: Vec<String>,
    pub corrects: Option<String>,
    pub correction_reason: Option<String>,
    pub forget_reason: Option<String>,
}

impl MemoryRevision {
    pub fn supersession(
        memory_id: impl Into<String>,
        expected_revision: u64,
        supersedes: Vec<String>,
    ) -> Result<Self, MemoryPortError> {
        let supersedes = bounded_ids(supersedes, "superseded memory ID")?;
        if supersedes.is_empty() {
            return Err(MemoryPortError::invalid(
                "supersession requires at least one superseded memory",
            ));
        }
        Ok(Self {
            memory_id: required(memory_id.into(), "memory ID")?,
            expected_revision: validated_expected_revision(expected_revision)?,
            supersedes,
            corrects: None,
            correction_reason: None,
            forget_reason: None,
        })
    }

    pub fn correction(
        memory_id: impl Into<String>,
        expected_revision: u64,
        corrects: impl Into<String>,
        reason: impl Into<String>,
    ) -> Result<Self, MemoryPortError> {
        let corrects = required(corrects.into(), "corrected memory ID")?;
        let mut revision =
            Self::supersession(memory_id, expected_revision, vec![corrects.clone()])?;
        revision.corrects = Some(corrects);
        revision.correction_reason = Some(bounded(
            reason.into(),
            "correction reason",
            MAX_EXPLANATION_BYTES,
        )?);
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
            correction_reason: None,
            forget_reason: Some(bounded(
                reason.into(),
                "forget reason",
                MAX_EXPLANATION_BYTES,
            )?),
        })
    }

    fn validated(
        memory_id: String,
        expected_revision: u64,
        supersedes: Vec<String>,
        corrects: Option<String>,
        correction_reason: Option<String>,
        forget_reason: Option<String>,
    ) -> Result<Self, MemoryPortError> {
        if let Some(reason) = forget_reason {
            if !supersedes.is_empty() || corrects.is_some() || correction_reason.is_some() {
                return Err(MemoryPortError::invalid(
                    "forget revisions cannot also supersede or correct",
                ));
            }
            return Self::forget(memory_id, expected_revision, reason);
        }
        match (corrects, correction_reason) {
            (Some(corrects), Some(reason)) => {
                let revision =
                    Self::correction(memory_id, expected_revision, corrects.clone(), reason)?;
                if supersedes != vec![corrects] {
                    return Err(MemoryPortError::invalid(
                        "correction must supersede exactly the corrected memory",
                    ));
                }
                Ok(revision)
            }
            (None, None) => Self::supersession(memory_id, expected_revision, supersedes),
            _ => Err(MemoryPortError::invalid(
                "correction ID and reason must be paired",
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "MemoryQueryWire")]
pub struct MemoryQuery {
    pub text: String,
    requested_scopes: Vec<MemoryScope>,
    pub limit: usize,
}

impl MemoryQuery {
    pub fn new(
        text: impl Into<String>,
        requested_scopes: Vec<MemoryScope>,
        limit: usize,
    ) -> Result<Self, MemoryPortError> {
        let requested_scopes = dedup_scopes(requested_scopes)?;
        if requested_scopes.is_empty() {
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
            requested_scopes,
            limit,
        })
    }

    pub fn requested_scopes(&self) -> &[MemoryScope] {
        &self.requested_scopes
    }
}

/// Host-issued scope grant. It deliberately has no `Deserialize` implementation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnowledgeAccessContext {
    authorized_scopes: Vec<MemoryScope>,
}

impl KnowledgeAccessContext {
    pub fn trusted(authorized_scopes: Vec<MemoryScope>) -> Result<Self, MemoryPortError> {
        let authorized_scopes = dedup_scopes(authorized_scopes)?;
        if authorized_scopes.is_empty() {
            return Err(MemoryPortError::invalid(
                "at least one trusted scope is required",
            ));
        }
        Ok(Self { authorized_scopes })
    }

    pub fn allows(&self, scope: &MemoryScope) -> bool {
        self.authorized_scopes
            .iter()
            .any(|authorized| scope.is_authorized_by(authorized))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MemoryQueryResult {
    hits: Vec<MemoryHit>,
}

impl MemoryQueryResult {
    pub fn new(
        access: &KnowledgeAccessContext,
        query: &MemoryQuery,
        hits: Vec<MemoryHit>,
    ) -> Result<Self, MemoryPortError> {
        validate_query_access(access, query.requested_scopes())?;
        if hits.len() > query.limit || hits.len() > MAX_KNOWLEDGE_HITS {
            return Err(MemoryPortError::invalid(
                "memory result exceeds query limit",
            ));
        }
        let mut ids = std::collections::BTreeSet::new();
        for hit in &hits {
            if !ids.insert(hit.record.id.clone()) {
                return Err(MemoryPortError::invalid(
                    "memory result contains duplicate IDs",
                ));
            }
            if !scope_allowed(access, query.requested_scopes(), &hit.record.scope) {
                return Err(MemoryPortError {
                    code: MemoryPortErrorCode::UnauthorizedScope,
                    message: "memory result contains an unauthorized scope".into(),
                });
            }
        }
        Ok(Self { hits })
    }

    pub fn hits(&self) -> &[MemoryHit] {
        &self.hits
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "MemoryHitWire")]
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

#[derive(Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
enum MemoryScopeWire {
    Owner(String),
    Agent(String),
    Session(String),
    Workspace(String),
}

impl TryFrom<MemoryScopeWire> for MemoryScope {
    type Error = MemoryPortError;
    fn try_from(value: MemoryScopeWire) -> Result<Self, Self::Error> {
        match value {
            MemoryScopeWire::Owner(id) => Self::owner(id),
            MemoryScopeWire::Agent(id) => Self::agent(id),
            MemoryScopeWire::Session(id) => Self::session(id),
            MemoryScopeWire::Workspace(id) => Self::workspace(id),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryProvenanceWire {
    source: String,
    source_identity: String,
    observed_at_ms: u64,
}

impl TryFrom<MemoryProvenanceWire> for MemoryProvenance {
    type Error = MemoryPortError;
    fn try_from(value: MemoryProvenanceWire) -> Result<Self, Self::Error> {
        Self::new(value.source, value.source_identity, value.observed_at_ms)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryRetentionWire {
    policy: RetentionPolicy,
    deadline_ms: Option<u64>,
}

impl TryFrom<MemoryRetentionWire> for MemoryRetention {
    type Error = MemoryPortError;
    fn try_from(value: MemoryRetentionWire) -> Result<Self, Self::Error> {
        Self::validated(value.policy, value.deadline_ms)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryWriteWire {
    id: String,
    kind: MemoryKind,
    scope: MemoryScope,
    content: String,
    confidence: f64,
    provenance: MemoryProvenance,
    #[serde(default)]
    retention: MemoryRetention,
}

impl TryFrom<MemoryWriteWire> for MemoryWrite {
    type Error = MemoryPortError;
    fn try_from(value: MemoryWriteWire) -> Result<Self, Self::Error> {
        let mut write = Self::new(
            value.id,
            value.kind,
            value.scope,
            value.content,
            value.confidence,
            value.provenance,
        )?;
        write.retention =
            MemoryRetention::validated(value.retention.policy, value.retention.deadline_ms)?;
        Ok(write)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryRecordWire {
    id: String,
    kind: MemoryKind,
    scope: MemoryScope,
    content: String,
    confidence: f64,
    provenance: MemoryProvenance,
    revision: u64,
    supersedes: BoundedVec<String, MAX_SUPERSEDES>,
    corrects: Option<String>,
    forgotten_at_ms: Option<u64>,
    forget_reason: Option<String>,
    retention: MemoryRetention,
}

impl TryFrom<MemoryRecordWire> for MemoryRecord {
    type Error = MemoryPortError;
    fn try_from(value: MemoryRecordWire) -> Result<Self, Self::Error> {
        let mut write = MemoryWrite::new(
            value.id,
            value.kind,
            value.scope,
            value.content,
            value.confidence,
            value.provenance,
        )?;
        write.retention = value.retention;
        Self::validated(
            write,
            value.revision,
            value.supersedes.0,
            value.corrects,
            value.forgotten_at_ms,
            value.forget_reason,
        )
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryRevisionWire {
    memory_id: String,
    expected_revision: u64,
    supersedes: BoundedVec<String, MAX_SUPERSEDES>,
    corrects: Option<String>,
    #[serde(default)]
    correction_reason: Option<String>,
    forget_reason: Option<String>,
}

impl TryFrom<MemoryRevisionWire> for MemoryRevision {
    type Error = MemoryPortError;
    fn try_from(value: MemoryRevisionWire) -> Result<Self, Self::Error> {
        Self::validated(
            value.memory_id,
            value.expected_revision,
            value.supersedes.0,
            value.corrects,
            value.correction_reason,
            value.forget_reason,
        )
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryQueryWire {
    text: String,
    requested_scopes: BoundedVec<MemoryScope, MAX_KNOWLEDGE_SCOPES>,
    limit: usize,
}

impl TryFrom<MemoryQueryWire> for MemoryQuery {
    type Error = MemoryPortError;
    fn try_from(value: MemoryQueryWire) -> Result<Self, Self::Error> {
        Self::new(value.text, value.requested_scopes.0, value.limit)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryHitWire {
    record: MemoryRecord,
    score: f64,
    explanation: String,
}

impl TryFrom<MemoryHitWire> for MemoryHit {
    type Error = MemoryPortError;
    fn try_from(value: MemoryHitWire) -> Result<Self, Self::Error> {
        Self::new(value.record, value.score, value.explanation)
    }
}

#[async_trait]
pub trait MemoryPort: Send + Sync {
    async fn write(&self, write: MemoryWrite) -> Result<MemoryRecord, MemoryPortError>;
    async fn query(
        &self,
        access: &KnowledgeAccessContext,
        query: MemoryQuery,
    ) -> Result<MemoryQueryResult, MemoryPortError>;
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

pub(crate) struct BoundedVec<T, const MAX: usize>(pub Vec<T>);

impl<'de, T, const MAX: usize> Deserialize<'de> for BoundedVec<T, MAX>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct BoundedVisitor<T, const MAX: usize>(PhantomData<T>);
        impl<'de, T, const MAX: usize> Visitor<'de> for BoundedVisitor<T, MAX>
        where
            T: Deserialize<'de>,
        {
            type Value = BoundedVec<T, MAX>;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                write!(formatter, "at most {MAX} items")
            }
            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                if sequence.size_hint().is_some_and(|size| size > MAX) {
                    return Err(de::Error::custom("collection exceeds limit"));
                }
                let mut values = Vec::with_capacity(sequence.size_hint().unwrap_or(0).min(MAX));
                while let Some(value) = sequence.next_element()? {
                    if values.len() == MAX {
                        return Err(de::Error::custom("collection exceeds limit"));
                    }
                    values.push(value);
                }
                Ok(BoundedVec(values))
            }
        }
        deserializer.deserialize_seq(BoundedVisitor::<T, MAX>(PhantomData))
    }
}

fn dedup_scopes(scopes: Vec<MemoryScope>) -> Result<Vec<MemoryScope>, MemoryPortError> {
    if scopes.len() > MAX_KNOWLEDGE_SCOPES {
        return Err(MemoryPortError::invalid("too many scopes"));
    }
    let mut unique = Vec::new();
    for scope in scopes {
        if !unique.contains(&scope) {
            unique.push(scope);
        }
    }
    Ok(unique)
}

fn validate_query_access(
    access: &KnowledgeAccessContext,
    requested: &[MemoryScope],
) -> Result<(), MemoryPortError> {
    if requested.iter().all(|scope| access.allows(scope)) {
        Ok(())
    } else {
        Err(MemoryPortError {
            code: MemoryPortErrorCode::UnauthorizedScope,
            message: "requested scope is not authorized".into(),
        })
    }
}

fn scope_allowed(
    access: &KnowledgeAccessContext,
    requested: &[MemoryScope],
    scope: &MemoryScope,
) -> bool {
    access.allows(scope)
        && requested
            .iter()
            .any(|requested| scope.is_authorized_by(requested))
}
