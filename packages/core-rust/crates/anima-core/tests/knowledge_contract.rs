use anima_core::{
    Artifact, ArtifactPort, ArtifactWrite, Citation, Document, DocumentChunk, DocumentChunkPage,
    DocumentPort, Evidence, KnowledgeAccessContext, MemoryHit, MemoryKind, MemoryPort,
    MemoryPortError, MemoryPortErrorCode, MemoryProvenance, MemoryQuery, MemoryQueryResult,
    MemoryRecord, MemoryRetention, MemoryRevision, MemoryScope, MemoryWrite, RetrievalHit,
    RetrievalQuery, RetrievalResult, RetrieverPort,
};
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde_json::{from_str, from_value, json, to_string, to_value, Value};

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

fn assert_rejects<T: DeserializeOwned>(value: Value) {
    assert!(from_value::<T>(value).is_err());
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
        assert_eq!(write.kind(), kind);
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
    assert_eq!(revision.expected_revision(), 3);
    assert_eq!(revision.corrects(), Some("memory-0"));
    assert_eq!(revision.correction_reason(), Some("new source"));
    assert!(revision.supersedes().contains(&"memory-0".to_owned()));
    assert!(MemoryRevision::forget("memory-1", 3, "retention policy").is_ok());
    assert!(record.retention().deadline_ms().is_some());
}

#[test]
fn memory_wire_contract_revalidates_every_domain_value() {
    assert_rejects::<MemoryScope>(json!({ "kind": "owner", "id": "" }));
    assert_rejects::<MemoryScope>(json!({ "kind": "agent", "id": "x".repeat(257) }));
    assert_rejects::<MemoryProvenance>(json!({
        "source": "", "source_identity": "agent-1", "observed_at_ms": 1
    }));
    assert_rejects::<MemoryRetention>(json!({ "policy": "deadline", "deadline_ms": null }));
    assert_rejects::<MemoryRetention>(json!({ "policy": "persistent", "deadline_ms": 1 }));

    let write = MemoryWrite::new(
        "memory-write",
        MemoryKind::Fact,
        MemoryScope::owner("owner-1").unwrap(),
        "content",
        0.5,
        provenance(),
    )
    .unwrap();
    for (field, value) in [
        ("confidence", json!(1.1)),
        ("content", json!("")),
        ("content", json!("x".repeat(32 * 1024 + 1))),
    ] {
        let mut invalid = to_value(&write).unwrap();
        invalid[field] = value;
        assert_rejects::<MemoryWrite>(invalid);
    }
    let mut invalid_scope = to_value(&write).unwrap();
    invalid_scope["scope"]["id"] = json!("");
    assert_rejects::<MemoryWrite>(invalid_scope);
    let mut invalid_retention = to_value(&write).unwrap();
    invalid_retention["retention"] = json!({ "policy": "deadline", "deadline_ms": null });
    assert_rejects::<MemoryWrite>(invalid_retention);

    let record = MemoryRecord::from_write(
        MemoryWrite::new(
            "memory-1",
            MemoryKind::Fact,
            MemoryScope::owner("owner-1").unwrap(),
            "content",
            0.5,
            provenance(),
        )
        .unwrap(),
        1,
    )
    .unwrap();
    for (field, value) in [
        ("id", json!("")),
        ("content", json!("")),
        ("confidence", json!(1.1)),
        ("revision", json!(0)),
    ] {
        let mut invalid = to_value(&record).unwrap();
        invalid[field] = value;
        assert_rejects::<MemoryRecord>(invalid);
    }
    let mut invalid_provenance = to_value(&record).unwrap();
    invalid_provenance["provenance"]["source_identity"] = json!("");
    assert_rejects::<MemoryRecord>(invalid_provenance);
    let mut invalid_supersedes = to_value(&record).unwrap();
    invalid_supersedes["supersedes"] = json!([""]);
    assert_rejects::<MemoryRecord>(invalid_supersedes);

    assert_rejects::<MemoryQuery>(json!({ "text": "query", "requested_scopes": [], "limit": 1 }));
    assert_rejects::<MemoryQuery>(
        json!({ "text": "query", "requested_scopes": [{ "kind": "owner", "id": "owner-1" }], "limit": 0 }),
    );
    assert_rejects::<MemoryRevision>(json!({
        "memory_id": "memory-1", "expected_revision": 1, "supersedes": [],
        "corrects": "memory-0", "correction_reason": null, "forget_reason": null
    }));
    assert_rejects::<MemoryRevision>(json!({
        "memory_id": "memory-1", "expected_revision": 1, "supersedes": ["memory-0"],
        "corrects": null, "correction_reason": null, "forget_reason": "forget"
    }));
    let hit = MemoryHit::new(record, 0.5, "reason").unwrap();
    let mut invalid_hit = to_value(hit).unwrap();
    invalid_hit["explanation"] = json!("");
    assert_rejects::<MemoryHit>(invalid_hit);
}

