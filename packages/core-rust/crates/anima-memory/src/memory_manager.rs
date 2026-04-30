mod storage;
mod storage_json;
#[cfg(test)]
mod tests;
mod types;

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use self::storage::{load_memory_store, save_memory_store};
pub use self::types::{
    AgentRelationship, AgentRelationshipOptions, Memory, MemoryEntity, MemoryEntityOptions,
    MemoryError, MemoryEvaluation, MemoryEvaluationDecision, MemoryEvaluationOptions,
    MemoryEvaluationOutcome, MemoryRecallOptions, MemoryRecallResult, MemoryScope,
    MemorySearchOptions, MemorySearchResult, MemoryType, MemoryVectorIndex, NewAgentRelationship,
    NewMemory, NewMemoryEntity, RecentMemoryOptions, RelationshipEndpointKind, VectorMemoryHit,
};
use crate::bm25::BM25;

static NEXT_MEMORY_ID: AtomicU64 = AtomicU64::new(0);
static NEXT_RELATIONSHIP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
pub struct MemoryManager {
    memories: HashMap<String, Memory>,
    memory_entities: HashMap<String, MemoryEntity>,
    agent_relationships: HashMap<String, AgentRelationship>,
    index: BM25,
    storage_file: Option<PathBuf>,
}

impl MemoryManager {
    pub fn new() -> Self {
        Self {
            memories: HashMap::new(),
            memory_entities: HashMap::new(),
            agent_relationships: HashMap::new(),
            index: BM25::default(),
            storage_file: None,
        }
    }

    pub fn with_storage_file(path: impl Into<PathBuf>) -> Self {
        let mut manager = Self::new();
        manager.storage_file = Some(path.into());
        manager
    }

    pub fn add(&mut self, memory: NewMemory) -> Result<Memory, MemoryError> {
        let importance = validate_importance(memory.importance)?;
        let full = Memory {
            id: next_memory_id(),
            agent_id: memory.agent_id,
            agent_name: memory.agent_name,
            memory_type: memory.memory_type,
            content: memory.content,
            importance,
            created_at: now_millis(),
            tags: memory.tags,
            scope: memory
                .scope
                .unwrap_or_else(|| default_scope(&memory.room_id)),
            room_id: memory.room_id,
            world_id: memory.world_id,
            session_id: memory.session_id,
        };

        self.memories.insert(full.id.clone(), full.clone());
        self.index
            .add_document(full.id.clone(), build_index_text(&full));
        self.ensure_entity_from_parts(
            RelationshipEndpointKind::Agent,
            &full.agent_id,
            &full.agent_name,
        );
        Ok(full)
    }

    pub fn evaluate_new_memory(
        &self,
        memory: &NewMemory,
        opts: MemoryEvaluationOptions,
    ) -> Result<MemoryEvaluation, MemoryError> {
        let importance = validate_importance(memory.importance)?;
        let min_importance = validate_importance(opts.min_importance)?;
        let content = memory.content.trim();
        if content.is_empty() {
            return Ok(MemoryEvaluation {
                decision: MemoryEvaluationDecision::Ignore,
                reason: "memory content is empty".into(),
                score: 0.0,
                suggested_importance: importance,
                duplicate_memory_id: None,
            });
        }

        let scope = memory
            .scope
            .unwrap_or_else(|| default_scope(&memory.room_id));
        if let Some(duplicate) = self.find_duplicate_memory(memory, scope) {
            return Ok(MemoryEvaluation {
                decision: MemoryEvaluationDecision::Merge,
                reason: "memory duplicates existing evidence".into(),
                score: 0.15,
                suggested_importance: duplicate.importance.max(importance),
                duplicate_memory_id: Some(duplicate.id.clone()),
            });
        }

        let score = memory_evaluation_score(memory, importance);
        if content.chars().count() < opts.min_content_chars && importance < min_importance {
            return Ok(MemoryEvaluation {
                decision: MemoryEvaluationDecision::Ignore,
                reason: "memory is too short and below the importance threshold".into(),
                score,
                suggested_importance: importance,
                duplicate_memory_id: None,
            });
        }

        Ok(MemoryEvaluation {
            decision: MemoryEvaluationDecision::Store,
            reason: "memory contains distinct evidence".into(),
            score,
            suggested_importance: score.max(importance).min(1.0),
            duplicate_memory_id: None,
        })
    }

