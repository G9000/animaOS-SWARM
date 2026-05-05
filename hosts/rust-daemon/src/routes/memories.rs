use anima_memory::{
    baseline_memory_eval_cases, run_memory_eval_cases, Memory, MemoryManager, MemoryScope,
    MemorySearchOptions, RecentMemoryOptions,
};
use tracing::warn;

use super::contracts::{
    AgentRelationshipCreateRequest, AgentRelationshipQuery, AgentRelationshipResponse,
    AgentRelationshipsEnvelope, MemoriesEnvelope, MemoryCreateRequest, MemoryEntitiesEnvelope,
    MemoryEntityCreateRequest, MemoryEntityQuery, MemoryEntityResponse,
    MemoryEvaluationOutcomeResponse, MemoryEvaluationRequest, MemoryEvaluationResponse,
    MemoryEvidenceTraceResponse, MemoryReadinessResponse, MemoryRecallEnvelope, MemoryRecallQuery,
    MemoryRecallResultResponse, MemoryResponse, MemoryRetentionReportResponse,
    MemoryRetentionRequest, MemorySearchEnvelope, MemorySearchQuery, MemorySearchResultResponse,
    RecentMemoriesQuery,
};
use super::ApiError;
use crate::app::SharedDaemonState;
use crate::memory_embeddings::SharedMemoryEmbeddings;
use crate::memory_store::{save_memory_manager, MemoryStoreConfig};

pub(crate) async fn handle_create_memory(
    body: Vec<u8>,
    state: &SharedDaemonState,
) -> Result<MemoryResponse, ApiError> {
    let request: MemoryCreateRequest = super::parse_json_body(body)?;
    let new_memory = request
        .into_domain()
        .map_err(ApiError::bad_request_static)?;

    let (memory_handle, embeddings_handle, memory_store) = {
        let guard = state.read().await;
        (
            guard.memory_handle(),
            guard.memory_embeddings_handle(),
            guard.memory_store_config(),
        )
    };
    let memory = {
        let mut memory_guard = memory_handle.write().await;
        let memory = memory_guard
            .add(new_memory)
            .map_err(|error| ApiError::bad_request(error.message()))?;
        persist_memory_store(
            memory_store.as_ref(),
            &memory_guard,
            "failed to persist memory",
        )
        .await?;
        memory
    };
    index_memory_embedding(&embeddings_handle, &memory).await;

    Ok(MemoryResponse::from(&memory))
}

pub(crate) async fn handle_search_memories(
    query: MemorySearchQuery,
    state: &SharedDaemonState,
) -> Result<MemorySearchEnvelope, ApiError> {
    let search_query = query
        .q
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::bad_request_static("q query parameter is required"))?;

    let memory_type = match query.memory_type {
        None => None,
        Some(value) => Some(
            anima_memory::MemoryType::parse(&value)
                .map_err(|_| ApiError::bad_request_static("type must be a valid memory type"))?,
        ),
    };

    let scope = match query.scope {
        None => None,
        Some(value) => Some(MemoryScope::parse(&value).map_err(|_| {
            ApiError::bad_request_static("scope must be one of shared, private, room")
        })?),
    };

    let results = {
        let memory = { state.read().await.memory_handle() };
        let memory_guard = memory.read().await;
        memory_guard.search(
            &search_query,
            MemorySearchOptions {
                agent_id: query.agent_id,
                agent_name: query.agent_name,
                memory_type,
                scope,
                room_id: query.room_id,
                world_id: query.world_id,
                session_id: query.session_id,
                limit: query.limit,
                min_importance: query.min_importance,
            },
        )
    };

    Ok(MemorySearchEnvelope {
        results: results
            .iter()
            .map(MemorySearchResultResponse::from)
            .collect(),
    })
}

pub(crate) async fn handle_recent_memories(
    query: RecentMemoriesQuery,
    state: &SharedDaemonState,
) -> Result<MemoriesEnvelope, ApiError> {
    let memories = {
        let scope = match query.scope {
            None => None,
            Some(value) => Some(MemoryScope::parse(&value).map_err(|_| {
                ApiError::bad_request_static("scope must be one of shared, private, room")
            })?),
        };
        let memory = { state.read().await.memory_handle() };
        let memory_guard = memory.read().await;
        memory_guard.get_recent(RecentMemoryOptions {
            agent_id: query.agent_id,
            agent_name: query.agent_name,
            scope,
            room_id: query.room_id,
            world_id: query.world_id,
            session_id: query.session_id,
            limit: query.limit,
        })
    };

    Ok(MemoriesEnvelope {
        memories: memories.iter().map(MemoryResponse::from).collect(),
    })
}

