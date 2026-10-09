use super::*;

use anima_memory::{
    Memory, MemoryError, MemoryManager, MemorySearchOptions, MemoryType, MemoryVectorIndex,
    NewMemory,
};
use serde_json::{json, Value};

use crate::memory_embeddings::MemoryEmbeddingRuntime;
use crate::memory_store::MemoryStoreConfig;
use crate::memory_text::{
    FACT_VALUE_INVALID, MEMORY_CONTENT_INVALID, MEMORY_TAGS_INVALID, MEMORY_TEXT_HIDDEN,
};
use crate::routes::memory_edits::{
    sync_embedding_with, ENTITY_KIND_REQUIRED, ENTITY_OWNS_MEMORIES, FACTS_LIMIT_INVALID,
    FACT_NOT_EDITABLE, INCLUDE_INACTIVE_INVALID, MEMORY_PATCH_EMPTY, MEMORY_TASK_FAILED,
};
use anima_memory::{NewTemporalFact, RelationshipEndpointKind, TemporalFact, TemporalFactOptions};

const OWNER_ORIGIN: &str = "http://localhost:4200";
const FOREIGN_ORIGIN: &str = "http://evil.example";

fn request(method: &str, uri: &str, origin: &str, body: Option<Value>) -> Request<Body> {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "127.0.0.1:8080")
        .header("origin", origin);
    match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

async fn send(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> axum::response::Response {
    app.clone()
        .oneshot(request(method, uri, OWNER_ORIGIN, body))
        .await
        .unwrap()
}

fn daemon() -> Arc<RwLock<DaemonState>> {
    Arc::new(RwLock::new(DaemonState::new()))
}

/// Adds a memory through `POST /api/memories` (which also embeds it).
async fn seed(app: &axum::Router, content: &str, tags: Option<Value>) -> Value {
    let mut body = json!({
        "agentId": "companion",
        "agentName": "Companion",
        "type": "fact",
        "content": content,
        "importance": 0.5,
    });
    if let Some(tags) = tags {
        body["tags"] = tags;
    }
    let response = send(app, "POST", "/api/memories", Some(body)).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    json_body(response).await
}

async fn stored(state: &Arc<RwLock<DaemonState>>, id: &str) -> Option<Memory> {
    let memory = state.read().await.memory_handle();
    let guard = memory.read().await;
    guard.get(id)
}

async fn search_ids(state: &Arc<RwLock<DaemonState>>, query: &str) -> Vec<String> {
    let memory = state.read().await.memory_handle();
    let guard = memory.read().await;
    guard
        .search(query, MemorySearchOptions::default())
        .into_iter()
        .map(|result| result.id)
        .collect()
}

async fn vector_hits(state: &Arc<RwLock<DaemonState>>, query: &str) -> Vec<String> {
    let embeddings = state.read().await.memory_embeddings_handle();
    let guard = embeddings.read().await;
    guard
        .search(query, 5)
        .into_iter()
        .map(|hit| hit.memory_id)
        .collect()
}

async fn vector_count(state: &Arc<RwLock<DaemonState>>) -> usize {
    let embeddings = state.read().await.memory_embeddings_handle();
    let count = embeddings.read().await.status().vector_count;
    count
}

fn memory_uri(id: &str) -> String {
    format!("/api/memories/{id}")
}

fn new_memory(content: &str) -> NewMemory {
    NewMemory {
        agent_id: "companion".into(),
        agent_name: "Companion".into(),
        memory_type: MemoryType::Fact,
        content: content.into(),
        importance: 0.5,
        tags: None,
        scope: None,
        room_id: None,
        world_id: None,
        session_id: None,
    }
}

#[test]
fn the_route_messages_are_exact() {
    assert_eq!(
        MEMORY_PATCH_EMPTY,
        "provide content, importance, or tags to change"
    );
    assert_eq!(
        MEMORY_TASK_FAILED,
        "The memory change did not finish; check Memory and try again"
    );
}

#[tokio::test]
async fn the_memory_edit_routes_refuse_a_non_owner() {
    let state = daemon();
    let app = router(Arc::clone(&state), DaemonConfig::default());
    let id = seed(&app, "The heron nests by the quarry", None).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let before = stored(&state, &id).await.unwrap();

    for (method, body) in [
        ("PATCH", Some(json!({"content": "Rewritten by a stranger"}))),
        ("DELETE", None),
    ] {
        let refused = app
            .clone()
            .oneshot(request(method, &memory_uri(&id), FOREIGN_ORIGIN, body))
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::FORBIDDEN, "{method}");
        assert_eq!(refused.headers()["cache-control"], "no-store");
        assert_eq!(
            json_body(refused).await["error"],
            "local owner authorization required"
        );
    }
    assert_eq!(stored(&state, &id).await, Some(before));
    assert_eq!(vector_count(&state).await, 1);
}