#[test]
fn evidence_wire_contract_revalidates_every_domain_value() {
    let scope = MemoryScope::workspace("workspace-1").unwrap();
    let document = Document::new("doc-1", scope.clone(), "title", HASH, provenance()).unwrap();
    let chunk =
        DocumentChunk::new("chunk-1", scope.clone(), "doc-1", 0, "content", "line:1").unwrap();
    let citation = Citation::new("doc-1", Some("chunk-1"), "line:1").unwrap();
    let artifact =
        Artifact::new("artifact-1", scope.clone(), "report", HASH, provenance()).unwrap();
    let artifact_write = ArtifactWrite::new(
        "artifact-1",
        scope.clone(),
        "report",
        b"report".to_vec(),
        provenance(),
    )
    .unwrap();

    for (field, value) in [("title", json!("")), ("content_sha256", json!("invalid"))] {
        let mut invalid = to_value(&document).unwrap();
        invalid[field] = value;
        assert_rejects::<Document>(invalid);
    }
    let mut invalid_chunk = to_value(&chunk).unwrap();
    invalid_chunk["content"] = json!("");
    assert_rejects::<DocumentChunk>(invalid_chunk);
    let mut invalid_citation = to_value(&citation).unwrap();
    invalid_citation["locator"] = json!("");
    assert_rejects::<Citation>(invalid_citation);
    let mut invalid_artifact = to_value(&artifact).unwrap();
    invalid_artifact["content_sha256"] = json!("not-a-hash");
    assert_rejects::<Artifact>(invalid_artifact);
    let mut invalid_artifact_write = to_value(&artifact_write).unwrap();
    invalid_artifact_write["name"] = json!("");
    assert_rejects::<ArtifactWrite>(invalid_artifact_write);
    assert_rejects::<RetrievalQuery>(
        json!({ "text": "query", "requested_scopes": [], "limit": 1 }),
    );
    assert_rejects::<RetrievalQuery>(
        json!({ "text": "query", "requested_scopes": [{ "kind": "workspace", "id": "workspace-1" }], "limit": 0 }),
    );
    let hit =
        RetrievalHit::new(Evidence::Document(document), 0.4, "matched", vec![citation]).unwrap();
    let mut invalid_hit = to_value(hit).unwrap();
    invalid_hit["explanation"] = json!("");
    assert_rejects::<RetrievalHit>(invalid_hit);
}

#[test]
fn memory_query_requires_exact_authorized_scope_and_explains_hits() {
    let owner = MemoryScope::owner("owner-1").unwrap();
    let query = MemoryQuery::new("evidence", vec![owner.clone()], 10).unwrap();
    let access = KnowledgeAccessContext::trusted(vec![owner.clone()]).unwrap();
    assert!(access.allows(&owner));
    assert!(!access.allows(&MemoryScope::agent("owner-1").unwrap()));

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
    assert_eq!(hit.explanation(), "matched provenance and content");
    assert!(MemoryHit::new(hit.record().clone(), f64::INFINITY, "bad score").is_err());
    assert_eq!(
        MemoryQueryResult::new(&access, &query, vec![hit])
            .unwrap()
            .hits()
            .len(),
        1
    );
}

