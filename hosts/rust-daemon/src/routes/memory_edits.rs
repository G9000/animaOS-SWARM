//! The owner's memory edits (spec §10): change and delete a memory, list,
//! replace, and forget facts, and delete an entity. Every route requires the
//! local owner and answers `Cache-Control: no-store`.
//!
//! Each mutation runs its whole body (memory write lock, change, save,
//! embedding sync) in its own task through `locked`, so a dropped request
//! never leaves a half-applied change. Lock order: memory write lock, then
//! the embeddings write lock (a leaf). The daemon state lock is held only to
//! clone the handles.

use std::future::Future;

use anima_memory::{
    Memory, MemoryError, MemoryPatch, NewTemporalFact, RelationshipEndpointKind, TemporalFact,
    TemporalFactOptions, TemporalRecordStatus,
};
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::Serialize;
use tracing::warn;

use super::contracts::{
    EntityDeleteResponse, ErrorBody, FactDeleteResponse, FactPatchRequest, FactReplacedResponse,
    MemoryDeleteResponse, MemoryFactResponse, MemoryFactsEnvelope, MemoryPatchRequest,
    MemoryResponse,
};
use super::http::{json_response, read_limited_body, request_query};
use super::jobs::{authorize, no_store};
use super::memories::{persist_memory_store, remove_memory_embeddings};
use super::sessions::rejected;
use super::{parse_json_body, ApiError, AppState};
use crate::app::SharedDaemonState;
use crate::memory_embeddings::{MemoryEmbeddingRuntime, SharedMemoryEmbeddings};
use crate::memory_store::{MemoryMutation, MemoryStoreConfig};
use crate::memory_text::{
    clean_tags, has_hidden_text, FACT_VALUE_INVALID, MAX_FACT_VALUE_CHARS, MAX_MEMORY_EDIT_CHARS,
    MEMORY_CONTENT_INVALID, MEMORY_TEXT_HIDDEN,
};
use crate::state::SharedMemoryStore;

pub(super) const MEMORY_PATCH_EMPTY: &str = "provide content, importance, or tags to change";
pub(super) const MEMORY_TASK_FAILED: &str =
    "The memory change did not finish; check Memory and try again";
pub(super) const FACT_NOT_EDITABLE: &str = "Only an active fact with a value can be edited";
pub(super) const ENTITY_KIND_REQUIRED: &str =
    "kind query parameter must be one of agent, user, system, external";
pub(super) const ENTITY_OWNS_MEMORIES: &str = "This entity still has memories; delete them first";
pub(super) const FACTS_LIMIT_INVALID: &str = "limit must be from 1 to 500";
pub(super) const INCLUDE_INACTIVE_INVALID: &str = "includeInactive must be true or false";

const DEFAULT_FACTS_LIMIT: usize = 100;
const MAX_FACTS_LIMIT: usize = 500;
/// The owner stated a replacement value, so it is fully trusted.
const FACT_REPLACEMENT_CONFIDENCE: f64 = 1.0;

#[utoipa::path(patch, path = "/api/memories/{memory_id}", tag = "memories",
    params(("memory_id" = String, Path)),
    request_body = MemoryPatchRequest,
    responses(
        (status = 200, description = "The changed memory, re-indexed (and re-embedded when its content changed)", body = MemoryResponse),
        (status = 400, description = "No field to change, or a field is invalid", body = ErrorBody),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "No such memory", body = ErrorBody),
        (status = 503, description = "The change could not be saved and was undone, or did not finish", body = ErrorBody)
    ))]
pub(super) async fn patch_memory(
    State(state): State<AppState>,
    Path(memory_id): Path<String>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, false) {
        return response;
    }
    let bytes = match read_limited_body(request, state.config.max_request_bytes).await {
        Ok(bytes) => bytes,
        Err(response) => return no_store(response),
    };
    let input: MemoryPatchRequest = match parse_json_body(bytes) {
        Ok(input) => input,
        Err(error) => return rejected(error),
    };
    let patch = match memory_patch(input) {
        Ok(patch) => patch,
        Err(error) => return rejected(error),
    };
    let daemon = state.daemon.clone();
    match locked(apply_patch(daemon, memory_id, patch)).await {
        Ok(memory) => answer(&MemoryResponse::from(&memory)),
        Err(error) => rejected(error),
    }
}

