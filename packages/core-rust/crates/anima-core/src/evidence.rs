//! Storage-neutral document, evidence, and artifact contracts.

use std::fmt;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::memory::{MemoryProvenance, MemoryScope};

const MAX_ID_BYTES: usize = 256;
const MAX_LABEL_BYTES: usize = 4 * 1024;
const MAX_LOCATOR_BYTES: usize = 4 * 1024;
const MAX_CHUNK_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "DocumentWire")]
pub struct Document {
    pub id: String,
    pub scope: MemoryScope,
    pub title: String,
    pub content_sha256: String,
    pub provenance: MemoryProvenance,
}

impl Document {
    pub fn new(
        id: impl Into<String>,
        scope: MemoryScope,
        title: impl Into<String>,
        content_sha256: impl Into<String>,
        provenance: MemoryProvenance,
    ) -> Result<Self, EvidencePortError> {
        Ok(Self {
            id: required(id.into(), "document ID")?,
            scope,
            title: bounded(title.into(), "document title", MAX_LABEL_BYTES)?,
            content_sha256: sha256(content_sha256.into())?,
            provenance,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "DocumentChunkWire")]
pub struct DocumentChunk {
    pub id: String,
    pub scope: MemoryScope,
    pub document_id: String,
    pub index: u32,
    /// Content is bounded plain text; credential material must never be passed to this API.
    pub content: String,
    pub content_sha256: String,
    pub locator: String,
}

impl DocumentChunk {
    pub fn new(
        id: impl Into<String>,
        scope: MemoryScope,
        document_id: impl Into<String>,
        index: u32,
        content: impl Into<String>,
        content_sha256: impl Into<String>,
        locator: impl Into<String>,
    ) -> Result<Self, EvidencePortError> {
        Ok(Self {
            id: required(id.into(), "chunk ID")?,
            scope,
            document_id: required(document_id.into(), "document ID")?,
            index,
            content: bounded(content.into(), "document chunk content", MAX_CHUNK_BYTES)?,
            content_sha256: sha256(content_sha256.into())?,
            locator: bounded(locator.into(), "citation locator", MAX_LOCATOR_BYTES)?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "CitationWire")]
pub struct Citation {
    pub document_id: String,
    pub chunk_id: Option<String>,
    pub locator: String,
}

impl Citation {
    pub fn new(
        document_id: impl Into<String>,
        chunk_id: Option<impl Into<String>>,
        locator: impl Into<String>,
    ) -> Result<Self, EvidencePortError> {
        Ok(Self {
            document_id: required(document_id.into(), "document ID")?,
            chunk_id: chunk_id
                .map(|id| required(id.into(), "chunk ID"))
                .transpose()?,
            locator: bounded(locator.into(), "citation locator", MAX_LOCATOR_BYTES)?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ArtifactWire")]
pub struct Artifact {
    pub id: String,
    pub scope: MemoryScope,
    pub name: String,
    pub content_sha256: String,
    pub provenance: MemoryProvenance,
}

impl Artifact {
    pub fn new(
        id: impl Into<String>,
        scope: MemoryScope,
        name: impl Into<String>,
        content_sha256: impl Into<String>,
        provenance: MemoryProvenance,
    ) -> Result<Self, EvidencePortError> {
        Ok(Self {
            id: required(id.into(), "artifact ID")?,
            scope,
            name: bounded(name.into(), "artifact name", MAX_LABEL_BYTES)?,
            content_sha256: sha256(content_sha256.into())?,
            provenance,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ArtifactWriteWire")]
pub struct ArtifactWrite {
    pub id: String,
    pub scope: MemoryScope,
    pub name: String,
    pub content_sha256: String,
    pub provenance: MemoryProvenance,
}

impl ArtifactWrite {
    pub fn new(
        id: impl Into<String>,
        scope: MemoryScope,
        name: impl Into<String>,
        content_sha256: impl Into<String>,
        provenance: MemoryProvenance,
    ) -> Result<Self, EvidencePortError> {
        let artifact = Artifact::new(id, scope, name, content_sha256, provenance)?;
        Ok(Self {
            id: artifact.id,
            scope: artifact.scope,
            name: artifact.name,
            content_sha256: artifact.content_sha256,
            provenance: artifact.provenance,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Evidence {
    Document(Document),
    Chunk(DocumentChunk),
    Artifact(Artifact),
}

impl Evidence {
    pub fn scope(&self) -> Option<&MemoryScope> {
        match self {
            Self::Document(document) => Some(&document.scope),
            Self::Artifact(artifact) => Some(&artifact.scope),
            Self::Chunk(chunk) => Some(&chunk.scope),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RetrievalQueryWire")]
pub struct RetrievalQuery {
    pub text: String,
    pub authorized_scopes: Vec<MemoryScope>,
    pub limit: usize,
}

impl RetrievalQuery {
    pub fn new(
        text: impl Into<String>,
        authorized_scopes: Vec<MemoryScope>,
        limit: usize,
    ) -> Result<Self, EvidencePortError> {
        if authorized_scopes.is_empty() {
            return Err(EvidencePortError::invalid(
                "at least one authorized scope is required",
            ));
        }
        if limit == 0 || limit > 1000 {
            return Err(EvidencePortError::invalid(
                "retrieval limit must be between 1 and 1000",
            ));
        }
        Ok(Self {
            text: bounded(text.into(), "retrieval query text", MAX_LABEL_BYTES)?,
            authorized_scopes,
            limit,
        })
    }

    pub fn authorizes(&self, scope: &Option<&MemoryScope>) -> bool {
        scope.is_some_and(|scope| {
            self.authorized_scopes
                .iter()
                .any(|authorized| scope.is_authorized_by(authorized))
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RetrievalHitWire")]
pub struct RetrievalHit {
    pub evidence: Evidence,
    pub score: f64,
    pub explanation: String,
    pub citations: Vec<Citation>,
}

impl RetrievalHit {
    pub fn new(
        evidence: Evidence,
        score: f64,
        explanation: impl Into<String>,
        citations: Vec<Citation>,
    ) -> Result<Self, EvidencePortError> {
        if !score.is_finite() {
            return Err(EvidencePortError::invalid("retrieval score must be finite"));
        }
        Ok(Self {
            evidence,
            score,
            explanation: bounded(explanation.into(), "retrieval explanation", MAX_LABEL_BYTES)?,
            citations,
        })
    }

    pub fn scope(&self) -> Option<&MemoryScope> {
        self.evidence.scope()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidencePortErrorCode {
    InvalidInput,
    UnauthorizedScope,
    NotFound,
    Conflict,
    Unsupported,
    Backend,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidencePortError {
    pub code: EvidencePortErrorCode,
    pub message: String,
}

impl EvidencePortError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: EvidencePortErrorCode::InvalidInput,
            message: message.into(),
        }
    }
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self {
            code: EvidencePortErrorCode::Unsupported,
            message: message.into(),
        }
    }
}

impl fmt::Display for EvidencePortError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.code, self.message)
    }
}
impl std::error::Error for EvidencePortError {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentWire {
    id: String,
    scope: MemoryScope,
    title: String,
    content_sha256: String,
    provenance: MemoryProvenance,
}
impl TryFrom<DocumentWire> for Document {
    type Error = EvidencePortError;
    fn try_from(value: DocumentWire) -> Result<Self, Self::Error> {
        Self::new(
            value.id,
            value.scope,
            value.title,
            value.content_sha256,
            value.provenance,
        )
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentChunkWire {
    id: String,
    scope: MemoryScope,
    document_id: String,
    index: u32,
    content: String,
    content_sha256: String,
    locator: String,
}
impl TryFrom<DocumentChunkWire> for DocumentChunk {
    type Error = EvidencePortError;
    fn try_from(value: DocumentChunkWire) -> Result<Self, Self::Error> {
        Self::new(
            value.id,
            value.scope,
            value.document_id,
            value.index,
            value.content,
            value.content_sha256,
            value.locator,
        )
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CitationWire {
    document_id: String,
    chunk_id: Option<String>,
    locator: String,
}
impl TryFrom<CitationWire> for Citation {
    type Error = EvidencePortError;
    fn try_from(value: CitationWire) -> Result<Self, Self::Error> {
        Self::new(value.document_id, value.chunk_id, value.locator)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArtifactWire {
    id: String,
    scope: MemoryScope,
    name: String,
    content_sha256: String,
    provenance: MemoryProvenance,
}
impl TryFrom<ArtifactWire> for Artifact {
    type Error = EvidencePortError;
    fn try_from(value: ArtifactWire) -> Result<Self, Self::Error> {
        Self::new(
            value.id,
            value.scope,
            value.name,
            value.content_sha256,
            value.provenance,
        )
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArtifactWriteWire {
    id: String,
    scope: MemoryScope,
    name: String,
    content_sha256: String,
    provenance: MemoryProvenance,
}
impl TryFrom<ArtifactWriteWire> for ArtifactWrite {
    type Error = EvidencePortError;
    fn try_from(value: ArtifactWriteWire) -> Result<Self, Self::Error> {
        Self::new(
            value.id,
            value.scope,
            value.name,
            value.content_sha256,
            value.provenance,
        )
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RetrievalQueryWire {
    text: String,
    authorized_scopes: Vec<MemoryScope>,
    limit: usize,
}
impl TryFrom<RetrievalQueryWire> for RetrievalQuery {
    type Error = EvidencePortError;
    fn try_from(value: RetrievalQueryWire) -> Result<Self, Self::Error> {
        Self::new(value.text, value.authorized_scopes, value.limit)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RetrievalHitWire {
    evidence: Evidence,
    score: f64,
    explanation: String,
    citations: Vec<Citation>,
}
impl TryFrom<RetrievalHitWire> for RetrievalHit {
    type Error = EvidencePortError;
    fn try_from(value: RetrievalHitWire) -> Result<Self, Self::Error> {
        Self::new(
            value.evidence,
            value.score,
            value.explanation,
            value.citations,
        )
    }
}

#[async_trait]
pub trait DocumentPort: Send + Sync {
    async fn write_document(&self, document: Document) -> Result<Document, EvidencePortError>;
    async fn get_document(
        &self,
        id: &str,
        scope: &MemoryScope,
    ) -> Result<Option<Document>, EvidencePortError>;
}

#[async_trait]
pub trait RetrieverPort: Send + Sync {
    async fn retrieve(&self, query: RetrievalQuery)
        -> Result<Vec<RetrievalHit>, EvidencePortError>;
}

#[async_trait]
pub trait ArtifactPort: Send + Sync {
    async fn write_artifact(&self, artifact: ArtifactWrite) -> Result<Artifact, EvidencePortError>;
    async fn get_artifact(
        &self,
        id: &str,
        scope: &MemoryScope,
    ) -> Result<Option<Artifact>, EvidencePortError>;
}

fn required(value: String, field: &str) -> Result<String, EvidencePortError> {
    bounded(value, field, MAX_ID_BYTES)
}
fn bounded(value: String, field: &str, max_bytes: usize) -> Result<String, EvidencePortError> {
    let value = value.trim().to_owned();
    if value.is_empty() || value.len() > max_bytes {
        Err(EvidencePortError::invalid(format!(
            "{field} must be non-empty and at most {max_bytes} bytes"
        )))
    } else {
        Ok(value)
    }
}
fn sha256(value: String) -> Result<String, EvidencePortError> {
    let value = value.trim().to_ascii_lowercase();
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(value)
    } else {
        Err(EvidencePortError::invalid(
            "content SHA-256 must be 64 hexadecimal characters",
        ))
    }
}
