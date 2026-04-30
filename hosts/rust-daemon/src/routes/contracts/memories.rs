use std::collections::HashMap;

use anima_memory::{Memory, MemoryScope, MemorySearchResult, MemoryType, NewMemory};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use super::shared::{parse_importance, parse_usize, required_string};

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemoryResponse {
    pub(crate) id: String,
    pub(crate) agent_id: String,
    pub(crate) agent_name: String,
    #[serde(rename = "type")]
    pub(crate) memory_type: String,
    pub(crate) content: String,
    pub(crate) importance: f64,
    pub(crate) created_at: u128,
    pub(crate) tags: Option<Vec<String>>,
    pub(crate) scope: String,
    pub(crate) room_id: Option<String>,
    pub(crate) world_id: Option<String>,
    pub(crate) session_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemorySearchResultResponse {
    pub(crate) id: String,
    pub(crate) agent_id: String,
    pub(crate) agent_name: String,
    #[serde(rename = "type")]
    pub(crate) memory_type: String,
    pub(crate) content: String,
    pub(crate) importance: f64,
    pub(crate) created_at: u128,
    pub(crate) tags: Option<Vec<String>>,
    pub(crate) score: f64,
    pub(crate) scope: String,
    pub(crate) room_id: Option<String>,
    pub(crate) world_id: Option<String>,
    pub(crate) session_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct MemoriesEnvelope {
    pub(crate) memories: Vec<MemoryResponse>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct MemorySearchEnvelope {
    pub(crate) results: Vec<MemorySearchResultResponse>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemoryCreateRequest {
    pub(crate) agent_id: Option<String>,
    pub(crate) agent_name: Option<String>,
    #[serde(rename = "type")]
    pub(crate) memory_type: Option<String>,
    pub(crate) content: Option<String>,
    pub(crate) importance: Option<f64>,
    pub(crate) tags: Option<Vec<String>>,
    pub(crate) scope: Option<String>,
    pub(crate) room_id: Option<String>,
    pub(crate) world_id: Option<String>,
    pub(crate) session_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, IntoParams, ToSchema, Default)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecentMemoriesQuery {
    pub(crate) agent_id: Option<String>,
    pub(crate) agent_name: Option<String>,
    pub(crate) scope: Option<String>,
    pub(crate) room_id: Option<String>,
    pub(crate) world_id: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) limit: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, IntoParams, ToSchema, Default)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemorySearchQuery {
    pub(crate) q: Option<String>,
    #[serde(rename = "type")]
    #[param(rename = "type")]
    pub(crate) memory_type: Option<String>,
    pub(crate) agent_id: Option<String>,
    pub(crate) agent_name: Option<String>,
    pub(crate) scope: Option<String>,
    pub(crate) room_id: Option<String>,
    pub(crate) world_id: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) limit: Option<usize>,
    pub(crate) min_importance: Option<f64>,
}

impl MemoryCreateRequest {
    pub(crate) fn into_domain(self) -> Result<NewMemory, &'static str> {
        let importance = self.importance.ok_or("importance is required")?;
        if !(0.0..=1.0).contains(&importance) {
            return Err("importance must be between 0 and 1");
        }

        let memory_type = match self.memory_type.as_deref() {
            Some(value) => MemoryType::parse(value)
                .map_err(|_| "type must be one of fact, observation, task_result, reflection")?,
            None => return Err("type is required"),
        };

        let scope = self
            .scope
            .as_deref()
            .map(MemoryScope::parse)
            .transpose()
            .map_err(|_| "scope must be one of shared, private, room")?;

        Ok(NewMemory {
            agent_id: required_string(self.agent_id, "agentId is required")?,
            agent_name: required_string(self.agent_name, "agentName is required")?,
            memory_type,
            content: required_string(self.content, "content is required")?,
            importance,
            tags: self.tags,
            scope,
            room_id: self.room_id,
            world_id: self.world_id,
            session_id: self.session_id,
        })
    }
}

impl RecentMemoriesQuery {
    pub(crate) fn from_query_map(query: &HashMap<String, String>) -> Result<Self, &'static str> {
        Ok(Self {
            agent_id: query.get("agentId").cloned(),
            agent_name: query.get("agentName").cloned(),
            scope: query.get("scope").cloned(),
            room_id: query.get("roomId").cloned(),
            world_id: query.get("worldId").cloned(),
            session_id: query.get("sessionId").cloned(),
            limit: query
                .get("limit")
                .map(String::as_str)
                .map(parse_usize)
                .transpose()?,
        })
    }
}

impl MemorySearchQuery {
    pub(crate) fn from_query_map(query: &HashMap<String, String>) -> Result<Self, &'static str> {
        Ok(Self {
            q: query.get("q").cloned(),
            memory_type: query.get("type").cloned(),
            agent_id: query.get("agentId").cloned(),
            agent_name: query.get("agentName").cloned(),
            scope: query.get("scope").cloned(),
            room_id: query.get("roomId").cloned(),
            world_id: query.get("worldId").cloned(),
            session_id: query.get("sessionId").cloned(),
            limit: query
                .get("limit")
                .map(String::as_str)
                .map(parse_usize)
                .transpose()?,
            min_importance: query
                .get("minImportance")
                .map(String::as_str)
                .map(parse_importance)
                .transpose()?,
        })
    }
}

impl From<&Memory> for MemoryResponse {
    fn from(value: &Memory) -> Self {
        Self {
            id: value.id.clone(),
            agent_id: value.agent_id.clone(),
            agent_name: value.agent_name.clone(),
            memory_type: value.memory_type.as_str().to_string(),
            content: value.content.clone(),
            importance: value.importance,
            created_at: value.created_at,
            tags: value.tags.clone(),
            scope: value.scope.as_str().to_string(),
            room_id: value.room_id.clone(),
            world_id: value.world_id.clone(),
            session_id: value.session_id.clone(),
        }
    }
}

impl From<&MemorySearchResult> for MemorySearchResultResponse {
    fn from(value: &MemorySearchResult) -> Self {
        Self {
            id: value.id.clone(),
            agent_id: value.agent_id.clone(),
            agent_name: value.agent_name.clone(),
            memory_type: value.memory_type.as_str().to_string(),
            content: value.content.clone(),
            importance: value.importance,
            created_at: value.created_at,
            tags: value.tags.clone(),
            score: value.score,
            scope: value.scope.as_str().to_string(),
            room_id: value.room_id.clone(),
            world_id: value.world_id.clone(),
            session_id: value.session_id.clone(),
        }
    }
}