    pub fn add_evaluated(
        &mut self,
        mut memory: NewMemory,
        opts: MemoryEvaluationOptions,
    ) -> Result<MemoryEvaluationOutcome, MemoryError> {
        let evaluation = self.evaluate_new_memory(&memory, opts)?;
        let stored = if evaluation.decision == MemoryEvaluationDecision::Store {
            memory.importance = evaluation.suggested_importance;
            Some(self.add(memory)?)
        } else {
            None
        };

        Ok(MemoryEvaluationOutcome {
            evaluation,
            memory: stored,
        })
    }

    pub fn recall(&self, query: &str, opts: MemoryRecallOptions) -> Vec<MemoryRecallResult> {
        self.recall_with_vector_index(query, opts, None)
    }

    pub fn recall_with_vector_index(
        &self,
        query: &str,
        opts: MemoryRecallOptions,
        vector_index: Option<&dyn MemoryVectorIndex>,
    ) -> Vec<MemoryRecallResult> {
        let limit = opts.limit.unwrap_or(10);
        let lexical_limit = opts
            .lexical_limit
            .unwrap_or(limit.saturating_mul(4).max(limit));
        let mut candidates: HashMap<String, RecallCandidate> = HashMap::new();

        let mut lexical_opts = opts.search.clone();
        lexical_opts.limit = Some(lexical_limit);
        let lexical_results = self.search(query, lexical_opts);
        let max_lexical_score = lexical_results
            .iter()
            .map(|result| result.score)
            .fold(0.0_f64, f64::max);
        for result in lexical_results {
            if let Some(memory) = self.memories.get(&result.id) {
                let candidate = recall_candidate(&mut candidates, memory);
                candidate.lexical_score = result.score;
            }
        }

        if let Some(vector_index) = vector_index {
            let vector_hits = vector_index.search(query, lexical_limit);
            let max_vector_score = vector_hits
                .iter()
                .filter(|hit| hit.score.is_finite() && hit.score > 0.0)
                .map(|hit| hit.score)
                .fold(0.0_f64, f64::max);
            for hit in vector_hits {
                let Some(memory) = self.memories.get(&hit.memory_id) else {
                    continue;
                };
                if !memory_matches_search_options(memory, &opts.search) {
                    continue;
                }
                if max_vector_score > 0.0 && hit.score.is_finite() && hit.score > 0.0 {
                    let candidate = recall_candidate(&mut candidates, memory);
                    candidate.vector_score =
                        candidate.vector_score.max(hit.score / max_vector_score);
                }
            }
        }

        let recent_limit = opts.recent_limit.unwrap_or(limit);
        if recent_limit > 0 {
            let recent = self.get_recent(recent_options_from_search(&opts.search, recent_limit));
            for (index, memory) in recent.iter().enumerate() {
                let candidate = recall_candidate(&mut candidates, memory);
                candidate.recency_score = candidate
                    .recency_score
                    .max(1.0 / f64::from((index + 1) as u32));
            }
        }

        let relationship_agent_id = opts.agent_id.as_ref().or(opts.search.agent_id.as_ref());
        if opts.entity_id.is_some() || relationship_agent_id.is_some() {
            let relationships = self.list_agent_relationships(AgentRelationshipOptions {
                entity_id: opts.entity_id.clone(),
                agent_id: relationship_agent_id.cloned(),
                room_id: opts.search.room_id.clone(),
                world_id: opts.search.world_id.clone(),
                session_id: opts.search.session_id.clone(),
                limit: Some(opts.relationship_limit.unwrap_or(20)),
                ..AgentRelationshipOptions::default()
            });

            for relationship in relationships {
                let relationship_score = (relationship.strength * relationship.confidence).min(1.0);
                for memory_id in relationship.evidence_memory_ids {
                    let Some(memory) = self.memories.get(&memory_id) else {
                        continue;
                    };
                    if !memory_matches_search_options(memory, &opts.search) {
                        continue;
                    }
                    let candidate = recall_candidate(&mut candidates, memory);
                    candidate.relationship_score =
                        candidate.relationship_score.max(relationship_score);
                }
            }
        }

        let mut results: Vec<_> = candidates
            .into_values()
            .map(|candidate| candidate.into_result(max_lexical_score))
            .collect();
        results.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| right.memory.created_at.cmp(&left.memory.created_at))
                .then_with(|| right.memory.id.cmp(&left.memory.id))
        });
        results.truncate(limit);
        results
    }

    pub fn upsert_entity(&mut self, entity: NewMemoryEntity) -> Result<MemoryEntity, MemoryError> {
        let id = normalize_required_string(entity.id, MemoryError::InvalidEntityId)?;
        let name = normalize_required_string(entity.name, MemoryError::InvalidEntityName)?;
        Ok(self.upsert_entity_parts(entity.kind, id, name, entity.aliases, entity.summary))
    }

    pub fn get_entity(&self, kind: RelationshipEndpointKind, id: &str) -> Option<MemoryEntity> {
        self.memory_entities.get(&entity_key(kind, id)).cloned()
    }

    pub fn list_entities(&self, opts: MemoryEntityOptions) -> Vec<MemoryEntity> {
        let name_filter = opts.name.as_ref().map(|name| name.to_ascii_lowercase());
        let alias_filter = opts.alias.as_ref().map(|alias| alias.to_ascii_lowercase());
        let mut entities: Vec<_> =
            self.memory_entities
                .values()
                .filter(|entity| {
                    if opts
                        .entity_id
                        .as_deref()
                        .is_some_and(|entity_id| entity.id != entity_id)
                    {
                        return false;
                    }
                    if opts.kind.is_some_and(|kind| entity.kind != kind) {
                        return false;
                    }
                    if name_filter.as_ref().is_some_and(|name| {
                        !entity.name.to_ascii_lowercase().contains(name.as_str())
                    }) {
                        return false;
                    }
                    if alias_filter.as_ref().is_some_and(|alias| {
                        !entity
                            .aliases
                            .iter()
                            .any(|value| value.eq_ignore_ascii_case(alias))
                    }) {
                        return false;
                    }
                    true
                })
                .cloned()
                .collect();

        entities.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.kind.as_str().cmp(right.kind.as_str()))
                .then_with(|| left.id.cmp(&right.id))
        });
        entities.truncate(opts.limit.unwrap_or(20));
        entities
    }

    pub fn search(&self, query: &str, opts: MemorySearchOptions) -> Vec<MemorySearchResult> {
        let limit = opts.limit.unwrap_or(10);
        let min_importance = opts.min_importance.unwrap_or(0.0);
        let raw = self.index.search(query, limit.saturating_mul(3));

        let mut results = Vec::new();
        for result in raw {
            let Some(memory) = self.memories.get(&result.id) else {
                continue;
            };
            if opts
                .agent_id
                .as_deref()
                .is_some_and(|agent_id| memory.agent_id != agent_id)
            {
                continue;
            }
            if opts
                .agent_name
                .as_deref()
                .is_some_and(|agent_name| memory.agent_name != agent_name)
            {
                continue;
            }
            if opts
                .memory_type
                .is_some_and(|memory_type| memory.memory_type != memory_type)
            {
                continue;
            }
            if opts.scope.is_some_and(|scope| memory.scope != scope) {
                continue;
            }
            if option_filter_misses(opts.room_id.as_deref(), memory.room_id.as_deref()) {
                continue;
            }
            if option_filter_misses(opts.world_id.as_deref(), memory.world_id.as_deref()) {
                continue;
            }
            if option_filter_misses(opts.session_id.as_deref(), memory.session_id.as_deref()) {
                continue;
            }
            if memory.importance < min_importance {
                continue;
            }

            results.push(MemorySearchResult::from_memory(memory, result.score));
            if results.len() >= limit {
                break;
            }
        }

        results
    }

    pub fn get_recent(&self, opts: RecentMemoryOptions) -> Vec<Memory> {
        let mut memories: Vec<_> = self
            .memories
            .values()
            .filter(|memory| {
                if opts
                    .agent_id
                    .as_deref()
                    .is_some_and(|agent_id| memory.agent_id != agent_id)
                {
                    return false;
                }
                if opts
                    .agent_name
                    .as_deref()
                    .is_some_and(|agent_name| memory.agent_name != agent_name)
                {
                    return false;
                }
                if opts.scope.is_some_and(|scope| memory.scope != scope) {
                    return false;
                }
                if option_filter_misses(opts.room_id.as_deref(), memory.room_id.as_deref()) {
                    return false;
                }
                if option_filter_misses(opts.world_id.as_deref(), memory.world_id.as_deref()) {
                    return false;
                }
                if option_filter_misses(opts.session_id.as_deref(), memory.session_id.as_deref()) {
                    return false;
                }
                true
            })
            .cloned()
            .collect();

        memories.sort_by(|left, right| {
            right.created_at.cmp(&left.created_at).then_with(|| {
                memory_id_sequence(&right.id)
                    .cmp(&memory_id_sequence(&left.id))
                    .then_with(|| right.id.cmp(&left.id))
            })
        });
        memories.truncate(opts.limit.unwrap_or(20));
        memories
    }

    pub fn upsert_agent_relationship(
        &mut self,
        relationship: NewAgentRelationship,
    ) -> Result<AgentRelationship, MemoryError> {
        let source_agent_id = normalize_required_string(
            relationship.source_agent_id,
            MemoryError::InvalidRelationshipEndpoint,
        )?;
        let source_agent_name = normalize_required_string(
            relationship.source_agent_name,
            MemoryError::InvalidRelationshipEndpointName,
        )?;
        let source_kind = relationship.source_kind.unwrap_or_default();
        let target_agent_id = normalize_required_string(
            relationship.target_agent_id,
            MemoryError::InvalidRelationshipEndpoint,
        )?;
        let target_agent_name = normalize_required_string(
            relationship.target_agent_name,
            MemoryError::InvalidRelationshipEndpointName,
        )?;
        let target_kind = relationship.target_kind.unwrap_or_default();
        let relationship_type = normalize_required_string(
            relationship.relationship_type,
            MemoryError::InvalidRelationshipType,
        )?;
        let strength = validate_unit_interval(
            relationship.strength,
            MemoryError::InvalidRelationshipStrength,
        )?;
        let confidence = validate_unit_interval(
            relationship.confidence,
            MemoryError::InvalidRelationshipConfidence,
        )?;
        let now = now_millis();

        self.upsert_entity_parts(
            source_kind,
            source_agent_id.clone(),
            source_agent_name.clone(),
            Vec::new(),
            None,
        );
        self.upsert_entity_parts(
            target_kind,
            target_agent_id.clone(),
            target_agent_name.clone(),
            Vec::new(),
            None,
        );

        let existing_id = self
            .agent_relationships
            .iter()
            .find(|(_, existing)| {
                same_agent_relationship_edge(
                    existing,
                    source_kind,
                    &source_agent_id,
                    target_kind,
                    &target_agent_id,
                    &relationship_type,
                    relationship.world_id.as_deref(),
                )
            })
            .map(|(id, _)| id.clone());

        if let Some(existing_id) = existing_id {
            let existing = self
                .agent_relationships
                .get_mut(&existing_id)
                .expect("relationship id should exist");
            existing.source_agent_name = source_agent_name;
            existing.target_agent_name = target_agent_name;
            existing.summary = relationship.summary.or_else(|| existing.summary.clone());
            existing.strength = strength;
            existing.confidence = confidence;
            merge_unique(
                &mut existing.evidence_memory_ids,
                relationship.evidence_memory_ids,
            );
            merge_optional_unique(&mut existing.tags, relationship.tags);
            existing.room_id = relationship.room_id.or_else(|| existing.room_id.clone());
            existing.session_id = relationship
                .session_id
                .or_else(|| existing.session_id.clone());
            existing.updated_at = now;
            return Ok(existing.clone());
        }

        let full = AgentRelationship {
            id: next_relationship_id(),
            source_kind,
            source_agent_id,
            source_agent_name,
            target_kind,
            target_agent_id,
            target_agent_name,
            relationship_type,
            summary: relationship.summary,
            strength,
            confidence,
            evidence_memory_ids: unique_strings(relationship.evidence_memory_ids),
            tags: relationship
                .tags
                .map(unique_strings)
                .filter(|tags| !tags.is_empty()),
            room_id: relationship.room_id,
            world_id: relationship.world_id,
            session_id: relationship.session_id,
            created_at: now,
            updated_at: now,
        };

        self.agent_relationships
            .insert(full.id.clone(), full.clone());
        Ok(full)
    }

    pub fn list_agent_relationships(
        &self,
        opts: AgentRelationshipOptions,
    ) -> Vec<AgentRelationship> {
        let min_strength = opts.min_strength.unwrap_or(0.0);
        let min_confidence = opts.min_confidence.unwrap_or(0.0);
        let mut relationships: Vec<_> = self
            .agent_relationships
            .values()
            .filter(|relationship| {
                if opts.entity_id.as_deref().is_some_and(|entity_id| {
                    relationship.source_agent_id != entity_id
                        && relationship.target_agent_id != entity_id
                }) {
                    return false;
                }
                if opts.agent_id.as_deref().is_some_and(|agent_id| {
                    (relationship.source_kind != RelationshipEndpointKind::Agent
                        || relationship.source_agent_id != agent_id)
                        && (relationship.target_kind != RelationshipEndpointKind::Agent
                            || relationship.target_agent_id != agent_id)
                }) {
                    return false;
                }
                if opts
                    .source_kind
                    .is_some_and(|kind| relationship.source_kind != kind)
                {
                    return false;
                }
                if option_filter_misses(
                    opts.source_agent_id.as_deref(),
                    Some(&relationship.source_agent_id),
                ) {
                    return false;
                }
                if opts
                    .target_kind
                    .is_some_and(|kind| relationship.target_kind != kind)
                {
                    return false;
                }
                if option_filter_misses(
                    opts.target_agent_id.as_deref(),
                    Some(&relationship.target_agent_id),
                ) {
                    return false;
                }
                if option_filter_misses(
                    opts.relationship_type.as_deref(),
                    Some(&relationship.relationship_type),
                ) {
                    return false;
                }
                if option_filter_misses(opts.room_id.as_deref(), relationship.room_id.as_deref()) {
                    return false;
                }
                if option_filter_misses(opts.world_id.as_deref(), relationship.world_id.as_deref())
                {
                    return false;
                }
                if option_filter_misses(
                    opts.session_id.as_deref(),
                    relationship.session_id.as_deref(),
                ) {
                    return false;
                }
                relationship.strength >= min_strength && relationship.confidence >= min_confidence
            })
            .cloned()
            .collect();

        relationships.sort_by(|left, right| {
            right
                .strength
                .total_cmp(&left.strength)
                .then_with(|| right.updated_at.cmp(&left.updated_at))
                .then_with(|| right.id.cmp(&left.id))
        });
        relationships.truncate(opts.limit.unwrap_or(20));
        relationships
    }

    pub fn forget_agent_relationship(&mut self, id: &str) {
        self.agent_relationships.remove(id);
    }

    pub fn forget(&mut self, id: &str) {
        self.memories.remove(id);
        self.index.remove_document(id);
    }

    pub fn clear(&mut self, agent_id: Option<&str>) {
        match agent_id {
            None => {
                self.memories.clear();
                self.memory_entities.clear();
                self.agent_relationships.clear();
                self.index.clear();
            }
            Some(agent_id) => {
                let ids_to_remove: Vec<_> = self
                    .memories
                    .iter()
                    .filter(|(_, memory)| memory.agent_id == agent_id)
                    .map(|(id, _)| id.clone())
                    .collect();

                for id in ids_to_remove {
                    self.memories.remove(&id);
                    self.index.remove_document(&id);
                }

                self.agent_relationships.retain(|_, relationship| {
                    (relationship.source_kind != RelationshipEndpointKind::Agent
                        || relationship.source_agent_id != agent_id)
                        && (relationship.target_kind != RelationshipEndpointKind::Agent
                            || relationship.target_agent_id != agent_id)
                });
                self.memory_entities.retain(|_, entity| {
                    entity.kind != RelationshipEndpointKind::Agent || entity.id != agent_id
                });
            }
        }
    }

    pub fn save(&self) -> io::Result<()> {
        let Some(path) = &self.storage_file else {
            return Ok(());
        };

        let mut memories: Vec<_> = self.memories.values().cloned().collect();
        memories.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });

        let mut relationships: Vec<_> = self.agent_relationships.values().cloned().collect();
        relationships.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });

        let mut entities: Vec<_> = self.memory_entities.values().cloned().collect();
        entities.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.kind.as_str().cmp(right.kind.as_str()))
                .then_with(|| left.id.cmp(&right.id))
        });

        save_memory_store(path, &memories, &entities, &relationships)
    }

    pub fn load(&mut self) -> io::Result<()> {
        let Some(path) = &self.storage_file else {
            return Ok(());
        };

        let Some(store) = load_memory_store(path)? else {
            return Ok(());
        };

        for memory in store.memories {
            self.memories.insert(memory.id.clone(), memory.clone());
            self.index
                .add_document(memory.id.clone(), build_index_text(&memory));
            self.ensure_entity_from_parts(
                RelationshipEndpointKind::Agent,
                &memory.agent_id,
                &memory.agent_name,
            );
        }
        for entity in store.memory_entities {
            self.memory_entities
                .insert(entity_key(entity.kind, &entity.id), entity);
        }
        for relationship in store.agent_relationships {
            self.ensure_entity_from_parts(
                relationship.source_kind,
                &relationship.source_agent_id,
                &relationship.source_agent_name,
            );
            self.ensure_entity_from_parts(
                relationship.target_kind,
                &relationship.target_agent_id,
                &relationship.target_agent_name,
            );
            self.agent_relationships
                .insert(relationship.id.clone(), relationship);
        }

        Ok(())
    }

    pub fn size(&self) -> usize {
        self.memories.len()
    }

    pub fn relationship_count(&self) -> usize {
        self.agent_relationships.len()
    }

    pub fn entity_count(&self) -> usize {
        self.memory_entities.len()
    }

    pub fn summary(&self) -> String {
        format!("{} memories", self.memories.len())
    }

    fn upsert_entity_parts(
        &mut self,
        kind: RelationshipEndpointKind,
        id: String,
        name: String,
        aliases: Vec<String>,
        summary: Option<String>,
    ) -> MemoryEntity {
        let key = entity_key(kind, &id);
        let aliases = unique_strings(aliases);
        let summary = normalize_optional_string(summary);
        let now = now_millis();

        if let Some(existing) = self.memory_entities.get_mut(&key) {
            existing.name = name;
            merge_unique(&mut existing.aliases, aliases);
            existing.summary = summary.or_else(|| existing.summary.clone());
            existing.updated_at = now;
            return existing.clone();
        }

        let entity = MemoryEntity {
            kind,
            id,
            name,
            aliases,
            summary,
            created_at: now,
            updated_at: now,
        };
        self.memory_entities.insert(key, entity.clone());
        entity
    }

    fn ensure_entity_from_parts(&mut self, kind: RelationshipEndpointKind, id: &str, name: &str) {
        let id = id.trim();
        let name = name.trim();
        if id.is_empty() || name.is_empty() {
            return;
        }

        let key = entity_key(kind, id);
        if self.memory_entities.contains_key(&key) {
            return;
        }

        let now = now_millis();
        self.memory_entities.insert(
            key,
            MemoryEntity {
                kind,
                id: id.to_string(),
                name: name.to_string(),
                aliases: Vec::new(),
                summary: None,
                created_at: now,
                updated_at: now,
            },
        );
    }

    fn find_duplicate_memory(&self, memory: &NewMemory, scope: MemoryScope) -> Option<&Memory> {
        let normalized_content = normalize_text_for_match(&memory.content);
        self.memories.values().find(|existing| {
            existing.agent_id == memory.agent_id
                && existing.memory_type == memory.memory_type
                && existing.scope == scope
                && existing.room_id == memory.room_id
                && existing.world_id == memory.world_id
                && existing.session_id == memory.session_id
                && normalize_text_for_match(&existing.content) == normalized_content
        })
    }
}