pub(crate) async fn handle_create_memory_entity(
    body: Vec<u8>,
    state: &SharedDaemonState,
) -> Result<MemoryEntityResponse, ApiError> {
    let request: MemoryEntityCreateRequest = super::parse_json_body(body)?;
    let new_entity = request
        .into_domain()
        .map_err(ApiError::bad_request_static)?;

    let entity = {
        let (memory, memory_store) = {
            let guard = state.read().await;
            (guard.memory_handle(), guard.memory_store_config())
        };
        let mut memory_guard = memory.write().await;
        let entity = memory_guard
            .upsert_entity(new_entity)
            .map_err(|error| ApiError::bad_request(error.message()))?;
        persist_memory_store(
            memory_store.as_ref(),
            &memory_guard,
            "failed to persist memory entity",
        )
        .await?;
        entity
    };

    Ok(MemoryEntityResponse::from(&entity))
}

pub(crate) async fn handle_list_memory_entities(
    query: MemoryEntityQuery,
    state: &SharedDaemonState,
) -> Result<MemoryEntitiesEnvelope, ApiError> {
    let entities = {
        let memory = { state.read().await.memory_handle() };
        let memory_guard = memory.read().await;
        memory_guard.list_entities(query.into_domain())
    };

    Ok(MemoryEntitiesEnvelope {
        entities: entities.iter().map(MemoryEntityResponse::from).collect(),
    })
}

pub(crate) async fn handle_evaluate_memory(
    body: Vec<u8>,
    state: &SharedDaemonState,
) -> Result<MemoryEvaluationResponse, ApiError> {
    let request: MemoryEvaluationRequest = super::parse_json_body(body)?;
    let (new_memory, options) = request
        .into_domain()
        .map_err(ApiError::bad_request_static)?;

    let evaluation = {
        let memory = { state.read().await.memory_handle() };
        let memory_guard = memory.read().await;
        memory_guard
            .evaluate_new_memory(&new_memory, options)
            .map_err(|error| ApiError::bad_request(error.message()))?
    };

    Ok(MemoryEvaluationResponse::from(&evaluation))
}

pub(crate) async fn handle_add_evaluated_memory(
    body: Vec<u8>,
    state: &SharedDaemonState,
) -> Result<MemoryEvaluationOutcomeResponse, ApiError> {
    let request: MemoryEvaluationRequest = super::parse_json_body(body)?;
    let (new_memory, options) = request
        .into_domain()
        .map_err(ApiError::bad_request_static)?;

    let (memory_handle, embeddings_handle, memory_store) = {
        let guard = state.read().await;
        (
            guard.memory_handle(),
            guard.memory_embeddings_handle(),
            guard.memory_store_config(),
        )
    };
    let outcome = {
        let mut memory_guard = memory_handle.write().await;
        let outcome = memory_guard
            .add_evaluated(new_memory, options)
            .map_err(|error| ApiError::bad_request(error.message()))?;
        if outcome.memory.is_some() {
            persist_memory_store(
                memory_store.as_ref(),
                &memory_guard,
                "failed to persist evaluated memory",
            )
            .await?;
        }
        outcome
    };
    if let Some(memory) = &outcome.memory {
        index_memory_embedding(&embeddings_handle, memory).await;
    }

    Ok(MemoryEvaluationOutcomeResponse {
        evaluation: MemoryEvaluationResponse::from(&outcome.evaluation),
        memory: outcome.memory.as_ref().map(MemoryResponse::from),
    })
}

pub(crate) async fn handle_recall_memories(
    query: MemoryRecallQuery,
    state: &SharedDaemonState,
) -> Result<MemoryRecallEnvelope, ApiError> {
    let (search_query, options) = query.into_domain().map_err(ApiError::bad_request_static)?;
    let results = {
        let (memory, embeddings) = {
            let guard = state.read().await;
            (guard.memory_handle(), guard.memory_embeddings_handle())
        };
        let memory_guard = memory.read().await;
        let embeddings_guard = embeddings.read().await;
        memory_guard.recall_with_vector_index(&search_query, options, Some(&*embeddings_guard))
    };

    Ok(MemoryRecallEnvelope {
        results: results
            .iter()
            .map(MemoryRecallResultResponse::from)
            .collect(),
    })
}

pub(crate) async fn handle_memory_trace(
    memory_id: String,
    state: &SharedDaemonState,
) -> Result<MemoryEvidenceTraceResponse, ApiError> {
    let trace = {
        let memory = { state.read().await.memory_handle() };
        let memory_guard = memory.read().await;
        memory_guard.trace_memory(&memory_id)
    }
    .ok_or_else(ApiError::not_found)?;

    Ok(MemoryEvidenceTraceResponse::from(&trace))
}