#[test]
fn evidence_contract_validates_hashes_locators_and_scope() {
    let scope = MemoryScope::session("session-1").unwrap();
    let document = Document::new("doc-1", scope.clone(), "Plan", HASH, provenance()).unwrap();
    let chunk =
        DocumentChunk::new("chunk-1", scope.clone(), "doc-1", 0, "body", "line:1-2").unwrap();
    let citation = Citation::new("doc-1", Some("chunk-1"), "line:1-2").unwrap();
    let artifact =
        Artifact::new("artifact-1", scope.clone(), "report", HASH, provenance()).unwrap();
    assert_eq!(document.content_sha256(), HASH);
    assert_eq!(chunk.content_sha256().len(), 64);
    assert_eq!(artifact.content_sha256(), HASH);
    assert_eq!(citation.locator(), "line:1-2");
    assert!(Document::new("doc-2", scope, "bad", "not-a-hash", provenance()).is_err());

    let query =
        RetrievalQuery::new("plan", vec![MemoryScope::session("session-1").unwrap()], 5).unwrap();
    let access =
        KnowledgeAccessContext::trusted(vec![MemoryScope::session("session-1").unwrap()]).unwrap();
    let hit =
        RetrievalHit::new(Evidence::Chunk(chunk), 0.8, "lexical match", vec![citation]).unwrap();
    assert_eq!(
        RetrievalResult::new(&access, &query, vec![hit.clone()])
            .unwrap()
            .hits()
            .len(),
        1
    );
    assert_eq!(hit.explanation(), "lexical match");
}

#[test]
fn knowledge_access_and_results_reject_a_deliberately_leaky_port() {
    let owner = MemoryScope::owner("owner-1").unwrap();
    let foreign = MemoryScope::owner("owner-2").unwrap();
    let access = KnowledgeAccessContext::trusted(vec![owner.clone()]).unwrap();
    let query = MemoryQuery::new("secret", vec![owner], 1).unwrap();
    let foreign_record = MemoryRecord::from_write(
        MemoryWrite::new(
            "foreign",
            MemoryKind::Fact,
            foreign,
            "leaked",
            1.0,
            provenance(),
        )
        .unwrap(),
        1,
    )
    .unwrap();
    let leaky = LeakyPort {
        hit: MemoryHit::new(foreign_record, 1.0, "leak").unwrap(),
    };
    assert_eq!(
        futures::executor::block_on(leaky.query(&access, query))
            .unwrap_err()
            .code,
        MemoryPortErrorCode::UnauthorizedScope
    );

    let retrieval_query =
        RetrievalQuery::new("secret", vec![MemoryScope::owner("owner-1").unwrap()], 1).unwrap();
    let leaky_retriever = LeakyRetriever {
        hit: RetrievalHit::new(
            Evidence::Document(
                Document::new(
                    "foreign-doc",
                    MemoryScope::owner("owner-2").unwrap(),
                    "foreign",
                    HASH,
                    provenance(),
                )
                .unwrap(),
            ),
            1.0,
            "leak",
            Vec::new(),
        )
        .unwrap(),
    };
    assert_eq!(
        futures::executor::block_on(leaky_retriever.retrieve(&access, retrieval_query))
            .unwrap_err()
            .code,
        anima_core::EvidencePortErrorCode::UnauthorizedScope
    );
}

#[test]
fn bounded_scope_and_citation_contracts_reject_overflow_and_duplicates() {
    let scope = MemoryScope::owner("owner-1").unwrap();
    let scopes = (0..65)
        .map(|index| json!({ "kind": "owner", "id": format!("owner-{index}") }))
        .collect::<Vec<_>>();
    assert_rejects::<MemoryQuery>(json!({ "text": "q", "requested_scopes": scopes, "limit": 1 }));
    let deduped = MemoryQuery::new("q", vec![scope.clone(), scope], 1).unwrap();
    assert_eq!(deduped.requested_scopes().len(), 1);

    let citation = Citation::new("doc-1", Some("chunk-1"), "line:1").unwrap();
    let evidence = Evidence::Document(
        Document::new(
            "doc-1",
            MemoryScope::owner("owner-1").unwrap(),
            "title",
            HASH,
            provenance(),
        )
        .unwrap(),
    );
    assert!(RetrievalHit::new(evidence, 1.0, "match", vec![citation.clone(), citation]).is_err());
}