fn build_index_text(memory: &Memory) -> String {
    let mut parts = vec![
        memory.content.clone(),
        memory.memory_type.as_str().to_string(),
        memory.scope.as_str().to_string(),
        memory.agent_name.clone(),
    ];
    parts.extend(memory.room_id.iter().cloned());
    parts.extend(memory.world_id.iter().cloned());
    parts.extend(memory.session_id.iter().cloned());
    if let Some(tags) = &memory.tags {
        parts.extend(tags.iter().cloned());
    }
    parts.join(" ")
}

#[derive(Clone)]
struct RecallCandidate {
    memory: Memory,
    lexical_score: f64,
    vector_score: f64,
    relationship_score: f64,
    recency_score: f64,
}

impl RecallCandidate {
    fn new(memory: &Memory) -> Self {
        Self {
            memory: memory.clone(),
            lexical_score: 0.0,
            vector_score: 0.0,
            relationship_score: 0.0,
            recency_score: 0.0,
        }
    }

    fn into_result(self, max_lexical_score: f64) -> MemoryRecallResult {
        let lexical_score = if max_lexical_score > 0.0 {
            self.lexical_score / max_lexical_score
        } else {
            0.0
        };
        let vector_score = self.vector_score.clamp(0.0, 1.0);
        let relationship_score = self.relationship_score.clamp(0.0, 1.0);
        let recency_score = self.recency_score.clamp(0.0, 1.0);
        let importance_score = self.memory.importance.clamp(0.0, 1.0);
        let score = (lexical_score * 0.40)
            + (vector_score * 0.25)
            + (relationship_score * 0.20)
            + (recency_score * 0.10)
            + (importance_score * 0.05);

        MemoryRecallResult {
            memory: self.memory,
            score,
            lexical_score,
            vector_score,
            relationship_score,
            recency_score,
            importance_score,
        }
    }
}