#[utoipa::path(delete, path = "/api/memories/{memory_id}", tag = "memories",
    params(("memory_id" = String, Path)),
    responses(
        (status = 200, description = "The memory, its embedding, and its citations are gone", body = MemoryDeleteResponse),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "No such memory", body = ErrorBody),
        (status = 503, description = "The deletion could not be saved and was undone, or did not finish", body = ErrorBody)
    ))]
pub(super) async fn delete_memory(
    State(state): State<AppState>,
    Path(memory_id): Path<String>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, false) {
        return response;
    }
    let daemon = state.daemon.clone();
    match locked(apply_delete(daemon, memory_id)).await {
        Ok(deleted) => answer(&deleted),
        Err(error) => rejected(error),
    }
}

#[utoipa::path(get, path = "/api/memories/facts", tag = "memories",
    params(
        ("agentId" = Option<String>, Query, description = "Keep facts whose evidence includes a memory of this agent (facts carry no agent of their own)"),
        ("subject" = Option<String>, Query, description = "Keep facts whose subject id equals this, or whose subject name equals it ignoring ASCII case"),
        ("includeInactive" = Option<bool>, Query, description = "Also list superseded and retracted facts (default false)"),
        ("limit" = Option<usize>, Query, description = "1 to 500 (default 100)")
    ),
    responses(
        (status = 200, description = "Facts, newest first", body = MemoryFactsEnvelope),
        (status = 400, description = "limit or includeInactive is invalid", body = ErrorBody),
        (status = 403, description = "Local owner required", body = ErrorBody)
    ))]
pub(super) async fn list_facts(State(state): State<AppState>, request: Request) -> Response {
    if let Err(response) = authorize(&state, &request, true) {
        return response;
    }
    let query = match request_query(request.uri()) {
        Ok(query) => query,
        Err(()) => return rejected(ApiError::bad_request_static("malformed query")),
    };
    let include_inactive = match query.get("includeInactive").map(String::as_str) {
        None | Some("false") => false,
        Some("true") => true,
        Some(_) => return rejected(ApiError::bad_request_static(INCLUDE_INACTIVE_INVALID)),
    };
    let limit = match query.get("limit") {
        None => DEFAULT_FACTS_LIMIT,
        Some(value) => match parse_facts_limit(value) {
            Some(limit) => limit,
            None => return rejected(ApiError::bad_request_static(FACTS_LIMIT_INVALID)),
        },
    };
    let agent_id = query.get("agentId");
    let subject = query.get("subject");

    let memory_handle = state.daemon.read().await.memory_handle();
    let manager = memory_handle.read().await;
    // The crate's own limit would cut before the filters below.
    let mut facts = manager.list_temporal_facts(TemporalFactOptions {
        include_inactive,
        limit: Some(usize::MAX),
        ..TemporalFactOptions::default()
    });
    facts.retain(|fact| {
        agent_id.is_none_or(|agent_id| {
            fact.evidence_memory_ids.iter().any(|id| {
                manager
                    .get(id)
                    .is_some_and(|memory| memory.agent_id == *agent_id)
            })
        }) && subject.is_none_or(|subject| {
            fact.subject_id == *subject || fact.subject_name.eq_ignore_ascii_case(subject)
        })
    });
    drop(manager);
    facts.truncate(limit);
    answer(&MemoryFactsEnvelope {
        facts: facts.iter().map(MemoryFactResponse::from).collect(),
    })
}

/// A whole number from 1 to `MAX_FACTS_LIMIT`.
fn parse_facts_limit(value: &str) -> Option<usize> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value
        .parse::<usize>()
        .ok()
        .filter(|limit| (1..=MAX_FACTS_LIMIT).contains(limit))
}

#[utoipa::path(patch, path = "/api/memories/facts/{fact_id}", tag = "memories",
    params(("fact_id" = String, Path)),
    request_body = FactPatchRequest,
    responses(
        (status = 200, description = "The new fact and the fact it superseded", body = FactReplacedResponse),
        (status = 400, description = "The value is invalid", body = ErrorBody),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "No such fact", body = ErrorBody),
        (status = 409, description = "Only an active fact with a value can be edited", body = ErrorBody),
        (status = 503, description = "The change could not be saved and was undone, or did not finish", body = ErrorBody)
    ))]
