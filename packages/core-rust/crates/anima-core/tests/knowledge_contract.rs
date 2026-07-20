use anima_core::{
    Artifact, ArtifactPort, ArtifactWrite, Citation, Document, DocumentChunk, DocumentPort,
    Evidence, MemoryHit, MemoryKind, MemoryPort, MemoryPortError, MemoryProvenance, MemoryQuery,
    MemoryRecord, MemoryRevision, MemoryScope, MemoryWrite, RetrievalHit, RetrievalQuery,
    RetrieverPort,
};
use async_trait::async_trait;
use serde_json::{from_str, to_string};

const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn provenance() -> MemoryProvenance {
    MemoryProvenance::new("legacy-memory", "agent:ada", 1_700_000_000_000).unwrap()
}

fn scopes() -> Vec<MemoryScope> {
    vec![
        MemoryScope::owner("owner-1").unwrap(),
        MemoryScope::agent("agent-1").unwrap(),
        MemoryScope::session("session-1").unwrap(),
        MemoryScope::workspace("workspace-1").unwrap(),
    ]
}

#[test]
fn memory_contract_supports_every_kind_and_scope() {
    let scope = MemoryScope::owner("owner-1").unwrap();
    for kind in [
        MemoryKind::Fact,
        MemoryKind::Preference,
        MemoryKind::Episode,
        MemoryKind::Procedure,
        MemoryKind::Reflection,
    ] {
        let write = MemoryWrite::new(
            format!("{kind:?}"),
            kind,
            scope.clone(),
            "validated memory content",
            0.75,
            provenance(),
        )
        .unwrap();
        assert_eq!(write.kind, kind);
    }

    let scopes = scopes();
    assert!(scopes.iter().all(|scope| scope.is_authorized_by(scope)));
    assert!(!scopes[0].is_authorized_by(&scopes[1]));
    assert!(!scopes[2].is_authorized_by(&MemoryScope::session("session-2").unwrap()));
}

#[test]
fn memory_contract_serde_and_lifecycle_fields_are_fail_closed() {
    let scope = MemoryScope::workspace("workspace-1").unwrap();
    let write = MemoryWrite::new(
        "memory-1",
        MemoryKind::Fact,
        scope.clone(),
        "I prefer evidence over guesses.",
        0.9,
        provenance(),
    )
    .unwrap()
    .with_retention_deadline(1_800_000_000_000)
    .unwrap();
    let record = MemoryRecord::from_write(write, 3).unwrap();
    assert_eq!(
        from_str::<MemoryRecord>(&to_string(&record).unwrap()).unwrap(),
        record
    );
    assert!(MemoryWrite::new(
        "memory-2",
        MemoryKind::Fact,
        scope,
        "content",
        f64::NAN,
        provenance(),
    )
    .is_err());

    let revision = MemoryRevision::correction("memory-1", 3, "memory-0", "new source").unwrap();
    assert_eq!(revision.expected_revision, 3);
    assert_eq!(revision.corrects.as_deref(), Some("memory-0"));
    assert!(revision.supersedes.contains(&"memory-0".to_owned()));
    assert!(MemoryRevision::forget("memory-1", 3, "retention policy").is_ok());
    assert!(record.retention.deadline_ms.is_some());
}

#[test]
fn memory_query_requires_exact_authorized_scope_and_explains_hits() {
    let owner = MemoryScope::owner("owner-1").unwrap();
    let query = MemoryQuery::new("evidence", vec![owner.clone()], 10).unwrap();
    assert!(query.authorizes(&owner));
    assert!(!query.authorizes(&MemoryScope::agent("owner-1").unwrap()));

    let record = MemoryRecord::from_write(
        MemoryWrite::new(
            "memory-1",
            MemoryKind::Reflection,
            owner,
            "explainable result",
            0.5,
            provenance(),
        )
        .unwrap(),
        1,
    )
    .unwrap();
    let hit = MemoryHit::new(record, 0.5, "matched provenance and content").unwrap();
    assert_eq!(hit.explanation, "matched provenance and content");
    assert!(MemoryHit::new(hit.record.clone(), f64::INFINITY, "bad score").is_err());
}

