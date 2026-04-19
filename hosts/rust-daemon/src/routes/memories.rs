use std::sync::{Arc, Mutex};

use anima_memory::{MemorySearchOptions, RecentMemoryOptions};

use super::contracts::{
    MemoriesEnvelope, MemoryCreateRequest, MemoryResponse, MemorySearchEnvelope, MemorySearchQuery,
    MemorySearchResultResponse, RecentMemoriesQuery,
};
use super::ApiError;
use crate::state::DaemonState;

pub(crate) fn handle_create_memory(
    body: Vec<u8>,
    state: &Arc<Mutex<DaemonState>>,
) -> Result<MemoryResponse, ApiError> {
    let request: MemoryCreateRequest = super::parse_json_body(body)?;
    let new_memory = request
        .into_domain()
        .map_err(ApiError::bad_request_static)?;

    let memory = {
        let memory = {
            let guard = state
                .lock()
                .expect("daemon state mutex should not be poisoned");
            Arc::clone(&guard.memory)
        };
        let mut memory_guard = memory.lock().expect("memory mutex should not be poisoned");
        memory_guard
            .add(new_memory)
            .map_err(|error| ApiError::bad_request(error.message()))?
    };

    Ok(MemoryResponse::from(&memory))
}

pub(crate) fn handle_search_memories(
    query: MemorySearchQuery,
    state: &Arc<Mutex<DaemonState>>,
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

    let results = {
        let memory = {
            let guard = state
                .lock()
                .expect("daemon state mutex should not be poisoned");
            Arc::clone(&guard.memory)
        };
        let memory_guard = memory.lock().expect("memory mutex should not be poisoned");
        memory_guard.search(
            &search_query,
            MemorySearchOptions {
                agent_id: query.agent_id,
                agent_name: query.agent_name,
                memory_type,
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

pub(crate) fn handle_recent_memories(
    query: RecentMemoriesQuery,
    state: &Arc<Mutex<DaemonState>>,
) -> Result<MemoriesEnvelope, ApiError> {
    let memories = {
        let memory = {
            let guard = state
                .lock()
                .expect("daemon state mutex should not be poisoned");
            Arc::clone(&guard.memory)
        };
        let memory_guard = memory.lock().expect("memory mutex should not be poisoned");
        memory_guard.get_recent(RecentMemoryOptions {
            agent_id: query.agent_id,
            agent_name: query.agent_name,
            limit: query.limit,
        })
    };

    Ok(MemoriesEnvelope {
        memories: memories.iter().map(MemoryResponse::from).collect(),
    })
}
