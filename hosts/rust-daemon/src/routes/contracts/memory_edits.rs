//! The owner's memory edits (spec §10): change and delete a memory.

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
