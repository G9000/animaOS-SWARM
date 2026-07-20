//! Storage-neutral document, evidence, and artifact contracts.

use std::fmt;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::memory::{
    BoundedVec, KnowledgeAccessContext, MemoryProvenance, MemoryScope, MAX_KNOWLEDGE_HITS,
    MAX_KNOWLEDGE_SCOPES,
};

const MAX_ID_BYTES: usize = 256;
const MAX_LABEL_BYTES: usize = 4 * 1024;
const MAX_LOCATOR_BYTES: usize = 4 * 1024;
const MAX_CHUNK_BYTES: usize = 64 * 1024;
pub const MAX_ARTIFACT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_CITATIONS: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "DocumentWire")]
pub struct Document {
    id: String,
    scope: MemoryScope,
    title: String,
    content_sha256: String,
    provenance: MemoryProvenance,
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

    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn scope(&self) -> &MemoryScope {
        &self.scope
    }
    pub fn title(&self) -> &str {
        &self.title
    }
    pub fn content_sha256(&self) -> &str {
        &self.content_sha256
    }
    pub fn provenance(&self) -> &MemoryProvenance {
        &self.provenance
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "DocumentChunkWire")]
pub struct DocumentChunk {
    id: String,
    scope: MemoryScope,
    document_id: String,
    index: u32,
    /// Content is bounded plain text; credential material must never be passed to this API.
    content: String,
    content_sha256: String,
    locator: String,
}

impl DocumentChunk {
    pub fn new(
        id: impl Into<String>,
        scope: MemoryScope,
        document_id: impl Into<String>,
        index: u32,
        content: impl Into<String>,
        locator: impl Into<String>,
    ) -> Result<Self, EvidencePortError> {
        Ok(Self {
            id: required(id.into(), "chunk ID")?,
            scope,
            document_id: required(document_id.into(), "document ID")?,
            index,
            content: bounded(content.into(), "document chunk content", MAX_CHUNK_BYTES)?,
            content_sha256: String::new(),
            locator: bounded(locator.into(), "citation locator", MAX_LOCATOR_BYTES)?,
        }
        .with_content_hash())
    }

    fn with_content_hash(mut self) -> Self {
        self.content_sha256 = sha256_bytes(self.content.as_bytes());
        self
    }

    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn scope(&self) -> &MemoryScope {
        &self.scope
    }
    pub fn document_id(&self) -> &str {
        &self.document_id
    }
    pub fn index(&self) -> u32 {
        self.index
    }
    pub fn content(&self) -> &str {
        &self.content
    }
    pub fn content_sha256(&self) -> &str {
        &self.content_sha256
    }
    pub fn locator(&self) -> &str {
        &self.locator
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "CitationWire")]
pub struct Citation {
    document_id: String,
    chunk_id: Option<String>,
    locator: String,
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

