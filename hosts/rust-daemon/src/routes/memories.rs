use anima_memory::{MemoryScope, MemorySearchOptions, RecentMemoryOptions};

use super::contracts::{
    AgentRelationshipCreateRequest, AgentRelationshipQuery, AgentRelationshipResponse,
    AgentRelationshipsEnvelope, MemoriesEnvelope, MemoryCreateRequest, MemoryEntitiesEnvelope,
    MemoryEntityCreateRequest, MemoryEntityQuery, MemoryEntityResponse,
    MemoryEvaluationOutcomeResponse, MemoryEvaluationRequest, MemoryEvaluationResponse,
    MemoryRecallEnvelope, MemoryRecallQuery, MemoryRecallResultResponse, MemoryResponse,
    MemorySearchEnvelope, MemorySearchQuery, MemorySearchResultResponse, RecentMemoriesQuery,
};
use super::ApiError;
use crate::app::SharedDaemonState;

pub(crate) async fn handle_create_memory(
    body: Vec<u8>,
    state: &SharedDaemonState,
) -> Result<MemoryResponse, ApiError> {
    let request: MemoryCreateRequest = super::parse_json_body(body)?;
    let new_memory = request
        .into_domain()
        .map_err(ApiError::bad_request_static)?;

    let memory = {
        let memory = { state.read().await.memory_handle() };
        let mut memory_guard = memory.write().await;
        let memory = memory_guard
            .add(new_memory)
            .map_err(|error| ApiError::bad_request(error.message()))?;
        memory_guard.save().map_err(|error| {
            ApiError::service_unavailable(format!("failed to persist memory: {error}"))
        })?;
        memory
    };

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
        let memory = { state.read().await.memory_handle() };
        let mut memory_guard = memory.write().await;
        let entity = memory_guard
            .upsert_entity(new_entity)
            .map_err(|error| ApiError::bad_request(error.message()))?;
        memory_guard.save().map_err(|error| {
            ApiError::service_unavailable(format!("failed to persist memory entity: {error}"))
        })?;
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

    let outcome = {
        let memory = { state.read().await.memory_handle() };
        let mut memory_guard = memory.write().await;
        let outcome = memory_guard
            .add_evaluated(new_memory, options)
            .map_err(|error| ApiError::bad_request(error.message()))?;
        if outcome.memory.is_some() {
            memory_guard.save().map_err(|error| {
                ApiError::service_unavailable(format!(
                    "failed to persist evaluated memory: {error}"
                ))
            })?;
        }
        outcome
    };

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
        let memory = { state.read().await.memory_handle() };
        let memory_guard = memory.read().await;
        memory_guard.recall(&search_query, options)
    };

    Ok(MemoryRecallEnvelope {
        results: results
            .iter()
            .map(MemoryRecallResultResponse::from)
            .collect(),
    })
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
        let memory = { state.read().await.memory_handle() };
        let mut memory_guard = memory.write().await;
        let relationship = memory_guard
            .upsert_agent_relationship(new_relationship)
            .map_err(|error| ApiError::bad_request(error.message()))?;
        memory_guard.save().map_err(|error| {
            ApiError::service_unavailable(format!("failed to persist agent relationship: {error}"))
        })?;
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
