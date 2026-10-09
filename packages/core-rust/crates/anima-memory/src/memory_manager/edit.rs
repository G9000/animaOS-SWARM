//! Owner edits: change a memory, delete one with its citations, delete an entity.

use super::{
    build_index_text, entity_key, unique_strings, validate_importance, Memory, MemoryError,
    MemoryManager, RelationshipEndpointKind,
};

/// A partial change to a memory. Absent fields keep their value.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemoryPatch {
    /// Trimmed; empty after trimming is `InvalidMemoryContent`.
    pub content: Option<String>,
    /// Validated like `MemoryManager::add`.
    pub importance: Option<f64>,
    /// `None` keeps, `Some(None)` clears, `Some(Some(v))` replaces (an empty list clears).
    pub tags: Option<Option<Vec<String>>>,
}

/// What `delete_memory` changed besides the memory itself. Ids are sorted ascending.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemoryDeletion {
    /// Agent relationships that cited the memory and were left with no evidence.
    pub removed_relationship_ids: Vec<String>,
    /// Agent relationships that cited the memory and keep other evidence.
    pub updated_relationship_ids: Vec<String>,
    /// Temporal facts that cited the memory (kept, citation removed).
    pub updated_fact_ids: Vec<String>,
    /// Temporal relationships that cited the memory (kept, citation removed).
    pub updated_temporal_relationship_ids: Vec<String>,
}

/// What `delete_entity` removed besides the entity itself. Ids are sorted ascending.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EntityDeletion {
    pub removed_relationship_ids: Vec<String>,
    pub removed_temporal_relationship_ids: Vec<String>,
    pub removed_fact_ids: Vec<String>,
}

impl MemoryManager {
    /// Ok(None): no such memory. Validation happens before anything changes.
    pub fn update_memory(
        &mut self,
        id: &str,
        patch: MemoryPatch,
    ) -> Result<Option<Memory>, MemoryError> {
        if !self.memories.contains_key(id) {
            return Ok(None);
        }
        let content = match patch.content {
            Some(content) => {
                let content = content.trim().to_string();
                if content.is_empty() {
                    return Err(MemoryError::InvalidMemoryContent);
                }
                Some(content)
            }
            None => None,
        };
        let importance = match patch.importance {
            Some(importance) => Some(validate_importance(importance)?),
            None => None,
        };
        let tags = patch.tags.map(|tags| {
            tags.map(unique_strings)
                .filter(|tags: &Vec<String>| !tags.is_empty())
        });

        let memory = self
            .memories
            .get_mut(id)
            .expect("memory presence was checked above");
        if let Some(content) = content {
            memory.content = content;
        }
        if let Some(importance) = importance {
            memory.importance = importance;
        }
        if let Some(tags) = tags {
            memory.tags = tags;
        }
        let updated = memory.clone();
        self.index
            .add_document(updated.id.clone(), build_index_text(&updated));
        Ok(Some(updated))
    }

    /// None: no such memory.
    ///
    /// Removes the memory and its index entry, then strips its id from every
    /// citation. Only agent relationships left with no evidence are removed;
    /// temporal facts and temporal relationships are kept even with no evidence.
    /// This is deliberately not `prune_relationship_evidence`, which would also
    /// remove relationships that never cited anything.
    pub fn delete_memory(&mut self, id: &str) -> Option<MemoryDeletion> {
        self.memories.remove(id)?;
        self.index.remove_document(id);

        let mut deletion = MemoryDeletion::default();
        let mut emptied = Vec::new();
        for relationship in self.agent_relationships.values_mut() {
            if !relationship.evidence_memory_ids.iter().any(|v| v == id) {
                continue;
            }
            relationship.evidence_memory_ids.retain(|v| v != id);
            if relationship.evidence_memory_ids.is_empty() {
                emptied.push(relationship.id.clone());
            } else {
                deletion
                    .updated_relationship_ids
                    .push(relationship.id.clone());
            }
        }
        for relationship_id in &emptied {
            self.agent_relationships.remove(relationship_id);
        }
        deletion.removed_relationship_ids = emptied;

        for fact in self.temporal_facts.values_mut() {
            if fact.evidence_memory_ids.iter().any(|v| v == id) {
                fact.evidence_memory_ids.retain(|v| v != id);
                deletion.updated_fact_ids.push(fact.id.clone());
            }
        }
        for relationship in self.temporal_relationships.values_mut() {
            if relationship.evidence_memory_ids.iter().any(|v| v == id) {
                relationship.evidence_memory_ids.retain(|v| v != id);
                deletion
                    .updated_temporal_relationship_ids
                    .push(relationship.id.clone());
            }
        }

        deletion.removed_relationship_ids.sort();
        deletion.updated_relationship_ids.sort();
        deletion.updated_fact_ids.sort();
        deletion.updated_temporal_relationship_ids.sort();
        Some(deletion)
    }

    /// Ok(None): no such entity. Err(EntityOwnsMemories): an agent entity that is
    /// the `agent_id` of any memory (loading would recreate it from its memories).
    ///
    /// Relationships and facts that name the entity are removed rather than edited,
    /// because loading recreates an entity from anything that names it.
    pub fn delete_entity(
        &mut self,
        kind: RelationshipEndpointKind,
        id: &str,
    ) -> Result<Option<EntityDeletion>, MemoryError> {
        let key = entity_key(kind, id);
        if !self.memory_entities.contains_key(&key) {
            return Ok(None);
        }
        if kind == RelationshipEndpointKind::Agent
            && self.memories.values().any(|memory| memory.agent_id == id)
        {
            return Err(MemoryError::EntityOwnsMemories);
        }

        let mut removed_relationship_ids: Vec<String> = self
            .agent_relationships
            .values()
            .filter(|r| {
                (r.source_kind == kind && r.source_agent_id == id)
                    || (r.target_kind == kind && r.target_agent_id == id)
            })
            .map(|r| r.id.clone())
            .collect();
        removed_relationship_ids.sort();
        for relationship_id in &removed_relationship_ids {
            self.agent_relationships.remove(relationship_id);
        }

        let mut removed_temporal_relationship_ids: Vec<String> = self
            .temporal_relationships
            .values()
            .filter(|r| {
                (r.source_kind == kind && r.source_id == id)
                    || (r.target_kind == kind && r.target_id == id)
            })
            .map(|r| r.id.clone())
            .collect();
        removed_temporal_relationship_ids.sort();
        for relationship_id in &removed_temporal_relationship_ids {
            self.temporal_relationships.remove(relationship_id);
        }
        for relationship in self.temporal_relationships.values_mut() {
            relationship
                .supersedes_relationship_ids
                .retain(|v| !removed_temporal_relationship_ids.contains(v));
        }

        let mut removed_fact_ids: Vec<String> = self
            .temporal_facts
            .values()
            .filter(|f| {
                (f.subject_kind == kind && f.subject_id == id)
                    || (f.object_kind == Some(kind) && f.object_id.as_deref() == Some(id))
            })
            .map(|f| f.id.clone())
            .collect();
        removed_fact_ids.sort();
        for fact_id in &removed_fact_ids {
            self.temporal_facts.remove(fact_id);
        }
        for fact in self.temporal_facts.values_mut() {
            fact.supersedes_fact_ids
                .retain(|v| !removed_fact_ids.contains(v));
        }

        self.memory_entities.remove(&key);
        Ok(Some(EntityDeletion {
            removed_relationship_ids,
            removed_temporal_relationship_ids,
            removed_fact_ids,
        }))
    }
}
