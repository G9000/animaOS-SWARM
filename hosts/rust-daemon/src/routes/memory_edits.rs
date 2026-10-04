//! The owner's memory edits (spec §10): change and delete a memory. Every
//! route requires the local owner and answers `Cache-Control: no-store`.
//!
//! Each mutation runs its whole body (memory write lock, change, save,
//! embedding sync) in its own task through `locked`, so a dropped request
//! never leaves a half-applied change. Lock order: memory write lock, then
//! the embeddings write lock (a leaf). The daemon state lock is held only to
//! clone the handles.

use std::future::Future;

use anima_memory::{Memory, MemoryError, MemoryPatch};
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::Serialize;
use tracing::warn;

use super::contracts::{ErrorBody, MemoryDeleteResponse, MemoryPatchRequest, MemoryResponse};
use super::http::{json_response, read_limited_body};
use super::jobs::{authorize, no_store};
use super::memories::{persist_memory_store, remove_memory_embeddings};
use super::sessions::rejected;
use super::{parse_json_body, ApiError, AppState};
use crate::app::SharedDaemonState;
use crate::memory_embeddings::{MemoryEmbeddingRuntime, SharedMemoryEmbeddings};
use crate::memory_store::{MemoryMutation, MemoryStoreConfig};
use crate::memory_text::{
    clean_tags, has_hidden_text, MAX_MEMORY_EDIT_CHARS, MEMORY_CONTENT_INVALID, MEMORY_TEXT_HIDDEN,
};
use crate::state::SharedMemoryStore;

pub(super) const MEMORY_PATCH_EMPTY: &str = "provide content, importance, or tags to change";
pub(super) const MEMORY_TASK_FAILED: &str =
    "The memory change did not finish; check Memory and try again";

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