#[test]
fn evidence_contract_validates_hashes_locators_and_scope() {
    let scope = MemoryScope::session("session-1").unwrap();
    let document = Document::new("doc-1", scope.clone(), "Plan", HASH, provenance()).unwrap();
    let chunk = DocumentChunk::new(
        "chunk-1",
        scope.clone(),
        "doc-1",
        0,
        "body",
        HASH,
        "line:1-2",
    )
    .unwrap();
    let citation = Citation::new("doc-1", Some("chunk-1"), "line:1-2").unwrap();
    let artifact =
        Artifact::new("artifact-1", scope.clone(), "report", HASH, provenance()).unwrap();
    assert_eq!(document.content_sha256, HASH);
    assert_eq!(chunk.content_sha256, HASH);
    assert_eq!(artifact.content_sha256, HASH);
    assert_eq!(citation.locator, "line:1-2");
    assert!(Document::new("doc-2", scope, "bad", "not-a-hash", provenance()).is_err());

    let query =
        RetrievalQuery::new("plan", vec![MemoryScope::session("session-1").unwrap()], 5).unwrap();
    let hit =
        RetrievalHit::new(Evidence::Chunk(chunk), 0.8, "lexical match", vec![citation]).unwrap();
    assert!(query.authorizes(&hit.scope()));
    assert_eq!(hit.explanation, "lexical match");
}

struct ContractPort;

#[async_trait]
impl MemoryPort for ContractPort {
    async fn write(&self, _write: MemoryWrite) -> Result<MemoryRecord, MemoryPortError> {
        Err(MemoryPortError::unsupported("test port"))
    }

    async fn query(&self, _query: MemoryQuery) -> Result<Vec<MemoryHit>, MemoryPortError> {
        Ok(Vec::new())
    }

    async fn revise(&self, _revision: MemoryRevision) -> Result<MemoryRecord, MemoryPortError> {
        Err(MemoryPortError::unsupported("test port"))
    }
}

#[async_trait]
impl DocumentPort for ContractPort {
    async fn write_document(
        &self,
        _document: Document,
    ) -> Result<Document, anima_core::EvidencePortError> {
        Err(anima_core::EvidencePortError::unsupported("test port"))
    }

    async fn get_document(
        &self,
        _id: &str,
        _scope: &MemoryScope,
    ) -> Result<Option<Document>, anima_core::EvidencePortError> {
        Ok(None)
    }
}

#[async_trait]
impl RetrieverPort for ContractPort {
    async fn retrieve(
        &self,
        _query: RetrievalQuery,
    ) -> Result<Vec<RetrievalHit>, anima_core::EvidencePortError> {
        Ok(Vec::new())
    }
}

#[async_trait]
impl ArtifactPort for ContractPort {
    async fn write_artifact(
        &self,
        _artifact: ArtifactWrite,
    ) -> Result<Artifact, anima_core::EvidencePortError> {
        Err(anima_core::EvidencePortError::unsupported("test port"))
    }

    async fn get_artifact(
        &self,
        _id: &str,
        _scope: &MemoryScope,
    ) -> Result<Option<Artifact>, anima_core::EvidencePortError> {
        Ok(None)
    }
}

#[tokio::test]
async fn port_traits_are_async_send_sync_contracts() {
    fn assert_memory_port<T: MemoryPort + Send + Sync>() {}
    fn assert_document_port<T: DocumentPort + Send + Sync>() {}
    fn assert_retriever_port<T: RetrieverPort + Send + Sync>() {}
    fn assert_artifact_port<T: ArtifactPort + Send + Sync>() {}
    assert_memory_port::<ContractPort>();
    assert_document_port::<ContractPort>();
    assert_retriever_port::<ContractPort>();
    assert_artifact_port::<ContractPort>();
}
