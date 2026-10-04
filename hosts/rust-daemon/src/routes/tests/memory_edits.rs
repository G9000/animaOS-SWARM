use super::*;

use anima_memory::{
    Memory, MemoryError, MemoryManager, MemorySearchOptions, MemoryType, MemoryVectorIndex,
    NewMemory,
};
use serde_json::{json, Value};

use crate::memory_embeddings::MemoryEmbeddingRuntime;
use crate::memory_store::MemoryStoreConfig;
use crate::memory_text::{MEMORY_CONTENT_INVALID, MEMORY_TAGS_INVALID, MEMORY_TEXT_HIDDEN};
use crate::routes::memory_edits::{sync_embedding_with, MEMORY_PATCH_EMPTY, MEMORY_TASK_FAILED};

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
