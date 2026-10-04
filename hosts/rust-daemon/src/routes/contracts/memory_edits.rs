//! The owner's memory edits (spec §10): change and delete a memory, list,
//! replace, and forget facts, and delete an entity.

use anima_memory::TemporalFact;
use serde::{Deserialize, Deserializer, Serialize};
use utoipa::ToSchema;

/// `PATCH /api/memories/{memory_id}`. Absent fields keep their value; at
/// least one must be present.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MemoryPatchRequest {
    /// Trimmed; 1 to 8,000 characters.
    #[serde(default)]
    pub(crate) content: Option<String>,
    /// From 0 to 1.
    #[serde(default)]
    pub(crate) importance: Option<f64>,
    /// `null` clears the tags; an array replaces them; absent keeps them.
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<Vec<String>>, nullable)]
    pub(crate) tags: Option<Option<Vec<String>>>,
}

/// A present `null` becomes `Some(None)`; an absent field stays `None`
/// (through `#[serde(default)]`).
fn double_option<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Deserialize::deserialize(deserializer).map(Some)
}

/// `DELETE /api/memories/{memory_id}`: the memory's id and what its
/// citation cleanup changed.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemoryDeleteResponse {
    pub(crate) id: String,
    /// Relationships that cited only this memory, now removed.
    pub(crate) removed_relationships: usize,
    /// Relationships that cited this memory and keep other evidence.
    pub(crate) updated_relationships: usize,
    /// Facts that cited this memory (kept, citation removed).
    pub(crate) updated_facts: usize,
}

/// A temporal fact. Timestamps are epoch milliseconds; absent values are `null`.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemoryFactResponse {
    pub(crate) id: String,
    pub(crate) subject_kind: String,
    pub(crate) subject_id: String,
    pub(crate) subject_name: String,
    pub(crate) predicate: String,
    pub(crate) object_kind: Option<String>,
    pub(crate) object_id: Option<String>,
    pub(crate) object_name: Option<String>,
    pub(crate) value: Option<String>,
    pub(crate) valid_from: Option<u64>,
    pub(crate) valid_to: Option<u64>,
    pub(crate) observed_at: u64,
    pub(crate) confidence: f64,
    pub(crate) evidence_memory_ids: Vec<String>,
    pub(crate) supersedes_fact_ids: Vec<String>,
    /// `active`, `superseded`, or `retracted`.
    pub(crate) status: String,
    pub(crate) tags: Option<Vec<String>>,
    pub(crate) room_id: Option<String>,
    pub(crate) world_id: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) created_at: u64,
    pub(crate) updated_at: u64,
}

impl From<&TemporalFact> for MemoryFactResponse {
    fn from(fact: &TemporalFact) -> Self {
        Self {
            id: fact.id.clone(),
            subject_kind: fact.subject_kind.as_str().to_string(),
            subject_id: fact.subject_id.clone(),
            subject_name: fact.subject_name.clone(),
            predicate: fact.predicate.clone(),
            object_kind: fact.object_kind.map(|kind| kind.as_str().to_string()),
            object_id: fact.object_id.clone(),
            object_name: fact.object_name.clone(),
            value: fact.value.clone(),
            valid_from: fact.valid_from,
            valid_to: fact.valid_to,
            observed_at: fact.observed_at,
            confidence: fact.confidence,
            evidence_memory_ids: fact.evidence_memory_ids.clone(),
            supersedes_fact_ids: fact.supersedes_fact_ids.clone(),
            status: fact.status.as_str().to_string(),
            tags: fact.tags.clone(),
            room_id: fact.room_id.clone(),
            world_id: fact.world_id.clone(),
            session_id: fact.session_id.clone(),
            created_at: fact.created_at,
            updated_at: fact.updated_at,
        }
    }
}

/// `GET /api/memories/facts`: newest first.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct MemoryFactsEnvelope {
    pub(crate) facts: Vec<MemoryFactResponse>,
}

/// `PATCH /api/memories/facts/{fact_id}`.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FactPatchRequest {
    /// Trimmed; 1 to 500 characters.
    pub(crate) value: String,
}

/// The new fact and the old one, re-read after it was superseded.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct FactReplacedResponse {
    pub(crate) fact: MemoryFactResponse,
    pub(crate) superseded: MemoryFactResponse,
}

/// `DELETE /api/memories/facts/{fact_id}`.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct FactDeleteResponse {
    pub(crate) id: String,
}

/// `DELETE /api/memories/entities/{entity_id}`.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EntityDeleteResponse {
    pub(crate) kind: String,
    pub(crate) id: String,
    /// Agent and temporal relationships that named the entity.
    pub(crate) removed_relationships: usize,
    /// Facts that named the entity.
    pub(crate) removed_facts: usize,
}