fn recall_candidate<'a>(
    candidates: &'a mut HashMap<String, RecallCandidate>,
    memory: &Memory,
) -> &'a mut RecallCandidate {
    candidates
        .entry(memory.id.clone())
        .or_insert_with(|| RecallCandidate::new(memory))
}

fn recent_options_from_search(search: &MemorySearchOptions, limit: usize) -> RecentMemoryOptions {
    RecentMemoryOptions {
        agent_id: search.agent_id.clone(),
        agent_name: search.agent_name.clone(),
        scope: search.scope,
        room_id: search.room_id.clone(),
        world_id: search.world_id.clone(),
        session_id: search.session_id.clone(),
        limit: Some(limit),
    }
}

fn memory_matches_search_options(memory: &Memory, opts: &MemorySearchOptions) -> bool {
    if opts
        .agent_id
        .as_deref()
        .is_some_and(|agent_id| memory.agent_id != agent_id)
    {
        return false;
    }
    if opts
        .agent_name
        .as_deref()
        .is_some_and(|agent_name| memory.agent_name != agent_name)
    {
        return false;
    }
    if opts
        .memory_type
        .is_some_and(|memory_type| memory.memory_type != memory_type)
    {
        return false;
    }
    if opts.scope.is_some_and(|scope| memory.scope != scope) {
        return false;
    }
    if option_filter_misses(opts.room_id.as_deref(), memory.room_id.as_deref()) {
        return false;
    }
    if option_filter_misses(opts.world_id.as_deref(), memory.world_id.as_deref()) {
        return false;
    }
    if option_filter_misses(opts.session_id.as_deref(), memory.session_id.as_deref()) {
        return false;
    }
    if opts
        .min_importance
        .is_some_and(|min_importance| memory.importance < min_importance)
    {
        return false;
    }
    true
}