pub(super) async fn patch_fact(
    State(state): State<AppState>,
    Path(fact_id): Path<String>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, false) {
        return response;
    }
    let bytes = match read_limited_body(request, state.config.max_request_bytes).await {
        Ok(bytes) => bytes,
        Err(response) => return no_store(response),
    };
    let input: FactPatchRequest = match parse_json_body(bytes) {
        Ok(input) => input,
        Err(error) => return rejected(error),
    };
    let value = input.value.trim();
    let length = value.chars().count();
    if length == 0 || length > MAX_FACT_VALUE_CHARS {
        return rejected(ApiError::bad_request_static(FACT_VALUE_INVALID));
    }
    if has_hidden_text(value) {
        return rejected(ApiError::bad_request_static(MEMORY_TEXT_HIDDEN));
    }
    let value = value.to_string();
    let daemon = state.daemon.clone();
    match locked(apply_fact_replacement(daemon, fact_id, value)).await {
        Ok(replaced) => answer(&replaced),
        Err(error) => rejected(error),
    }
}

#[utoipa::path(delete, path = "/api/memories/facts/{fact_id}", tag = "memories",
    params(("fact_id" = String, Path)),
    responses(
        (status = 200, description = "The fact is forgotten, and no fact still names it as superseded", body = FactDeleteResponse),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "No such fact", body = ErrorBody),
        (status = 503, description = "The deletion could not be saved and was undone, or did not finish", body = ErrorBody)
    ))]
pub(super) async fn delete_fact(
    State(state): State<AppState>,
    Path(fact_id): Path<String>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, false) {
        return response;
    }
    let daemon = state.daemon.clone();
    match locked(apply_fact_delete(daemon, fact_id)).await {
        Ok(deleted) => answer(&deleted),
        Err(error) => rejected(error),
    }
}

#[utoipa::path(delete, path = "/api/memories/entities/{entity_id}", tag = "memories",
    params(
        ("entity_id" = String, Path),
        ("kind" = String, Query, description = "agent, user, system, or external: an entity's key is its kind plus its id")
    ),
    responses(
        (status = 200, description = "The entity, the relationships that named it, and the facts that named it are gone", body = EntityDeleteResponse),
        (status = 400, description = "kind is missing or not one of agent, user, system, external", body = ErrorBody),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "No such entity", body = ErrorBody),
        (status = 409, description = "An agent entity that still owns memories", body = ErrorBody),
        (status = 503, description = "The deletion could not be saved and was undone, or did not finish", body = ErrorBody)
    ))]
pub(super) async fn delete_entity(
    State(state): State<AppState>,
    Path(entity_id): Path<String>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, false) {
        return response;
    }
    let query = match request_query(request.uri()) {
        Ok(query) => query,
        Err(()) => return rejected(ApiError::bad_request_static("malformed query")),
    };
    let Some(kind) = query
        .get("kind")
        .and_then(|kind| RelationshipEndpointKind::from_str(kind).ok())
    else {
        return rejected(ApiError::bad_request_static(ENTITY_KIND_REQUIRED));
    };
    let daemon = state.daemon.clone();
    match locked(apply_entity_delete(daemon, kind, entity_id)).await {
        Ok(deleted) => answer(&deleted),
        Err(error) => rejected(error),
    }
}

/// Runs `work` in its own task and awaits it. A dropped caller does not cancel it; a panicked or cancelled task is a 503 MEMORY_TASK_FAILED.
async fn locked<T: Send + 'static>(
    work: impl Future<Output = Result<T, ApiError>> + Send + 'static,
) -> Result<T, ApiError> {
    match tokio::spawn(work).await {
        Ok(result) => result,
        Err(_) => Err(ApiError::service_unavailable(
            MEMORY_TASK_FAILED.to_string(),
        )),
    }
}

fn answer<T: Serialize>(body: &T) -> Response {
    no_store(json_response(StatusCode::OK, body))
}

/// Validates the whole request before anything changes.
fn memory_patch(input: MemoryPatchRequest) -> Result<MemoryPatch, ApiError> {
    if input.content.is_none() && input.importance.is_none() && input.tags.is_none() {
        return Err(ApiError::bad_request_static(MEMORY_PATCH_EMPTY));
    }
    let content = match input.content {
        Some(content) => {
            let content = content.trim();
            let length = content.chars().count();
            if length == 0 || length > MAX_MEMORY_EDIT_CHARS {
                return Err(ApiError::bad_request_static(MEMORY_CONTENT_INVALID));
            }
            if has_hidden_text(content) {
                return Err(ApiError::bad_request_static(MEMORY_TEXT_HIDDEN));
            }
            Some(content.to_string())
        }
        None => None,
    };
    if let Some(importance) = input.importance {
        if !importance.is_finite() || !(0.0..=1.0).contains(&importance) {
            return Err(ApiError::bad_request(
                MemoryError::InvalidImportance.message(),
            ));
        }
    }
    let tags = match input.tags {
        None => None,
        Some(None) => Some(None),
        Some(Some(tags)) => {
            let tags = clean_tags(tags).map_err(ApiError::bad_request_static)?;
            Some((!tags.is_empty()).then_some(tags))
        }
    };
    Ok(MemoryPatch {
        content,
        importance: input.importance,
        tags,
    })
}