pub(crate) async fn handle_memory_readiness(
    state: &SharedDaemonState,
) -> Result<MemoryReadinessResponse, ApiError> {
    let eval_report = run_memory_eval_cases(&baseline_memory_eval_cases());
    let embedding_status = {
        let embeddings = { state.read().await.memory_embeddings_handle() };
        let embeddings_guard = embeddings.read().await;
        embeddings_guard.status()
    };

    Ok(MemoryReadinessResponse {
        passed: eval_report.passed() && embedding_status.enabled,
        embeddings: (&embedding_status).into(),
        evaluation: (&eval_report).into(),
    })
}

pub(crate) async fn handle_apply_memory_retention(
    body: Vec<u8>,
    state: &SharedDaemonState,
) -> Result<MemoryRetentionReportResponse, ApiError> {
    let request: MemoryRetentionRequest = super::parse_json_body(body)?;
    let policy = request
        .into_domain()
        .map_err(ApiError::bad_request_static)?;

    let (memory_handle, embeddings_handle, memory_store) = {
        let guard = state.read().await;
        (
            guard.memory_handle(),
            guard.memory_embeddings_handle(),
            guard.memory_store_config(),
        )
    };
    let report = {
        let mut memory_guard = memory_handle.write().await;
        let report = memory_guard
            .apply_retention(policy)
            .map_err(|error| ApiError::bad_request(error.message()))?;
        if !report.decayed_memories.is_empty()
            || !report.removed_memory_ids.is_empty()
            || !report.removed_relationship_ids.is_empty()
        {
            persist_memory_store(
                memory_store.as_ref(),
                &memory_guard,
                "failed to persist memory retention changes",
            )
            .await?;
        }
        report
    };
    remove_memory_embeddings(&embeddings_handle, &report.removed_memory_ids).await;

    Ok(MemoryRetentionReportResponse::from(&report))
}

async fn index_memory_embedding(embeddings: &SharedMemoryEmbeddings, memory: &Memory) {
    if let Err(error) = embeddings.write().await.upsert_memory(memory) {
        warn!(
            memory_id = %memory.id,
            error = %error,
            "failed to index memory embedding"
        );
    }
}

async fn remove_memory_embeddings(embeddings: &SharedMemoryEmbeddings, memory_ids: &[String]) {
    if memory_ids.is_empty() {
        return;
    }
    if let Err(error) = embeddings.write().await.remove_memories(memory_ids) {
        warn!(
            removed_count = memory_ids.len(),
            error = %error,
            "failed to remove memory embeddings"
        );
    }
}

async fn persist_memory_store(
    memory_store: Option<&MemoryStoreConfig>,
    manager: &MemoryManager,
    message: &'static str,
) -> Result<(), ApiError> {
    save_memory_manager(memory_store, manager)
        .await
        .map_err(|error| ApiError::service_unavailable(format!("{message}: {error}")))
}

pub(crate) async fn handle_create_agent_relationship(
    body: Vec<u8>,
    state: &SharedDaemonState,
) -> Result<AgentRelationshipResponse, ApiError> {
    let request: AgentRelationshipCreateRequest = super::parse_json_body(body)?;
    let new_relationship = request
        .into_domain()
        .map_err(ApiError::bad_request_static)?;

    let relationship = {
        let (memory, memory_store) = {
            let guard = state.read().await;
            (guard.memory_handle(), guard.memory_store_config())
        };
        let mut memory_guard = memory.write().await;
        let relationship = memory_guard
            .upsert_agent_relationship(new_relationship)
            .map_err(|error| ApiError::bad_request(error.message()))?;
        persist_memory_store(
            memory_store.as_ref(),
            &memory_guard,
            "failed to persist agent relationship",
        )
        .await?;
        relationship
    };

    Ok(AgentRelationshipResponse::from(&relationship))
}

pub(crate) async fn handle_list_agent_relationships(
    query: AgentRelationshipQuery,
    state: &SharedDaemonState,
) -> Result<AgentRelationshipsEnvelope, ApiError> {
    let relationships = {
        let memory = { state.read().await.memory_handle() };
        let memory_guard = memory.read().await;
        memory_guard.list_agent_relationships(query.into_domain())
    };

    Ok(AgentRelationshipsEnvelope {
        relationships: relationships
            .iter()
            .map(AgentRelationshipResponse::from)
            .collect(),
    })
}