fn memory_evaluation_score(memory: &NewMemory, importance: f64) -> f64 {
    let type_weight = match memory.memory_type {
        MemoryType::Fact | MemoryType::Reflection => 0.20,
        MemoryType::TaskResult => 0.15,
        MemoryType::Observation => 0.12,
    };
    let tag_weight = memory
        .tags
        .as_ref()
        .is_some_and(|tags| tags.iter().any(|tag| !tag.trim().is_empty()))
        .then_some(0.10)
        .unwrap_or(0.0);
    let context_weight = [
        memory.room_id.as_deref(),
        memory.world_id.as_deref(),
        memory.session_id.as_deref(),
    ]
    .iter()
    .any(|value| value.is_some_and(|value| !value.trim().is_empty()))
    .then_some(0.10)
    .unwrap_or(0.0);

    ((importance * 0.60) + type_weight + tag_weight + context_weight).min(1.0)
}

fn default_scope(room_id: &Option<String>) -> MemoryScope {
    if room_id.as_deref().is_some_and(|value| !value.is_empty()) {
        MemoryScope::Room
    } else {
        MemoryScope::Private
    }
}

fn option_filter_misses(expected: Option<&str>, actual: Option<&str>) -> bool {
    expected.is_some_and(|expected| actual != Some(expected))
}