async fn memory_handles(
    daemon: &SharedDaemonState,
) -> (
    SharedMemoryStore,
    SharedMemoryEmbeddings,
    Option<MemoryStoreConfig>,
) {
    let guard = daemon.read().await;
    (
        guard.memory_handle(),
        guard.memory_embeddings_handle(),
        guard.memory_store_config(),
    )
}

async fn apply_patch(
    daemon: SharedDaemonState,
    memory_id: String,
    patch: MemoryPatch,
) -> Result<Memory, ApiError> {
    let content_changed = patch.content.is_some();
    let (memory_handle, embeddings_handle, memory_store) = memory_handles(&daemon).await;
    let mut memory_guard = memory_handle.write().await;
    if memory_guard.get(&memory_id).is_none() {
        return Err(ApiError::not_found());
    }
    let memory = {
        let mut mutation = MemoryMutation::new(&mut memory_guard);
        let memory = mutation
            .update_memory(&memory_id, patch)
            .map_err(|error| ApiError::bad_request(error.message()))?
            .ok_or_else(ApiError::not_found)?;
        persist_memory_store(
            memory_store.as_ref(),
            &mut mutation,
            "failed to persist memory",
        )
        .await?;
        memory
    };
    // Still under the memory write lock, so two concurrent edits cannot leave
    // the older one's vector behind.
    sync_embedding(
        &mut *embeddings_handle.write().await,
        &memory,
        content_changed,
    );
    drop(memory_guard);
    Ok(memory)
}

async fn apply_delete(
    daemon: SharedDaemonState,
    memory_id: String,
) -> Result<MemoryDeleteResponse, ApiError> {
    let (memory_handle, embeddings_handle, memory_store) = memory_handles(&daemon).await;
    let mut memory_guard = memory_handle.write().await;
    if memory_guard.get(&memory_id).is_none() {
        return Err(ApiError::not_found());
    }
    let deletion = {
        let mut mutation = MemoryMutation::new(&mut memory_guard);
        let deletion = mutation
            .delete_memory(&memory_id)
            .ok_or_else(ApiError::not_found)?;
        persist_memory_store(
            memory_store.as_ref(),
            &mut mutation,
            "failed to persist memory deletion",
        )
        .await?;
        deletion
    };
    remove_memory_embeddings(&embeddings_handle, std::slice::from_ref(&memory_id)).await;
    drop(memory_guard);
    Ok(MemoryDeleteResponse {
        id: memory_id,
        removed_relationships: deletion.removed_relationship_ids.len(),
        updated_relationships: deletion.updated_relationship_ids.len()
            + deletion.updated_temporal_relationship_ids.len(),
        updated_facts: deletion.updated_fact_ids.len(),
    })
}

async fn apply_fact_replacement(
    daemon: SharedDaemonState,
    fact_id: String,
    value: String,
) -> Result<FactReplacedResponse, ApiError> {
    let (memory_handle, _, memory_store) = memory_handles(&daemon).await;
    let mut memory_guard = memory_handle.write().await;
    let old = memory_guard
        .get_temporal_fact(&fact_id)
        .ok_or_else(ApiError::not_found)?;
    if old.status != TemporalRecordStatus::Active || old.value.is_none() {
        return Err(ApiError::conflict(FACT_NOT_EDITABLE));
    }
    let replaced = {
        let mut mutation = MemoryMutation::new(&mut memory_guard);
        let fact = mutation
            .add_temporal_fact(replacement_fact(&old, value))
            .map_err(|error| ApiError::bad_request(error.message()))?;
        persist_memory_store(
            memory_store.as_ref(),
            &mut mutation,
            "failed to persist fact change",
        )
        .await?;
        let superseded = mutation
            .get_temporal_fact(&old.id)
            .ok_or_else(ApiError::not_found)?;
        FactReplacedResponse {
            fact: MemoryFactResponse::from(&fact),
            superseded: MemoryFactResponse::from(&superseded),
        }
    };
    drop(memory_guard);
    Ok(replaced)
}

