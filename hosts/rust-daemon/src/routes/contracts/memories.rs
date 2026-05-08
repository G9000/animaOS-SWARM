use std::collections::HashMap;

use anima_memory::{
    AgentRelationship, AgentRelationshipOptions, Memory, MemoryEntity, MemoryEntityOptions,
    MemoryEvalCaseResult, MemoryEvalCheckResult, MemoryEvalReport, MemoryEvaluation,
    MemoryEvaluationDecision, MemoryEvaluationOptions, MemoryEvidenceTrace,
    MemoryImportanceAdjustment, MemoryRecallOptions, MemoryRecallResult, MemoryRetentionPolicy,
    MemoryRetentionReport, MemoryScope, MemorySearchOptions, MemorySearchResult, MemoryType,
    NewAgentRelationship, NewMemory, NewMemoryEntity, RelationshipEndpointKind,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use super::shared::{parse_importance, parse_usize, required_string};
use crate::memory_embeddings::MemoryEmbeddingStatus;

fn parse_relationship_endpoint_kind(value: &str) -> Result<RelationshipEndpointKind, &'static str> {
    RelationshipEndpointKind::from_str(value)
        .map_err(|_| "endpoint kind must be one of agent, user, system, external")
}

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
    pub(crate) created_at: u64,
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
    pub(crate) created_at: u64,
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

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentRelationshipResponse {
    pub(crate) id: String,
    pub(crate) source_kind: String,
    pub(crate) source_agent_id: String,
    pub(crate) source_agent_name: String,
    pub(crate) target_kind: String,
    pub(crate) target_agent_id: String,
    pub(crate) target_agent_name: String,
    pub(crate) relationship_type: String,
    pub(crate) summary: Option<String>,
    pub(crate) strength: f64,
    pub(crate) confidence: f64,
    pub(crate) evidence_memory_ids: Vec<String>,
    pub(crate) tags: Option<Vec<String>>,
    pub(crate) room_id: Option<String>,
    pub(crate) world_id: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) created_at: u64,
    pub(crate) updated_at: u64,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct AgentRelationshipsEnvelope {
    pub(crate) relationships: Vec<AgentRelationshipResponse>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemoryEntityResponse {
    pub(crate) kind: String,
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) aliases: Vec<String>,
    pub(crate) summary: Option<String>,
    pub(crate) created_at: u64,
    pub(crate) updated_at: u64,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct MemoryEntitiesEnvelope {
    pub(crate) entities: Vec<MemoryEntityResponse>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemoryEvaluationResponse {
    pub(crate) decision: String,
    pub(crate) reason: String,
    pub(crate) score: f64,
    pub(crate) suggested_importance: f64,
    pub(crate) duplicate_memory_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemoryEvaluationOutcomeResponse {
    pub(crate) evaluation: MemoryEvaluationResponse,
    pub(crate) memory: Option<MemoryResponse>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemoryRecallResultResponse {
    pub(crate) memory: MemoryResponse,
    pub(crate) score: f64,
    pub(crate) lexical_score: f64,
    pub(crate) vector_score: f64,
    pub(crate) relationship_score: f64,
    pub(crate) temporal_score: f64,
    pub(crate) recency_score: f64,
    pub(crate) importance_score: f64,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct MemoryRecallEnvelope {
    pub(crate) results: Vec<MemoryRecallResultResponse>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemoryEvidenceTraceResponse {
    pub(crate) memory: MemoryResponse,
    pub(crate) relationships: Vec<AgentRelationshipResponse>,
    pub(crate) entities: Vec<MemoryEntityResponse>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemoryImportanceAdjustmentResponse {
    pub(crate) memory_id: String,
    pub(crate) previous_importance: f64,
    pub(crate) new_importance: f64,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemoryRetentionReportResponse {
    pub(crate) decayed_memories: Vec<MemoryImportanceAdjustmentResponse>,
    pub(crate) removed_memory_ids: Vec<String>,
    pub(crate) removed_relationship_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemoryEmbeddingStatusResponse {
    pub(crate) enabled: bool,
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) dimension: usize,
    pub(crate) vector_count: usize,
    pub(crate) persisted: bool,
    pub(crate) storage_file: Option<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemoryEvalCheckResultResponse {
    pub(crate) name: String,
    pub(crate) passed: bool,
    pub(crate) detail: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemoryEvalCaseResultResponse {
    pub(crate) name: String,
    pub(crate) checks: Vec<MemoryEvalCheckResultResponse>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemoryEvalReportResponse {
    pub(crate) passed: bool,
    pub(crate) total_checks: usize,
    pub(crate) passed_checks: usize,
    pub(crate) failure_messages: Vec<String>,
    pub(crate) cases: Vec<MemoryEvalCaseResultResponse>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemoryReadinessResponse {
    pub(crate) passed: bool,
    pub(crate) embeddings: MemoryEmbeddingStatusResponse,
    pub(crate) evaluation: MemoryEvalReportResponse,
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

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentRelationshipCreateRequest {
    pub(crate) source_kind: Option<String>,
    pub(crate) source_agent_id: Option<String>,
    pub(crate) source_agent_name: Option<String>,
    pub(crate) target_kind: Option<String>,
    pub(crate) target_agent_id: Option<String>,
    pub(crate) target_agent_name: Option<String>,
    pub(crate) relationship_type: Option<String>,
    pub(crate) summary: Option<String>,
    pub(crate) strength: Option<f64>,
    pub(crate) confidence: Option<f64>,
    pub(crate) evidence_memory_ids: Option<Vec<String>>,
    pub(crate) tags: Option<Vec<String>>,
    pub(crate) room_id: Option<String>,
    pub(crate) world_id: Option<String>,
    pub(crate) session_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemoryEntityCreateRequest {
    pub(crate) kind: Option<String>,
    pub(crate) id: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) aliases: Option<Vec<String>>,
    pub(crate) summary: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemoryEvaluationRequest {
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
    pub(crate) min_content_chars: Option<usize>,
    pub(crate) min_importance: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemoryRetentionRequest {
    pub(crate) max_age_millis: Option<u64>,
    pub(crate) min_importance: Option<f64>,
    pub(crate) max_memories: Option<usize>,
    pub(crate) decay_half_life_millis: Option<u64>,
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

#[derive(Clone, Debug, Deserialize, IntoParams, ToSchema, Default)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentRelationshipQuery {
    pub(crate) entity_id: Option<String>,
    pub(crate) agent_id: Option<String>,
    pub(crate) source_kind: Option<String>,
    pub(crate) source_agent_id: Option<String>,
    pub(crate) target_kind: Option<String>,
    pub(crate) target_agent_id: Option<String>,
    pub(crate) relationship_type: Option<String>,
    pub(crate) room_id: Option<String>,
    pub(crate) world_id: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) min_strength: Option<f64>,
    pub(crate) min_confidence: Option<f64>,
    pub(crate) limit: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, IntoParams, ToSchema, Default)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemoryEntityQuery {
    pub(crate) entity_id: Option<String>,
    pub(crate) kind: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) alias: Option<String>,
    pub(crate) limit: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, IntoParams, ToSchema, Default)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemoryRecallQuery {
    pub(crate) q: Option<String>,
    #[serde(rename = "type")]
    #[param(rename = "type")]
    pub(crate) memory_type: Option<String>,
    pub(crate) agent_id: Option<String>,
    pub(crate) agent_name: Option<String>,
    pub(crate) entity_id: Option<String>,
    pub(crate) recall_agent_id: Option<String>,
    pub(crate) scope: Option<String>,
    pub(crate) room_id: Option<String>,
    pub(crate) world_id: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) limit: Option<usize>,
    pub(crate) lexical_limit: Option<usize>,
    pub(crate) recent_limit: Option<usize>,
    pub(crate) relationship_limit: Option<usize>,
    pub(crate) temporal_limit: Option<usize>,
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

impl AgentRelationshipCreateRequest {
    pub(crate) fn into_domain(self) -> Result<NewAgentRelationship, &'static str> {
        let strength = self.strength.unwrap_or(0.5);
        if !(0.0..=1.0).contains(&strength) {
            return Err("strength must be between 0 and 1");
        }
        let confidence = self.confidence.unwrap_or(0.5);
        if !(0.0..=1.0).contains(&confidence) {
            return Err("confidence must be between 0 and 1");
        }

        Ok(NewAgentRelationship {
            source_kind: self
                .source_kind
                .as_deref()
                .map(parse_relationship_endpoint_kind)
                .transpose()?,
            source_agent_id: required_string(self.source_agent_id, "sourceAgentId is required")?,
            source_agent_name: required_string(
                self.source_agent_name,
                "sourceAgentName is required",
            )?,
            target_kind: self
                .target_kind
                .as_deref()
                .map(parse_relationship_endpoint_kind)
                .transpose()?,
            target_agent_id: required_string(self.target_agent_id, "targetAgentId is required")?,
            target_agent_name: required_string(
                self.target_agent_name,
                "targetAgentName is required",
            )?,
            relationship_type: required_string(
                self.relationship_type,
                "relationshipType is required",
            )?,
            summary: self.summary,
            strength,
            confidence,
            evidence_memory_ids: self.evidence_memory_ids.unwrap_or_default(),
            tags: self.tags,
            room_id: self.room_id,
            world_id: self.world_id,
            session_id: self.session_id,
        })
    }
}

impl MemoryEntityCreateRequest {
    pub(crate) fn into_domain(self) -> Result<NewMemoryEntity, &'static str> {
        let kind = self
            .kind
            .as_deref()
            .ok_or("kind is required")
            .and_then(parse_relationship_endpoint_kind)?;

        Ok(NewMemoryEntity {
            kind,
            id: required_string(self.id, "id is required")?,
            name: required_string(self.name, "name is required")?,
            aliases: self.aliases.unwrap_or_default(),
            summary: self.summary,
        })
    }
}

impl MemoryEvaluationRequest {
    pub(crate) fn into_domain(self) -> Result<(NewMemory, MemoryEvaluationOptions), &'static str> {
        let memory = MemoryCreateRequest {
            agent_id: self.agent_id,
            agent_name: self.agent_name,
            memory_type: self.memory_type,
            content: self.content,
            importance: self.importance,
            tags: self.tags,
            scope: self.scope,
            room_id: self.room_id,
            world_id: self.world_id,
            session_id: self.session_id,
        }
        .into_domain()?;

        let default_options = MemoryEvaluationOptions::default();
        let min_importance = self
            .min_importance
            .unwrap_or(default_options.min_importance);
        if !min_importance.is_finite() || !(0.0..=1.0).contains(&min_importance) {
            return Err("minImportance must be between 0 and 1");
        }

        Ok((
            memory,
            MemoryEvaluationOptions {
                min_content_chars: self
                    .min_content_chars
                    .unwrap_or(default_options.min_content_chars),
                min_importance,
            },
        ))
    }
}

impl MemoryRetentionRequest {
    pub(crate) fn into_domain(self) -> Result<MemoryRetentionPolicy, &'static str> {
        if self.max_age_millis == Some(0) {
            return Err("maxAgeMillis must be greater than 0");
        }
        if self.decay_half_life_millis == Some(0) {
            return Err("decayHalfLifeMillis must be greater than 0");
        }
        if self.max_memories == Some(0) {
            return Err("maxMemories must be greater than 0");
        }
        let default_options = MemoryEvaluationOptions::default();
        let min_importance = self
            .min_importance
            .unwrap_or(default_options.min_importance);
        if !min_importance.is_finite() || !(0.0..=1.0).contains(&min_importance) {
            return Err("minImportance must be between 0 and 1");
        }

        Ok(MemoryRetentionPolicy {
            max_age_millis: self.max_age_millis,
            min_importance: self.min_importance,
            max_memories: self.max_memories,
            decay_half_life_millis: self.decay_half_life_millis,
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

impl AgentRelationshipQuery {
    pub(crate) fn from_query_map(query: &HashMap<String, String>) -> Result<Self, &'static str> {
        Ok(Self {
            entity_id: query.get("entityId").cloned(),
            agent_id: query.get("agentId").cloned(),
            source_kind: query
                .get("sourceKind")
                .map(|kind| {
                    parse_relationship_endpoint_kind(kind.as_str())?;
                    Ok::<String, &'static str>(kind.clone())
                })
                .transpose()?,
            source_agent_id: query.get("sourceAgentId").cloned(),
            target_kind: query
                .get("targetKind")
                .map(|kind| {
                    parse_relationship_endpoint_kind(kind.as_str())?;
                    Ok::<String, &'static str>(kind.clone())
                })
                .transpose()?,
            target_agent_id: query.get("targetAgentId").cloned(),
            relationship_type: query.get("relationshipType").cloned(),
            room_id: query.get("roomId").cloned(),
            world_id: query.get("worldId").cloned(),
            session_id: query.get("sessionId").cloned(),
            min_strength: query
                .get("minStrength")
                .map(String::as_str)
                .map(parse_importance)
                .transpose()?,
            min_confidence: query
                .get("minConfidence")
                .map(String::as_str)
                .map(parse_importance)
                .transpose()?,
            limit: query
                .get("limit")
                .map(String::as_str)
                .map(parse_usize)
                .transpose()?,
        })
    }

    pub(crate) fn into_domain(self) -> AgentRelationshipOptions {
        AgentRelationshipOptions {
            entity_id: self.entity_id,
            agent_id: self.agent_id,
            source_kind: self
                .source_kind
                .as_deref()
                .and_then(|kind| RelationshipEndpointKind::from_str(kind).ok()),
            source_agent_id: self.source_agent_id,
            target_kind: self
                .target_kind
                .as_deref()
                .and_then(|kind| RelationshipEndpointKind::from_str(kind).ok()),
            target_agent_id: self.target_agent_id,
            relationship_type: self.relationship_type,
            room_id: self.room_id,
            world_id: self.world_id,
            session_id: self.session_id,
            min_strength: self.min_strength,
            min_confidence: self.min_confidence,
            limit: self.limit,
        }
    }
}

impl MemoryEntityQuery {
    pub(crate) fn from_query_map(query: &HashMap<String, String>) -> Result<Self, &'static str> {
        Ok(Self {
            entity_id: query.get("entityId").cloned(),
            kind: query
                .get("kind")
                .map(|kind| {
                    parse_relationship_endpoint_kind(kind.as_str())?;
                    Ok::<String, &'static str>(kind.clone())
                })
                .transpose()?,
            name: query.get("name").cloned(),
            alias: query.get("alias").cloned(),
            limit: query
                .get("limit")
                .map(String::as_str)
                .map(parse_usize)
                .transpose()?,
        })
    }

    pub(crate) fn into_domain(self) -> MemoryEntityOptions {
        MemoryEntityOptions {
            entity_id: self.entity_id,
            kind: self
                .kind
                .as_deref()
                .and_then(|kind| RelationshipEndpointKind::from_str(kind).ok()),
            name: self.name,
            alias: self.alias,
            limit: self.limit,
        }
    }
}

impl MemoryRecallQuery {
    pub(crate) fn from_query_map(query: &HashMap<String, String>) -> Result<Self, &'static str> {
        Ok(Self {
            q: query.get("q").cloned(),
            memory_type: query.get("type").cloned(),
            agent_id: query.get("agentId").cloned(),
            agent_name: query.get("agentName").cloned(),
            entity_id: query.get("entityId").cloned(),
            recall_agent_id: query.get("recallAgentId").cloned(),
            scope: query.get("scope").cloned(),
            room_id: query.get("roomId").cloned(),
            world_id: query.get("worldId").cloned(),
            session_id: query.get("sessionId").cloned(),
            limit: query
                .get("limit")
                .map(String::as_str)
                .map(parse_usize)
                .transpose()?,
            lexical_limit: query
                .get("lexicalLimit")
                .map(String::as_str)
                .map(parse_usize)
                .transpose()?,
            recent_limit: query
                .get("recentLimit")
                .map(String::as_str)
                .map(parse_usize)
                .transpose()?,
            relationship_limit: query
                .get("relationshipLimit")
                .map(String::as_str)
                .map(parse_usize)
                .transpose()?,
            temporal_limit: query
                .get("temporalLimit")
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

    pub(crate) fn into_domain(self) -> Result<(String, MemoryRecallOptions), &'static str> {
        let query = self
            .q
            .filter(|value| !value.is_empty())
            .ok_or("q query parameter is required")?;
        let memory_type = self
            .memory_type
            .as_deref()
            .map(MemoryType::parse)
            .transpose()
            .map_err(|_| "type must be one of fact, observation, task_result, reflection")?;
        let scope = self
            .scope
            .as_deref()
            .map(MemoryScope::parse)
            .transpose()
            .map_err(|_| "scope must be one of shared, private, room")?;

        Ok((
            query,
            MemoryRecallOptions {
                search: MemorySearchOptions {
                    agent_id: self.agent_id,
                    agent_name: self.agent_name,
                    memory_type,
                    scope,
                    room_id: self.room_id,
                    world_id: self.world_id,
                    session_id: self.session_id,
                    limit: self.lexical_limit,
                    min_importance: self.min_importance,
                },
                entity_id: self.entity_id,
                agent_id: self.recall_agent_id,
                limit: self.limit,
                lexical_limit: self.lexical_limit,
                recent_limit: self.recent_limit,
                relationship_limit: self.relationship_limit,
                temporal_limit: self.temporal_limit,
                temporal_intent_terms: Vec::new(),
                weights: None,
            },
        ))
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

impl From<&AgentRelationship> for AgentRelationshipResponse {
    fn from(value: &AgentRelationship) -> Self {
        Self {
            id: value.id.clone(),
            source_kind: value.source_kind.as_str().to_string(),
            source_agent_id: value.source_agent_id.clone(),
            source_agent_name: value.source_agent_name.clone(),
            target_kind: value.target_kind.as_str().to_string(),
            target_agent_id: value.target_agent_id.clone(),
            target_agent_name: value.target_agent_name.clone(),
            relationship_type: value.relationship_type.clone(),
            summary: value.summary.clone(),
            strength: value.strength,
            confidence: value.confidence,
            evidence_memory_ids: value.evidence_memory_ids.clone(),
            tags: value.tags.clone(),
            room_id: value.room_id.clone(),
            world_id: value.world_id.clone(),
            session_id: value.session_id.clone(),
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

impl From<&MemoryEntity> for MemoryEntityResponse {
    fn from(value: &MemoryEntity) -> Self {
        Self {
            kind: value.kind.as_str().to_string(),
            id: value.id.clone(),
            name: value.name.clone(),
            aliases: value.aliases.clone(),
            summary: value.summary.clone(),
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

impl From<&MemoryEvaluation> for MemoryEvaluationResponse {
    fn from(value: &MemoryEvaluation) -> Self {
        Self {
            decision: match value.decision {
                MemoryEvaluationDecision::Store => "store",
                MemoryEvaluationDecision::Merge => "merge",
                MemoryEvaluationDecision::Ignore => "ignore",
            }
            .to_string(),
            reason: value.reason.clone(),
            score: value.score,
            suggested_importance: value.suggested_importance,
            duplicate_memory_id: value.duplicate_memory_id.clone(),
        }
    }
}

impl From<&MemoryRecallResult> for MemoryRecallResultResponse {
    fn from(value: &MemoryRecallResult) -> Self {
        Self {
            memory: MemoryResponse::from(&value.memory),
            score: value.score,
            lexical_score: value.lexical_score,
            vector_score: value.vector_score,
            relationship_score: value.relationship_score,
            temporal_score: value.temporal_score,
            recency_score: value.recency_score,
            importance_score: value.importance_score,
        }
    }
}

impl From<&MemoryEvidenceTrace> for MemoryEvidenceTraceResponse {
    fn from(value: &MemoryEvidenceTrace) -> Self {
        Self {
            memory: MemoryResponse::from(&value.memory),
            relationships: value
                .relationships
                .iter()
                .map(AgentRelationshipResponse::from)
                .collect(),
            entities: value
                .entities
                .iter()
                .map(MemoryEntityResponse::from)
                .collect(),
        }
    }
}

impl From<&MemoryImportanceAdjustment> for MemoryImportanceAdjustmentResponse {
    fn from(value: &MemoryImportanceAdjustment) -> Self {
        Self {
            memory_id: value.memory_id.clone(),
            previous_importance: value.previous_importance,
            new_importance: value.new_importance,
        }
    }
}

impl From<&MemoryRetentionReport> for MemoryRetentionReportResponse {
    fn from(value: &MemoryRetentionReport) -> Self {
        Self {
            decayed_memories: value
                .decayed_memories
                .iter()
                .map(MemoryImportanceAdjustmentResponse::from)
                .collect(),
            removed_memory_ids: value.removed_memory_ids.clone(),
            removed_relationship_ids: value.removed_relationship_ids.clone(),
        }
    }
}

impl From<&MemoryEmbeddingStatus> for MemoryEmbeddingStatusResponse {
    fn from(value: &MemoryEmbeddingStatus) -> Self {
        Self {
            enabled: value.enabled,
            provider: value.provider.clone(),
            model: value.model.clone(),
            dimension: value.dimension,
            vector_count: value.vector_count,
            persisted: value.persisted,
            storage_file: value
                .storage_file
                .as_ref()
                .map(|path| path.display().to_string()),
        }
    }
}

impl From<&MemoryEvalCheckResult> for MemoryEvalCheckResultResponse {
    fn from(value: &MemoryEvalCheckResult) -> Self {
        Self {
            name: value.name.clone(),
            passed: value.passed,
            detail: value.detail.clone(),
        }
    }
}

impl From<&MemoryEvalCaseResult> for MemoryEvalCaseResultResponse {
    fn from(value: &MemoryEvalCaseResult) -> Self {
        Self {
            name: value.name.clone(),
            checks: value
                .checks
                .iter()
                .map(MemoryEvalCheckResultResponse::from)
                .collect(),
        }
    }
}

impl From<&MemoryEvalReport> for MemoryEvalReportResponse {
    fn from(value: &MemoryEvalReport) -> Self {
        Self {
            passed: value.passed(),
            total_checks: value.total_checks(),
            passed_checks: value.passed_checks(),
            failure_messages: value.failure_messages(),
            cases: value
                .cases
                .iter()
                .map(MemoryEvalCaseResultResponse::from)
                .collect(),
        }
    }
}