#[test]
fn document_chunks_and_artifact_bytes_are_verified_and_bounded() {
    let scope = MemoryScope::workspace("workspace-1").unwrap();
    let chunk = DocumentChunk::new("chunk-1", scope.clone(), "doc-1", 0, "body", "line:1").unwrap();
    assert_ne!(chunk.content_sha256(), HASH);
    assert!(DocumentChunk::new("chunk-2", scope.clone(), "doc-1", 1, "", "line:2").is_err());
    let page = DocumentChunkPage::new("doc-1", &scope, vec![chunk], 1).unwrap();
    assert_eq!(page.chunks().len(), 1);

    let artifact = ArtifactWrite::new(
        "artifact-1",
        scope.clone(),
        "report",
        b"bytes".to_vec(),
        provenance(),
    )
    .unwrap();
    assert_eq!(
        artifact.content_sha256(),
        "277089d91c0bdf4f2e6862ba7e4a07605119431f5d13f726dd352b06f1b206a9"
    );
    let mut tampered = to_value(&artifact).unwrap();
    tampered["content_sha256"] = json!(HASH);
    assert_rejects::<ArtifactWrite>(tampered);
    assert!(ArtifactWrite::new(
        "artifact-2",
        scope,
        "large",
        vec![0; 4 * 1024 * 1024 + 1],
        provenance()
    )
    .is_err());
}

struct ContractPort;

struct LeakyPort {
    hit: MemoryHit,
}

struct LeakyRetriever {
    hit: RetrievalHit,
}

#[async_trait]
impl MemoryPort for ContractPort {
    async fn write(&self, _write: MemoryWrite) -> Result<MemoryRecord, MemoryPortError> {
        Err(MemoryPortError::unsupported("test port"))
    }

    async fn query(
        &self,
        access: &KnowledgeAccessContext,
        query: MemoryQuery,
    ) -> Result<MemoryQueryResult, MemoryPortError> {
        MemoryQueryResult::new(access, &query, Vec::new())
    }

    async fn revise(&self, _revision: MemoryRevision) -> Result<MemoryRecord, MemoryPortError> {
        Err(MemoryPortError::unsupported("test port"))
    }
}

#[async_trait]
impl MemoryPort for LeakyPort {
    async fn write(&self, _write: MemoryWrite) -> Result<MemoryRecord, MemoryPortError> {
        Err(MemoryPortError::unsupported("test"))
    }
    async fn query(
        &self,
        access: &KnowledgeAccessContext,
        query: MemoryQuery,
    ) -> Result<MemoryQueryResult, MemoryPortError> {
        MemoryQueryResult::new(access, &query, vec![self.hit.clone()])
    }
    async fn revise(&self, _revision: MemoryRevision) -> Result<MemoryRecord, MemoryPortError> {
        Err(MemoryPortError::unsupported("test"))
    }
}

#[async_trait]
impl RetrieverPort for LeakyRetriever {
    async fn retrieve(
        &self,
        access: &KnowledgeAccessContext,
        query: RetrievalQuery,
    ) -> Result<RetrievalResult, anima_core::EvidencePortError> {
        RetrievalResult::new(access, &query, vec![self.hit.clone()])
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

    async fn write_chunk(
        &self,
        _chunk: DocumentChunk,
    ) -> Result<DocumentChunk, anima_core::EvidencePortError> {
        Err(anima_core::EvidencePortError::unsupported("test"))
    }
    async fn get_chunk(
        &self,
        _id: &str,
        _scope: &MemoryScope,
    ) -> Result<Option<DocumentChunk>, anima_core::EvidencePortError> {
        Ok(None)
    }
    async fn list_chunks(
        &self,
        document_id: &str,
        scope: &MemoryScope,
        limit: usize,
    ) -> Result<DocumentChunkPage, anima_core::EvidencePortError> {
        DocumentChunkPage::new(document_id, scope, Vec::new(), limit)
    }
}

#[async_trait]
impl RetrieverPort for ContractPort {
    async fn retrieve(
        &self,
        access: &KnowledgeAccessContext,
        query: RetrievalQuery,
    ) -> Result<RetrievalResult, anima_core::EvidencePortError> {
        RetrievalResult::new(access, &query, Vec::new())
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