/// The old fact with a new value: same subject, predicate, evidence, tags,
/// and scope; stated now, with full confidence; it supersedes the old fact.
fn replacement_fact(old: &TemporalFact, value: String) -> NewTemporalFact {
    NewTemporalFact {
        subject_kind: old.subject_kind,
        subject_id: old.subject_id.clone(),
        subject_name: old.subject_name.clone(),
        predicate: old.predicate.clone(),
        object_kind: None,
        object_id: None,
        object_name: None,
        value: Some(value),
        valid_from: None,
        valid_to: None,
        observed_at: None,
        confidence: FACT_REPLACEMENT_CONFIDENCE,
        evidence_memory_ids: old.evidence_memory_ids.clone(),
        supersedes_fact_ids: vec![old.id.clone()],
        status: None,
        tags: old.tags.clone(),
        room_id: old.room_id.clone(),
        world_id: old.world_id.clone(),
        session_id: old.session_id.clone(),
    }
}

async fn apply_fact_delete(
    daemon: SharedDaemonState,
    fact_id: String,
) -> Result<FactDeleteResponse, ApiError> {
    let (memory_handle, _, memory_store) = memory_handles(&daemon).await;
    let mut memory_guard = memory_handle.write().await;
    if memory_guard.get_temporal_fact(&fact_id).is_none() {
        return Err(ApiError::not_found());
    }
    {
        let mut mutation = MemoryMutation::new(&mut memory_guard);
        mutation.forget_temporal_fact(&fact_id);
        persist_memory_store(
            memory_store.as_ref(),
            &mut mutation,
            "failed to persist fact deletion",
        )
        .await?;
    }
    drop(memory_guard);
    Ok(FactDeleteResponse { id: fact_id })
}

async fn apply_entity_delete(
    daemon: SharedDaemonState,
    kind: RelationshipEndpointKind,
    entity_id: String,
) -> Result<EntityDeleteResponse, ApiError> {
    let (memory_handle, _, memory_store) = memory_handles(&daemon).await;
    let mut memory_guard = memory_handle.write().await;
    if memory_guard.get_entity(kind, &entity_id).is_none() {
        return Err(ApiError::not_found());
    }
    let deletion = {
        let mut mutation = MemoryMutation::new(&mut memory_guard);
        let deletion = match mutation.delete_entity(kind, &entity_id) {
            Ok(Some(deletion)) => deletion,
            Ok(None) => return Err(ApiError::not_found()),
            Err(MemoryError::EntityOwnsMemories) => {
                return Err(ApiError::conflict(ENTITY_OWNS_MEMORIES))
            }
            Err(error) => return Err(ApiError::bad_request(error.message())),
        };
        persist_memory_store(
            memory_store.as_ref(),
            &mut mutation,
            "failed to persist entity deletion",
        )
        .await?;
        deletion
    };
    drop(memory_guard);
    Ok(EntityDeleteResponse {
        kind: kind.as_str().to_string(),
        id: entity_id,
        removed_relationships: deletion.removed_relationship_ids.len()
            + deletion.removed_temporal_relationship_ids.len(),
        removed_facts: deletion.removed_fact_ids.len(),
    })
}

fn sync_embedding(runtime: &mut MemoryEmbeddingRuntime, memory: &Memory, content_changed: bool) {
    sync_embedding_with(
        runtime,
        memory,
        content_changed,
        MemoryEmbeddingRuntime::upsert_memory,
    );
}

/// Re-embeds a memory whose content changed; the embedding is of the content
/// only, so importance and tag changes leave it alone. A failed re-embed
/// removes the old vector (a stale vector must not outlive its text); the
/// edit is saved either way and recall falls back to the text index.
pub(super) fn sync_embedding_with(
    runtime: &mut MemoryEmbeddingRuntime,
    memory: &Memory,
    content_changed: bool,
    upsert: impl FnOnce(&mut MemoryEmbeddingRuntime, &Memory) -> Result<(), String>,
) {
    if !content_changed {
        return;
    }
    let Err(error) = upsert(runtime, memory) else {
        return;
    };
    warn!(
        memory_id = %memory.id,
        error = %error,
        "failed to re-embed an edited memory; removing its old vector"
    );
    if let Err(error) = runtime.remove_memories(std::slice::from_ref(&memory.id)) {
        warn!(
            memory_id = %memory.id,
            error = %error,
            "failed to remove an edited memory's old vector"
        );
    }
}