fn same_agent_relationship_edge(
    relationship: &AgentRelationship,
    source_kind: RelationshipEndpointKind,
    source_agent_id: &str,
    target_kind: RelationshipEndpointKind,
    target_agent_id: &str,
    relationship_type: &str,
    world_id: Option<&str>,
) -> bool {
    relationship.source_kind == source_kind
        && relationship.source_agent_id == source_agent_id
        && relationship.target_kind == target_kind
        && relationship.target_agent_id == target_agent_id
        && relationship.relationship_type == relationship_type
        && relationship.world_id.as_deref() == world_id
}

fn normalize_required_string(value: String, error: MemoryError) -> Result<String, MemoryError> {
    let value = value.trim().to_string();
    if value.is_empty() {
        Err(error)
    } else {
        Ok(value)
    }
}

fn validate_unit_interval(value: f64, error: MemoryError) -> Result<f64, MemoryError> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(value)
    } else {
        Err(error)
    }
}

fn merge_optional_unique(existing: &mut Option<Vec<String>>, incoming: Option<Vec<String>>) {
    let Some(incoming) = incoming else {
        return;
    };
    let existing_values = existing.get_or_insert_with(Vec::new);
    merge_unique(existing_values, incoming);
    if existing_values.is_empty() {
        *existing = None;
    }
}