#[tokio::test]
async fn patch_changes_content_importance_and_tags_and_answers_the_memory() {
    let state = daemon();
    let app = router(Arc::clone(&state), DaemonConfig::default());
    let seeded = seed(
        &app,
        "The heron nests by the quarry",
        Some(json!(["birds"])),
    )
    .await;
    let id = seeded["id"].as_str().unwrap();

    let response = send(
        &app,
        "PATCH",
        &memory_uri(id),
        Some(json!({
            "content": "  The kingfisher dives at dawn  ",
            "importance": 0.9,
            "tags": ["rivers", " dawn ", "rivers", ""],
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let body = json_body(response).await;
    assert_eq!(body["id"], seeded["id"]);
    assert_eq!(body["createdAt"], seeded["createdAt"]);
    assert_eq!(body["agentId"], seeded["agentId"]);
    assert_eq!(body["type"], "fact");
    assert_eq!(body["content"], "The kingfisher dives at dawn");
    assert_eq!(body["importance"], 0.9);
    assert_eq!(body["tags"], json!(["rivers", "dawn"]));

    let found = send(&app, "GET", "/api/memories/search?q=kingfisher", None).await;
    let found = json_body(found).await;
    assert_eq!(found["results"][0]["id"], seeded["id"]);
    let gone = send(&app, "GET", "/api/memories/search?q=heron", None).await;
    assert_eq!(json_body(gone).await["results"], json!([]));
}

#[tokio::test]
async fn patch_validates_each_field() {
    let state = daemon();
    let app = router(Arc::clone(&state), DaemonConfig::default());
    let id = seed(
        &app,
        "The heron nests by the quarry",
        Some(json!(["birds"])),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let before = stored(&state, &id).await.unwrap();

    let too_many_tags: Vec<String> = (0..21).map(|n| format!("tag{n}")).collect();
    let cases = [
        (
            json!({"importance": 1.5}),
            MemoryError::InvalidImportance.message(),
        ),
        (
            json!({"content": "Valid text", "importance": -0.1}),
            MemoryError::InvalidImportance.message(),
        ),
        (json!({"content": ""}), MEMORY_CONTENT_INVALID),
        (json!({"content": "   "}), MEMORY_CONTENT_INVALID),
        (
            json!({"content": "x".repeat(8_001)}),
            MEMORY_CONTENT_INVALID,
        ),
        (json!({"tags": too_many_tags}), MEMORY_TAGS_INVALID),
        (json!({"tags": ["a".repeat(41)]}), MEMORY_TAGS_INVALID),
        (
            json!({"content": "Remember \u{E0041}this"}),
            MEMORY_TEXT_HIDDEN,
        ),
        (
            json!({"content": "Fine text", "tags": ["\u{202E}flip"]}),
            MEMORY_TAGS_INVALID,
        ),
    ];
    for (body, message) in cases {
        let response = send(&app, "PATCH", &memory_uri(&id), Some(body.clone())).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(json_body(response).await["error"], message, "{body}");
        assert_eq!(stored(&state, &id).await.as_ref(), Some(&before), "{body}");
    }

    // Exactly 8,000 characters (not bytes) is accepted.
    let longest = "é".repeat(8_000);
    let response = send(
        &app,
        "PATCH",
        &memory_uri(&id),
        Some(json!({"content": longest})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(stored(&state, &id).await.unwrap().content, longest);
}

#[tokio::test]
async fn patch_with_no_fields_is_a_400() {
    let state = daemon();
    let app = router(Arc::clone(&state), DaemonConfig::default());
    let id = seed(&app, "The heron nests by the quarry", None).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let empty = send(&app, "PATCH", &memory_uri(&id), Some(json!({}))).await;
    assert_eq!(empty.status(), StatusCode::BAD_REQUEST);
    assert_eq!(empty.headers()["cache-control"], "no-store");
    assert_eq!(json_body(empty).await["error"], MEMORY_PATCH_EMPTY);

    let unknown = send(
        &app,
        "PATCH",
        &memory_uri(&id),
        Some(json!({"content": "New text", "color": "red"})),
    )
    .await;
    assert_eq!(unknown.status(), StatusCode::BAD_REQUEST);
    assert_eq!(unknown.headers()["cache-control"], "no-store");
    assert_eq!(
        stored(&state, &id).await.unwrap().content,
        "The heron nests by the quarry"
    );
}

#[tokio::test]
async fn patch_tags_null_clears_and_absent_keeps() {
    let state = daemon();
    let app = router(Arc::clone(&state), DaemonConfig::default());
    let id = seed(
        &app,
        "The heron nests by the quarry",
        Some(json!(["birds"])),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let kept = send(
        &app,
        "PATCH",
        &memory_uri(&id),
        Some(json!({"importance": 0.7})),
    )
    .await;
    assert_eq!(json_body(kept).await["tags"], json!(["birds"]));

    let cleared = send(&app, "PATCH", &memory_uri(&id), Some(json!({"tags": null}))).await;
    assert_eq!(cleared.status(), StatusCode::OK);
    assert_eq!(json_body(cleared).await["tags"], Value::Null);
    assert_eq!(stored(&state, &id).await.unwrap().tags, None);

    let replaced = send(
        &app,
        "PATCH",
        &memory_uri(&id),
        Some(json!({"tags": ["rivers"]})),
    )
    .await;
    assert_eq!(json_body(replaced).await["tags"], json!(["rivers"]));
    // An array that cleans to nothing clears too.
    let blank = send(
        &app,
        "PATCH",
        &memory_uri(&id),
        Some(json!({"tags": ["  "]})),
    )
    .await;
    assert_eq!(json_body(blank).await["tags"], Value::Null);
}

#[tokio::test]
async fn patch_unknown_memory_is_404() {
    let app = router(daemon(), DaemonConfig::default());
    let response = send(
        &app,
        "PATCH",
        "/api/memories/no-such-memory",
        Some(json!({"content": "New text"})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(response.headers()["cache-control"], "no-store");
}

#[tokio::test]
async fn patch_re_embeds_changed_content_only() {
    let state = daemon();
    let app = router(Arc::clone(&state), DaemonConfig::default());
    let id = seed(&app, "The heron nests by the quarry", None).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    seed(&app, "Weekly budget review happens on Fridays", None).await;
    assert_eq!(vector_count(&state).await, 2);

    let response = send(
        &app,
        "PATCH",
        &memory_uri(&id),
        Some(json!({"content": "Sourdough starter needs feeding twice daily"})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(vector_count(&state).await, 2);
    assert_eq!(
        vector_hits(&state, "sourdough starter feeding")
            .await
            .first(),
        Some(&id)
    );

    // Drop the vector by hand: an importance or tag change must not bring it
    // back (the embedding is of the content only), a content change must.
    {
        let embeddings = state.read().await.memory_embeddings_handle();
        embeddings
            .write()
            .await
            .remove_memories(std::slice::from_ref(&id))
            .unwrap();
    }
    for body in [json!({"importance": 0.8}), json!({"tags": ["baking"]})] {
        let response = send(&app, "PATCH", &memory_uri(&id), Some(body)).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(vector_count(&state).await, 1);
    }
    let response = send(
        &app,
        "PATCH",
        &memory_uri(&id),
        Some(json!({"content": "Sourdough starter needs feeding at noon"})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(vector_count(&state).await, 2);
    assert_eq!(
        vector_hits(&state, "sourdough starter feeding")
            .await
            .first(),
        Some(&id)
    );
}

/// The embedding runtime offers no failure injection, so the sync step is
/// checked at unit level with a failing upsert.
#[test]
fn a_failed_embedding_removes_the_stale_vector() {
    let mut manager = MemoryManager::new();
    let memory = manager
        .add(new_memory("The heron nests by the quarry"))
        .unwrap();
    let mut runtime = MemoryEmbeddingRuntime::local_default();
    runtime.upsert_memory(&memory).unwrap();
    assert_eq!(runtime.status().vector_count, 1);

    sync_embedding_with(&mut runtime, &memory, false, |_, _| {
        panic!("an unchanged content must not re-embed")
    });
    assert_eq!(runtime.status().vector_count, 1);

    sync_embedding_with(&mut runtime, &memory, true, |_, _| {
        Err("embedding provider unavailable".to_string())
    });
    assert_eq!(runtime.status().vector_count, 0);
    assert!(runtime.search("heron quarry", 5).is_empty());
}

#[tokio::test]
async fn delete_removes_the_memory_its_embedding_and_citations() {
    let state = daemon();
    let app = router(Arc::clone(&state), DaemonConfig::default());
    let deleted = seed(&app, "The heron nests by the quarry", None).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let kept = seed(&app, "Weekly budget review happens on Fridays", None).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let relationship = send(
        &app,
        "POST",
        "/api/memories/relationships",
        Some(json!({
            "sourceAgentId": "companion",
            "sourceAgentName": "Companion",
            "targetKind": "user",
            "targetAgentId": "user-1",
            "targetAgentName": "Leo",
            "relationshipType": "knows",
            "evidenceMemoryIds": [deleted, kept],
        })),
    )
    .await;
    assert_eq!(relationship.status(), StatusCode::CREATED);

    let response = send(&app, "DELETE", &memory_uri(&deleted), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(
        json_body(response).await,
        json!({
            "id": deleted,
            "removedRelationships": 0,
            "updatedRelationships": 1,
            "updatedFacts": 0,
        })
    );

    let recent = json_body(send(&app, "GET", "/api/memories/recent", None).await).await;
    let recent_ids: Vec<&str> = recent["memories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|memory| memory["id"].as_str().unwrap())
        .collect();
    assert_eq!(recent_ids, vec![kept.as_str()]);
    assert!(search_ids(&state, "heron quarry").await.is_empty());
    assert_eq!(vector_count(&state).await, 1);
    assert!(!vector_hits(&state, "heron nests quarry")
        .await
        .contains(&deleted));

    let relationships =
        json_body(send(&app, "GET", "/api/memories/relationships", None).await).await;
    assert_eq!(
        relationships["relationships"][0]["evidenceMemoryIds"],
        json!([kept])
    );
}

#[tokio::test]
async fn delete_unknown_memory_is_404() {
    let app = router(daemon(), DaemonConfig::default());
    let response = send(&app, "DELETE", "/api/memories/no-such-memory", None).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(response.headers()["cache-control"], "no-store");
}

#[tokio::test]
async fn a_failed_save_restores_the_memory_and_answers_503() {
    let state = daemon();
    let id = {
        let guard = state.read().await;
        let memory = guard
            .memory_handle()
            .write()
            .await
            .add(new_memory("The heron nests by the quarry"))
            .unwrap();
        guard
            .memory_embeddings_handle()
            .write()
            .await
            .upsert_memory(&memory)
            .unwrap();
        memory.id
    };
    // A directory where the store file should be: every save fails.
    let unwritable =
        std::env::temp_dir().join(format!("anima-memory-edits-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&unwritable).unwrap();
    state
        .write()
        .await
        .set_memory_store(Some(MemoryStoreConfig::Json(unwritable.clone())));
    let before = stored(&state, &id).await.unwrap();
    let app = router(Arc::clone(&state), DaemonConfig::default());

    let patched = send(
        &app,
        "PATCH",
        &memory_uri(&id),
        Some(json!({"content": "The kingfisher dives at dawn"})),
    )
    .await;
    assert_eq!(patched.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(patched.headers()["cache-control"], "no-store");
    let error = json_body(patched).await["error"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        error.starts_with("failed to persist memory: "),
        "unexpected error: {error}"
    );
    assert_eq!(stored(&state, &id).await, Some(before.clone()));
    assert_eq!(search_ids(&state, "heron").await, vec![id.clone()]);
    assert!(search_ids(&state, "kingfisher").await.is_empty());
    assert_eq!(
        vector_hits(&state, "heron nests quarry").await.first(),
        Some(&id)
    );

    let deleted = send(&app, "DELETE", &memory_uri(&id), None).await;
    assert_eq!(deleted.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(deleted.headers()["cache-control"], "no-store");
    assert_eq!(stored(&state, &id).await, Some(before));
    assert_eq!(vector_count(&state).await, 1);

    std::fs::remove_dir_all(&unwritable).unwrap();
}

#[tokio::test]
async fn a_dropped_patch_still_finishes() {
    let state = daemon();
    let app = router(Arc::clone(&state), DaemonConfig::default());
    let id = seed(&app, "The heron nests by the quarry", None).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let memory = state.read().await.memory_handle();

    let reader = memory.read().await;
    {
        let mut patch = Box::pin(app.clone().oneshot(request(
            "PATCH",
            &memory_uri(&id),
            OWNER_ORIGIN,
            Some(json!({"content": "The kingfisher dives at dawn"})),
        )));
        assert!(
            futures::poll!(patch.as_mut()).is_pending(),
            "the PATCH must wait for the memory lock"
        );
        // The request is dropped here, before its change could run.
    }
    // Wait (by yielding, not sleeping) until the spawned change queues for
    // the write lock: a queued writer makes further reads wait.
    let mut queued = false;
    for _ in 0..1_000 {
        if memory.try_read().is_err() {
            queued = true;
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(
        queued,
        "the spawned memory change never queued for the lock"
    );
    drop(reader);

    // tokio's lock is FIFO: this write waits behind the spawned change.
    let guard = memory.write().await;
    assert_eq!(
        guard.get(&id).unwrap().content,
        "The kingfisher dives at dawn"
    );
}

// ---- Facts and entities (M7 Task 3) ----

async fn add_fact(
    state: &Arc<RwLock<DaemonState>>,
    subject_id: &str,
    predicate: &str,
    value: &str,
    observed_at: u64,
    tweak: impl FnOnce(&mut NewTemporalFact),
) -> TemporalFact {
    let mut fact = NewTemporalFact {
        subject_kind: RelationshipEndpointKind::User,
        subject_id: subject_id.into(),
        subject_name: format!("Name of {subject_id}"),
        predicate: predicate.into(),
        object_kind: None,
        object_id: None,
        object_name: None,
        value: Some(value.into()),
        valid_from: None,
        valid_to: None,
        observed_at: Some(observed_at),
        confidence: 0.6,
        evidence_memory_ids: Vec::new(),
        supersedes_fact_ids: Vec::new(),
        status: None,
        tags: None,
        room_id: None,
        world_id: None,
        session_id: None,
    };
    tweak(&mut fact);
    let memory = state.read().await.memory_handle();
    let added = memory.write().await.add_temporal_fact(fact).unwrap();
    added
}

async fn all_facts(state: &Arc<RwLock<DaemonState>>) -> Vec<TemporalFact> {
    let memory = state.read().await.memory_handle();
    let facts = memory
        .read()
        .await
        .list_temporal_facts(TemporalFactOptions {
            include_inactive: true,
            limit: Some(usize::MAX),
            ..TemporalFactOptions::default()
        });
    facts
}

fn fact_ids(body: &Value) -> Vec<String> {
    body["facts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|fact| fact["id"].as_str().unwrap().to_string())
        .collect()
}

fn fact_uri(id: &str) -> String {
    format!("/api/memories/facts/{id}")
}

#[test]
fn the_fact_and_entity_messages_are_exact() {
    assert_eq!(
        FACT_NOT_EDITABLE,
        "Only an active fact with a value can be edited"
    );
    assert_eq!(
        ENTITY_KIND_REQUIRED,
        "kind query parameter must be one of agent, user, system, external"
    );
    assert_eq!(
        ENTITY_OWNS_MEMORIES,
        "This entity still has memories; delete them first"
    );
    assert_eq!(FACTS_LIMIT_INVALID, "limit must be from 1 to 500");
    assert_eq!(
        INCLUDE_INACTIVE_INVALID,
        "includeInactive must be true or false"
    );
}

#[tokio::test]
async fn the_fact_and_entity_routes_refuse_a_non_owner() {
    let state = daemon();
    let app = router(Arc::clone(&state), DaemonConfig::default());
    let fact = add_fact(&state, "user-1", "likes", "tea", 1_000, |_| {}).await;
    let before = all_facts(&state).await;

    let routes = [
        ("GET", "/api/memories/facts".to_string(), None),
        (
            "PATCH",
            fact_uri(&fact.id),
            Some(json!({"value": "coffee"})),
        ),
        ("DELETE", fact_uri(&fact.id), None),
        (
            "DELETE",
            "/api/memories/entities/user-1?kind=user".to_string(),
            None,
        ),
    ];
    for (method, uri, body) in routes {
        let refused = app
            .clone()
            .oneshot(request(method, &uri, FOREIGN_ORIGIN, body))
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::FORBIDDEN, "{method} {uri}");
        assert_eq!(refused.headers()["cache-control"], "no-store");
        assert_eq!(
            json_body(refused).await["error"],
            "local owner authorization required"
        );
    }
    assert_eq!(all_facts(&state).await, before);
    let memory = state.read().await.memory_handle();
    assert!(memory
        .read()
        .await
        .get_entity(RelationshipEndpointKind::User, "user-1")
        .is_some());
}

#[tokio::test]
async fn facts_list_newest_first_and_hide_inactive_unless_asked() {
    let state = daemon();
    let app = router(Arc::clone(&state), DaemonConfig::default());
    let oldest = add_fact(&state, "user-1", "likes", "tea", 1_000, |_| {}).await;
    let middle = add_fact(&state, "user-1", "lives in", "Lisbon", 2_000, |_| {}).await;
    let newest = add_fact(&state, "user-1", "works as", "a nurse", 3_000, |_| {}).await;
    // The middle fact is replaced by a fourth, so it is superseded.
    let replacement = add_fact(&state, "user-1", "lives in", "Porto", 4_000, |fact| {
        fact.supersedes_fact_ids = vec![middle.id.clone()];
    })
    .await;

    let response = send(&app, "GET", "/api/memories/facts", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let body = json_body(response).await;
    assert_eq!(
        fact_ids(&body),
        vec![replacement.id.clone(), newest.id.clone(), oldest.id.clone()]
    );
    let first = &body["facts"][0];
    assert_eq!(first["subjectKind"], "user");
    assert_eq!(first["subjectId"], "user-1");
    assert_eq!(first["predicate"], "lives in");
    assert_eq!(first["value"], "Porto");
    assert_eq!(first["status"], "active");
    assert_eq!(first["objectKind"], Value::Null);
    assert_eq!(first["observedAt"], 4_000);
    assert_eq!(first["supersedesFactIds"], json!([middle.id]));

    let body = json_body(
        send(
            &app,
            "GET",
            "/api/memories/facts?includeInactive=true",
            None,
        )
        .await,
    )
    .await;
    assert_eq!(
        fact_ids(&body),
        vec![replacement.id, newest.id, middle.id.clone(), oldest.id]
    );
    assert_eq!(body["facts"][2]["status"], "superseded");
}

#[tokio::test]
async fn facts_filter_by_agent_through_their_evidence() {
    let state = daemon();
    let app = router(Arc::clone(&state), DaemonConfig::default());
    let (mine, theirs) = {
        let memory = state.read().await.memory_handle();
        let mut guard = memory.write().await;
        let mine = guard.add(new_memory("Leo drinks tea")).unwrap().id;
        let mut other = new_memory("Mia drinks coffee");
        other.agent_id = "other-agent".into();
        other.agent_name = "Other".into();
        let theirs = guard.add(other).unwrap().id;
        (mine, theirs)
    };
    let mine_fact = add_fact(&state, "leo", "drinks", "tea", 1_000, |fact| {
        fact.evidence_memory_ids = vec![mine.clone()];
    })
    .await;
    let theirs_fact = add_fact(&state, "mia", "drinks", "coffee", 2_000, |fact| {
        fact.evidence_memory_ids = vec![theirs.clone()];
    })
    .await;
    let mixed_fact = add_fact(&state, "both", "drinks", "water", 3_000, |fact| {
        fact.evidence_memory_ids = vec!["gone".into(), theirs.clone(), mine.clone()];
    })
    .await;
    let bare_fact = add_fact(&state, "bare", "drinks", "juice", 4_000, |_| {}).await;

    let body =
        json_body(send(&app, "GET", "/api/memories/facts?agentId=companion", None).await).await;
    assert_eq!(
        fact_ids(&body),
        vec![mixed_fact.id.clone(), mine_fact.id.clone()]
    );

    let body =
        json_body(send(&app, "GET", "/api/memories/facts?agentId=other-agent", None).await).await;
    assert_eq!(
        fact_ids(&body),
        vec![mixed_fact.id.clone(), theirs_fact.id.clone()]
    );

    let body = json_body(send(&app, "GET", "/api/memories/facts?agentId=nobody", None).await).await;
    assert!(fact_ids(&body).is_empty());

    let body = json_body(send(&app, "GET", "/api/memories/facts", None).await).await;
    assert_eq!(fact_ids(&body).len(), 4);
    assert!(fact_ids(&body).contains(&bare_fact.id));
}

#[tokio::test]
async fn facts_filter_by_subject_id_or_name() {
    let state = daemon();
    let app = router(Arc::clone(&state), DaemonConfig::default());
    let by_id = add_fact(&state, "user-1", "likes", "tea", 1_000, |fact| {
        fact.subject_name = "Leo".into();
    })
    .await;
    let other = add_fact(&state, "user-2", "likes", "jam", 2_000, |fact| {
        fact.subject_name = "Mia Costa".into();
    })
    .await;

    for subject in ["user-1", "leo", "LEO"] {
        let body = json_body(
            send(
                &app,
                "GET",
                &format!("/api/memories/facts?subject={subject}"),
                None,
            )
            .await,
        )
        .await;
        assert_eq!(fact_ids(&body), vec![by_id.id.clone()], "{subject}");
    }
    let body =
        json_body(send(&app, "GET", "/api/memories/facts?subject=mia%20costa", None).await).await;
    assert_eq!(fact_ids(&body), vec![other.id.clone()]);
    let body = json_body(send(&app, "GET", "/api/memories/facts?subject=Mia", None).await).await;
    assert!(fact_ids(&body).is_empty());
}

#[tokio::test]
async fn facts_limit_defaults_and_is_bounded() {
    let state = daemon();
    let app = router(Arc::clone(&state), DaemonConfig::default());
    for n in 0..150u64 {
        add_fact(&state, "user-1", &format!("p{n}"), "v", 1_000 + n, |_| {}).await;
    }
    let body = json_body(send(&app, "GET", "/api/memories/facts", None).await).await;
    assert_eq!(body["facts"].as_array().unwrap().len(), 100);
    let body = json_body(send(&app, "GET", "/api/memories/facts?limit=500", None).await).await;
    assert_eq!(body["facts"].as_array().unwrap().len(), 150);
    let body = json_body(send(&app, "GET", "/api/memories/facts?limit=3", None).await).await;
    assert_eq!(body["facts"].as_array().unwrap().len(), 3);

    for limit in ["0", "501", "x", "1.5", ""] {
        let response = send(
            &app,
            "GET",
            &format!("/api/memories/facts?limit={limit}"),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "limit={limit}");
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(json_body(response).await["error"], FACTS_LIMIT_INVALID);
    }
    let response = send(
        &app,
        "GET",
        "/api/memories/facts?includeInactive=maybe",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_body(response).await["error"], INCLUDE_INACTIVE_INVALID);
}

#[tokio::test]
async fn replacing_a_fact_supersedes_it_with_the_new_value() {
    let state = daemon();
    let app = router(Arc::clone(&state), DaemonConfig::default());
    let old = add_fact(&state, "user-1", "likes", "tea", 1_000, |fact| {
        fact.evidence_memory_ids = vec!["m-1".into()];
        fact.tags = Some(vec!["drinks".into()]);
        fact.room_id = Some("room-1".into());
        fact.world_id = Some("world-1".into());
        fact.session_id = Some("session-1".into());
    })
    .await;

    let response = send(
        &app,
        "PATCH",
        &fact_uri(&old.id),
        Some(json!({"value": "  coffee  "})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let body = json_body(response).await;
    let fact = &body["fact"];
    assert_ne!(fact["id"], json!(old.id));
    assert_eq!(fact["value"], "coffee");
    assert_eq!(fact["status"], "active");
    assert_eq!(fact["supersedesFactIds"], json!([old.id]));
    assert_eq!(fact["confidence"], 1.0);
    assert_eq!(fact["subjectKind"], "user");
    assert_eq!(fact["subjectId"], "user-1");
    assert_eq!(fact["predicate"], "likes");
    assert_eq!(fact["evidenceMemoryIds"], json!(["m-1"]));
    assert_eq!(fact["tags"], json!(["drinks"]));
    assert_eq!(fact["roomId"], "room-1");
    assert_eq!(fact["worldId"], "world-1");
    assert_eq!(fact["sessionId"], "session-1");
    assert_eq!(fact["validTo"], Value::Null);

    let superseded = &body["superseded"];
    assert_eq!(superseded["id"], json!(old.id));
    assert_eq!(superseded["status"], "superseded");
    assert!(superseded["validTo"].is_u64());
    assert_eq!(superseded["value"], "tea");

    let listed = json_body(send(&app, "GET", "/api/memories/facts", None).await).await;
    assert_eq!(listed["facts"].as_array().unwrap().len(), 1);
    assert_eq!(listed["facts"][0]["id"], fact["id"]);

    // The same value still replaces the fact: the owner confirmed it.
    let again = send(
        &app,
        "PATCH",
        &fact_uri(fact["id"].as_str().unwrap()),
        Some(json!({"value": "coffee"})),
    )
    .await;
    assert_eq!(again.status(), StatusCode::OK);
    assert_eq!(all_facts(&state).await.len(), 3);
}

#[tokio::test]
async fn replacing_refuses_inactive_and_valueless_facts() {
    let state = daemon();
    let app = router(Arc::clone(&state), DaemonConfig::default());
    let old = add_fact(&state, "user-1", "likes", "tea", 1_000, |_| {}).await;
    add_fact(&state, "user-1", "likes", "coffee", 2_000, |fact| {
        fact.supersedes_fact_ids = vec![old.id.clone()];
    })
    .await;
    let relation = add_fact(&state, "user-1", "knows", "ignored", 3_000, |fact| {
        fact.value = None;
        fact.object_kind = Some(RelationshipEndpointKind::External);
        fact.object_id = Some("mia".into());
        fact.object_name = Some("Mia".into());
    })
    .await;
    let before = all_facts(&state).await;

    for id in [&old.id, &relation.id] {
        let response = send(
            &app,
            "PATCH",
            &fact_uri(id),
            Some(json!({"value": "water"})),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT, "{id}");
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(json_body(response).await["error"], FACT_NOT_EDITABLE);
    }
    assert_eq!(all_facts(&state).await, before);
}

#[tokio::test]
async fn replacing_validates_the_value() {
    let state = daemon();
    let app = router(Arc::clone(&state), DaemonConfig::default());
    let fact = add_fact(&state, "user-1", "likes", "tea", 1_000, |_| {}).await;
    let before = all_facts(&state).await;

    let too_long = "x".repeat(501);
    for (value, message) in [
        ("", FACT_VALUE_INVALID),
        ("   ", FACT_VALUE_INVALID),
        (too_long.as_str(), FACT_VALUE_INVALID),
        ("tea\u{202E}", MEMORY_TEXT_HIDDEN),
    ] {
        let response = send(
            &app,
            "PATCH",
            &fact_uri(&fact.id),
            Some(json!({"value": value})),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{value:?}");
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(json_body(response).await["error"], message, "{value:?}");
    }
    let unknown_field = send(
        &app,
        "PATCH",
        &fact_uri(&fact.id),
        Some(json!({"value": "ok", "status": "active"})),
    )
    .await;
    assert_eq!(unknown_field.status(), StatusCode::BAD_REQUEST);
    assert_eq!(all_facts(&state).await, before);

    let exactly_500 = send(
        &app,
        "PATCH",
        &fact_uri(&fact.id),
        Some(json!({"value": "y".repeat(500)})),
    )
    .await;
    assert_eq!(exactly_500.status(), StatusCode::OK);

    let unknown = send(
        &app,
        "PATCH",
        &fact_uri("no-such-fact"),
        Some(json!({"value": "water"})),
    )
    .await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    assert_eq!(unknown.headers()["cache-control"], "no-store");
}

#[tokio::test]
async fn deleting_a_fact_removes_it_and_the_links_to_it() {
    let state = daemon();
    let app = router(Arc::clone(&state), DaemonConfig::default());
    let old = add_fact(&state, "user-1", "likes", "tea", 1_000, |_| {}).await;
    let newer = add_fact(&state, "user-1", "likes", "coffee", 2_000, |fact| {
        fact.supersedes_fact_ids = vec![old.id.clone()];
    })
    .await;
    assert_eq!(newer.supersedes_fact_ids, vec![old.id.clone()]);

    let response = send(&app, "DELETE", &fact_uri(&old.id), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(json_body(response).await, json!({"id": old.id}));

    let facts = all_facts(&state).await;
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].id, newer.id);
    assert!(facts[0].supersedes_fact_ids.is_empty());

    let unknown = send(&app, "DELETE", &fact_uri(&old.id), None).await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    assert_eq!(unknown.headers()["cache-control"], "no-store");
}

#[tokio::test]
async fn deleting_an_entity_removes_its_relationships_and_answers_counts() {
    let state = daemon();
    let app = router(Arc::clone(&state), DaemonConfig::default());
    let relationship = send(
        &app,
        "POST",
        "/api/memories/relationships",
        Some(json!({
            "sourceAgentId": "companion",
            "sourceAgentName": "Companion",
            "targetKind": "user",
            "targetAgentId": "user-1",
            "targetAgentName": "Leo",
            "relationshipType": "knows",
        })),
    )
    .await;
    assert_eq!(relationship.status(), StatusCode::CREATED);
    let other = send(
        &app,
        "POST",
        "/api/memories/entities",
        Some(json!({"kind": "external", "id": "mia", "name": "Mia"})),
    )
    .await;
    assert_eq!(other.status(), StatusCode::CREATED);
    add_fact(&state, "user-1", "likes", "tea", 1_000, |_| {}).await;
    add_fact(&state, "user-1", "lives in", "Lisbon", 2_000, |_| {}).await;
    add_fact(&state, "mia", "likes", "jam", 3_000, |fact| {
        fact.subject_kind = RelationshipEndpointKind::External;
    })
    .await;

    let response = send(
        &app,
        "DELETE",
        "/api/memories/entities/user-1?kind=user",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(
        json_body(response).await,
        json!({
            "kind": "user",
            "id": "user-1",
            "removedRelationships": 1,
            "removedFacts": 2,
        })
    );

    let entities = json_body(send(&app, "GET", "/api/memories/entities", None).await).await;
    let keys: Vec<String> = entities["entities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entity| {
            format!(
                "{}:{}",
                entity["kind"].as_str().unwrap(),
                entity["id"].as_str().unwrap()
            )
        })
        .collect();
    assert!(!keys.contains(&"user:user-1".to_string()), "{keys:?}");
    assert!(keys.contains(&"external:mia".to_string()), "{keys:?}");
    assert!(keys.contains(&"agent:companion".to_string()), "{keys:?}");
    let facts = all_facts(&state).await;
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].subject_id, "mia");
}

#[tokio::test]
async fn deleting_an_entity_needs_its_kind_and_refuses_one_that_owns_memories() {
    let state = daemon();
    let app = router(Arc::clone(&state), DaemonConfig::default());
    seed(&app, "The heron nests by the quarry", None).await;
    add_fact(&state, "companion", "is", "curious", 1_000, |_| {}).await;
    let namesake = send(
        &app,
        "POST",
        "/api/memories/entities",
        Some(json!({"kind": "external", "id": "companion", "name": "A namesake"})),
    )
    .await;
    assert_eq!(namesake.status(), StatusCode::CREATED);

    for uri in [
        "/api/memories/entities/companion",
        "/api/memories/entities/companion?kind=robot",
        "/api/memories/entities/companion?kind=",
    ] {
        let response = send(&app, "DELETE", uri, None).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{uri}");
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(json_body(response).await["error"], ENTITY_KIND_REQUIRED);
    }

    let owns = send(
        &app,
        "DELETE",
        "/api/memories/entities/companion?kind=agent",
        None,
    )
    .await;
    assert_eq!(owns.status(), StatusCode::CONFLICT);
    assert_eq!(owns.headers()["cache-control"], "no-store");
    assert_eq!(json_body(owns).await["error"], ENTITY_OWNS_MEMORIES);
    assert_eq!(all_facts(&state).await.len(), 1);

    let unknown = send(
        &app,
        "DELETE",
        "/api/memories/entities/nobody?kind=user",
        None,
    )
    .await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    assert_eq!(unknown.headers()["cache-control"], "no-store");

    // The same id under another kind is a different entity.
    let removed = send(
        &app,
        "DELETE",
        "/api/memories/entities/companion?kind=external",
        None,
    )
    .await;
    assert_eq!(removed.status(), StatusCode::OK);
    let memory = state.read().await.memory_handle();
    let guard = memory.read().await;
    assert!(guard
        .get_entity(RelationshipEndpointKind::Agent, "companion")
        .is_some());
    assert!(guard
        .get_entity(RelationshipEndpointKind::External, "companion")
        .is_none());
}

#[tokio::test]
async fn deleting_an_entity_accepts_ids_with_a_colon() {
    let state = daemon();
    let app = router(Arc::clone(&state), DaemonConfig::default());
    let created = send(
        &app,
        "POST",
        "/api/memories/entities",
        Some(json!({"kind": "external", "id": "telegram:42", "name": "Telegram 42"})),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let response = send(
        &app,
        "DELETE",
        "/api/memories/entities/telegram:42?kind=external",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await["id"], "telegram:42");
}

#[test]
fn the_new_memory_routes_are_in_the_openapi_document() {
    use utoipa::OpenApi;

    let document = crate::routes::ApiDoc::openapi();
    let paths = &document.paths.paths;
    let item = &paths["/api/memories/{memory_id}"];
    assert!(item.patch.is_some() && item.delete.is_some());
    assert!(paths["/api/memories/facts"].get.is_some());
    let item = &paths["/api/memories/facts/{fact_id}"];
    assert!(item.patch.is_some() && item.delete.is_some());
    assert!(paths["/api/memories/entities/{entity_id}"].delete.is_some());
}

#[tokio::test]
async fn a_failed_save_restores_a_fact_replacement() {
    let state = daemon();
    let fact = add_fact(&state, "user-1", "likes", "tea", 1_000, |_| {}).await;
    let unwritable =
        std::env::temp_dir().join(format!("anima-memory-edits-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&unwritable).unwrap();
    state
        .write()
        .await
        .set_memory_store(Some(MemoryStoreConfig::Json(unwritable.clone())));
    let before = all_facts(&state).await;
    let app = router(Arc::clone(&state), DaemonConfig::default());

    let response = send(
        &app,
        "PATCH",
        &fact_uri(&fact.id),
        Some(json!({"value": "coffee"})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(all_facts(&state).await, before);
    assert_eq!(before[0].status, anima_memory::TemporalRecordStatus::Active);

    let deleted = send(&app, "DELETE", &fact_uri(&fact.id), None).await;
    assert_eq!(deleted.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(all_facts(&state).await, before);

    let entity = send(
        &app,
        "DELETE",
        "/api/memories/entities/user-1?kind=user",
        None,
    )
    .await;
    assert_eq!(entity.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(all_facts(&state).await, before);

    std::fs::remove_dir_all(&unwritable).unwrap();
}