    pub fn document_id(&self) -> &str {
        &self.document_id
    }
    pub fn chunk_id(&self) -> Option<&str> {
        self.chunk_id.as_deref()
    }
    pub fn locator(&self) -> &str {
        &self.locator
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ArtifactWire")]
pub struct Artifact {
    id: String,
    scope: MemoryScope,
    name: String,
    content_sha256: String,
    provenance: MemoryProvenance,
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

    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn scope(&self) -> &MemoryScope {
        &self.scope
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn content_sha256(&self) -> &str {
        &self.content_sha256
    }
    pub fn provenance(&self) -> &MemoryProvenance {
        &self.provenance
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ArtifactWriteWire")]
pub struct ArtifactWrite {
    id: String,
    scope: MemoryScope,
    name: String,
    content_sha256: String,
    content: Vec<u8>,
    provenance: MemoryProvenance,
}

impl ArtifactWrite {
    pub fn new(
        id: impl Into<String>,
        scope: MemoryScope,
        name: impl Into<String>,
        content: Vec<u8>,
        provenance: MemoryProvenance,
    ) -> Result<Self, EvidencePortError> {
        if content.len() > MAX_ARTIFACT_BYTES {
            return Err(EvidencePortError::invalid(
                "artifact content exceeds byte limit",
            ));
        }
        let artifact = Artifact::new(id, scope, name, sha256_bytes(&content), provenance)?;
        Ok(Self {
            id: artifact.id,
            scope: artifact.scope,
            name: artifact.name,
            content_sha256: artifact.content_sha256,
            content,
            provenance: artifact.provenance,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn scope(&self) -> &MemoryScope {
        &self.scope
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn content_sha256(&self) -> &str {
        &self.content_sha256
    }
    pub fn content(&self) -> &[u8] {
        &self.content
    }
    pub fn provenance(&self) -> &MemoryProvenance {
        &self.provenance
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
    text: String,
    requested_scopes: Vec<MemoryScope>,
    limit: usize,
}

impl RetrievalQuery {
    pub fn new(
        text: impl Into<String>,
        requested_scopes: Vec<MemoryScope>,
        limit: usize,
    ) -> Result<Self, EvidencePortError> {
        let requested_scopes = dedup_scopes(requested_scopes)?;
        if requested_scopes.is_empty() {
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
            requested_scopes,
            limit,
        })
    }

    pub fn requested_scopes(&self) -> &[MemoryScope] {
        &self.requested_scopes
    }

    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn limit(&self) -> usize {
        self.limit
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RetrievalHitWire")]
pub struct RetrievalHit {
    evidence: Evidence,
    score: f64,
    explanation: String,
    citations: Vec<Citation>,
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
        if citations.len() > MAX_CITATIONS || has_duplicate_citations(&citations) {
            return Err(EvidencePortError::invalid(
                "citations must be unique and within the limit",
            ));
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

    pub fn evidence(&self) -> &Evidence {
        &self.evidence
    }
    pub fn score(&self) -> f64 {
        self.score
    }
    pub fn explanation(&self) -> &str {
        &self.explanation
    }
    pub fn citations(&self) -> &[Citation] {
        &self.citations
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentChunkPage {
    chunks: Vec<DocumentChunk>,
}

impl DocumentChunkPage {
    pub fn new(
        document_id: &str,
        scope: &MemoryScope,
        chunks: Vec<DocumentChunk>,
        limit: usize,
    ) -> Result<Self, EvidencePortError> {
        if limit == 0 || limit > MAX_KNOWLEDGE_HITS || chunks.len() > limit {
            return Err(EvidencePortError::invalid("chunk page exceeds limit"));
        }
        let mut previous = None;
        let mut ids = std::collections::BTreeSet::new();
        for chunk in &chunks {
            if chunk.document_id != document_id
                || &chunk.scope != scope
                || !ids.insert(chunk.id.clone())
                || previous.is_some_and(|index| chunk.index <= index)
            {
                return Err(EvidencePortError::invalid(
                    "chunk page must be scoped, unique, and ascending",
                ));
            }
            previous = Some(chunk.index);
        }
        Ok(Self { chunks })
    }
    pub fn chunks(&self) -> &[DocumentChunk] {
        &self.chunks
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RetrievalResult {
    hits: Vec<RetrievalHit>,
}

impl RetrievalResult {
    pub fn new(
        access: &KnowledgeAccessContext,
        query: &RetrievalQuery,
        hits: Vec<RetrievalHit>,
    ) -> Result<Self, EvidencePortError> {
        if query
            .requested_scopes()
            .iter()
            .any(|scope| !access.allows(scope))
        {
            return Err(EvidencePortError {
                code: EvidencePortErrorCode::UnauthorizedScope,
                message: "requested scope is not authorized".into(),
            });
        }
        if hits.len() > query.limit || hits.len() > MAX_KNOWLEDGE_HITS {
            return Err(EvidencePortError::invalid(
                "retrieval result exceeds query limit",
            ));
        }
        let mut ids = std::collections::BTreeSet::new();
        for hit in &hits {
            let Some(scope) = hit.scope() else {
                return Err(EvidencePortError::invalid(
                    "retrieval evidence must have a scope",
                ));
            };
            if !ids.insert(evidence_id(&hit.evidence).to_owned()) {
                return Err(EvidencePortError::invalid(
                    "retrieval result contains duplicate evidence",
                ));
            }
            if !access.allows(scope)
                || !query
                    .requested_scopes()
                    .iter()
                    .any(|requested| scope.is_authorized_by(requested))
            {
                return Err(EvidencePortError {
                    code: EvidencePortErrorCode::UnauthorizedScope,
                    message: "retrieval result contains an unauthorized scope".into(),
                });
            }
        }
        Ok(Self { hits })
    }
    pub fn hits(&self) -> &[RetrievalHit] {
        &self.hits
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
            value.locator,
        )
        .and_then(|chunk| {
            if chunk.content_sha256 == value.content_sha256 {
                Ok(chunk)
            } else {
                Err(EvidencePortError::invalid(
                    "document chunk hash does not match content",
                ))
            }
        })
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
    content: BoundedVec<u8, MAX_ARTIFACT_BYTES>,
    provenance: MemoryProvenance,
}
impl TryFrom<ArtifactWriteWire> for ArtifactWrite {
    type Error = EvidencePortError;
    fn try_from(value: ArtifactWriteWire) -> Result<Self, Self::Error> {
        Self::new(
            value.id,
            value.scope,
            value.name,
            value.content.0,
            value.provenance,
        )
        .and_then(|artifact| {
            if artifact.content_sha256 == value.content_sha256 {
                Ok(artifact)
            } else {
                Err(EvidencePortError::invalid(
                    "artifact content hash does not match bytes",
                ))
            }
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RetrievalQueryWire {
    text: String,
    requested_scopes: BoundedVec<MemoryScope, MAX_KNOWLEDGE_SCOPES>,
    limit: usize,
}
impl TryFrom<RetrievalQueryWire> for RetrievalQuery {
    type Error = EvidencePortError;
    fn try_from(value: RetrievalQueryWire) -> Result<Self, Self::Error> {
        Self::new(value.text, value.requested_scopes.0, value.limit)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RetrievalHitWire {
    evidence: Evidence,
    score: f64,
    explanation: String,
    citations: BoundedVec<Citation, MAX_CITATIONS>,
}
impl TryFrom<RetrievalHitWire> for RetrievalHit {
    type Error = EvidencePortError;
    fn try_from(value: RetrievalHitWire) -> Result<Self, Self::Error> {
        Self::new(
            value.evidence,
            value.score,
            value.explanation,
            value.citations.0,
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
    async fn write_chunk(&self, chunk: DocumentChunk) -> Result<DocumentChunk, EvidencePortError>;
    async fn get_chunk(
        &self,
        id: &str,
        scope: &MemoryScope,
    ) -> Result<Option<DocumentChunk>, EvidencePortError>;
    async fn list_chunks(
        &self,
        document_id: &str,
        scope: &MemoryScope,
        limit: usize,
    ) -> Result<DocumentChunkPage, EvidencePortError>;
}

#[async_trait]
pub trait RetrieverPort: Send + Sync {
    async fn retrieve(
        &self,
        access: &KnowledgeAccessContext,
        query: RetrievalQuery,
    ) -> Result<RetrievalResult, EvidencePortError>;
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

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn dedup_scopes(scopes: Vec<MemoryScope>) -> Result<Vec<MemoryScope>, EvidencePortError> {
    if scopes.len() > MAX_KNOWLEDGE_SCOPES {
        return Err(EvidencePortError::invalid("too many scopes"));
    }
    let mut unique = Vec::new();
    for scope in scopes {
        if !unique.contains(&scope) {
            unique.push(scope);
        }
    }
    Ok(unique)
}

fn has_duplicate_citations(citations: &[Citation]) -> bool {
    let mut seen = std::collections::BTreeSet::new();
    citations.iter().any(|citation| {
        !seen.insert((
            citation.document_id.clone(),
            citation.chunk_id.clone(),
            citation.locator.clone(),
        ))
    })
}

fn evidence_id(evidence: &Evidence) -> &str {
    match evidence {
        Evidence::Document(document) => &document.id,
        Evidence::Chunk(chunk) => &chunk.id,
        Evidence::Artifact(artifact) => &artifact.id,
    }
}