fn merge_unique(existing: &mut Vec<String>, incoming: Vec<String>) {
    for value in incoming {
        let value = value.trim().to_string();
        if !value.is_empty() && !existing.iter().any(|item| item == &value) {
            existing.push(value);
        }
    }
}

fn unique_strings(values: Vec<String>) -> Vec<String> {
    let mut unique = Vec::new();
    merge_unique(&mut unique, values);
    unique
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn normalize_text_for_match(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn entity_key(kind: RelationshipEndpointKind, id: &str) -> String {
    format!("{}:{id}", kind.as_str())
}

pub(super) fn validate_importance(importance: f64) -> Result<f64, MemoryError> {
    if importance.is_finite() && (0.0..=1.0).contains(&importance) {
        Ok(importance)
    } else {
        Err(MemoryError::InvalidImportance)
    }
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after unix epoch")
        .as_millis()
}

fn next_memory_id() -> String {
    let next = NEXT_MEMORY_ID.fetch_add(1, Ordering::Relaxed);
    format!("mem-{}-{next}", now_millis())
}

fn next_relationship_id() -> String {
    let next = NEXT_RELATIONSHIP_ID.fetch_add(1, Ordering::Relaxed);
    format!("rel-{}-{next}", now_millis())
}

fn memory_id_sequence(id: &str) -> u64 {
    id.rsplit('-')
        .next()
        .and_then(|suffix| suffix.parse::<u64>().ok())
        .unwrap_or(0)
}
