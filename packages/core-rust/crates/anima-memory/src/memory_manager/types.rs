#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryType {
    Fact,
    Observation,
    TaskResult,
    Reflection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryScope {
    Shared,
    Private,
    Room,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryError {
    InvalidImportance,
    InvalidEntityId,
    InvalidEntityName,
    InvalidRelationshipEndpointKind,
    InvalidRelationshipEndpoint,
    InvalidRelationshipEndpointName,
    InvalidRelationshipType,
    InvalidRelationshipStrength,
    InvalidRelationshipConfidence,
    InvalidTemporalSubject,
    InvalidTemporalSubjectName,
    InvalidTemporalPredicate,
    InvalidTemporalObject,
    InvalidTemporalObjectName,
    InvalidTemporalRelationshipType,
    InvalidTemporalStrength,
    InvalidTemporalConfidence,
    InvalidTemporalValidityRange,
}

impl MemoryError {
    pub const fn message(self) -> &'static str {
        match self {
            Self::InvalidImportance => "importance must be between 0 and 1",
            Self::InvalidEntityId => "entity ID must not be empty",
            Self::InvalidEntityName => "entity name must not be empty",
            Self::InvalidRelationshipEndpointKind => {
                "relationship endpoint kind must be one of agent, user, system, external"
            }
            Self::InvalidRelationshipEndpoint => "relationship endpoint IDs must not be empty",
            Self::InvalidRelationshipEndpointName => {
                "relationship endpoint names must not be empty"
            }
            Self::InvalidRelationshipType => "relationshipType must not be empty",
            Self::InvalidRelationshipStrength => "strength must be between 0 and 1",
            Self::InvalidRelationshipConfidence => "confidence must be between 0 and 1",
            Self::InvalidTemporalSubject => "temporal subject ID must not be empty",
            Self::InvalidTemporalSubjectName => "temporal subject name must not be empty",
            Self::InvalidTemporalPredicate => "temporal predicate must not be empty",
            Self::InvalidTemporalObject => "temporal fact must have an object endpoint or value",
            Self::InvalidTemporalObjectName => "temporal object name must not be empty",
            Self::InvalidTemporalRelationshipType => "temporal relationship type must not be empty",
            Self::InvalidTemporalStrength => "temporal strength must be between 0 and 1",
            Self::InvalidTemporalConfidence => "temporal confidence must be between 0 and 1",
            Self::InvalidTemporalValidityRange => {
                "validTo must be greater than or equal to validFrom"
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct NewMemoryEntity {
    pub kind: RelationshipEndpointKind,
    pub id: String,
    pub name: String,
    pub aliases: Vec<String>,
    pub summary: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MemoryEntity {
    pub kind: RelationshipEndpointKind,
    pub id: String,
    pub name: String,
    pub aliases: Vec<String>,
    pub summary: Option<String>,
    pub created_at: u128,
    pub updated_at: u128,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemoryEntityOptions {
    pub entity_id: Option<String>,
    pub kind: Option<RelationshipEndpointKind>,
    pub name: Option<String>,
    pub alias: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RelationshipEndpointKind {
    #[default]
    Agent,
    User,
    System,
    External,
}

impl RelationshipEndpointKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::User => "user",
            Self::System => "system",
            Self::External => "external",
        }
    }

    pub fn from_str(value: &str) -> Result<Self, MemoryError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "agent" => Ok(Self::Agent),
            "user" => Ok(Self::User),
            "system" => Ok(Self::System),
            "external" => Ok(Self::External),
            _ => Err(MemoryError::InvalidRelationshipEndpointKind),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct NewAgentRelationship {
    pub source_kind: Option<RelationshipEndpointKind>,
    pub source_agent_id: String,
    pub source_agent_name: String,
    pub target_kind: Option<RelationshipEndpointKind>,
    pub target_agent_id: String,
    pub target_agent_name: String,
    pub relationship_type: String,
    pub summary: Option<String>,
    pub strength: f64,
    pub confidence: f64,
    pub evidence_memory_ids: Vec<String>,
    pub tags: Option<Vec<String>>,
    pub room_id: Option<String>,
    pub world_id: Option<String>,
    pub session_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentRelationship {
    pub id: String,
    pub source_kind: RelationshipEndpointKind,
    pub source_agent_id: String,
    pub source_agent_name: String,
    pub target_kind: RelationshipEndpointKind,
    pub target_agent_id: String,
    pub target_agent_name: String,
    pub relationship_type: String,
    pub summary: Option<String>,
    pub strength: f64,
    pub confidence: f64,
    pub evidence_memory_ids: Vec<String>,
    pub tags: Option<Vec<String>>,
    pub room_id: Option<String>,
    pub world_id: Option<String>,
    pub session_id: Option<String>,
    pub created_at: u128,
    pub updated_at: u128,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentRelationshipOptions {
    pub entity_id: Option<String>,
    pub agent_id: Option<String>,
    pub source_kind: Option<RelationshipEndpointKind>,
    pub source_agent_id: Option<String>,
    pub target_kind: Option<RelationshipEndpointKind>,
    pub target_agent_id: Option<String>,
    pub relationship_type: Option<String>,
    pub room_id: Option<String>,
    pub world_id: Option<String>,
    pub session_id: Option<String>,
    pub min_strength: Option<f64>,
    pub min_confidence: Option<f64>,
    pub limit: Option<usize>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TemporalRecordStatus {
    #[default]
    Active,
    Superseded,
    Retracted,
}

impl TemporalRecordStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Superseded => "superseded",
            Self::Retracted => "retracted",
        }
    }

    pub fn parse(value: &str) -> Result<Self, ()> {
        match value {
            "active" => Ok(Self::Active),
            "superseded" => Ok(Self::Superseded),
            "retracted" => Ok(Self::Retracted),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct NewTemporalFact {
    pub subject_kind: RelationshipEndpointKind,
    pub subject_id: String,
    pub subject_name: String,
    pub predicate: String,
    pub object_kind: Option<RelationshipEndpointKind>,
    pub object_id: Option<String>,
    pub object_name: Option<String>,
    pub value: Option<String>,
    pub valid_from: Option<u128>,
    pub valid_to: Option<u128>,
    pub observed_at: Option<u128>,
    pub confidence: f64,
    pub evidence_memory_ids: Vec<String>,
    pub supersedes_fact_ids: Vec<String>,
    pub status: Option<TemporalRecordStatus>,
    pub tags: Option<Vec<String>>,
    pub room_id: Option<String>,
    pub world_id: Option<String>,
    pub session_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TemporalFact {
    pub id: String,
    pub subject_kind: RelationshipEndpointKind,
    pub subject_id: String,
    pub subject_name: String,
    pub predicate: String,
    pub object_kind: Option<RelationshipEndpointKind>,
    pub object_id: Option<String>,
    pub object_name: Option<String>,
    pub value: Option<String>,
    pub valid_from: Option<u128>,
    pub valid_to: Option<u128>,
    pub observed_at: u128,
    pub confidence: f64,
    pub evidence_memory_ids: Vec<String>,
    pub supersedes_fact_ids: Vec<String>,
    pub status: TemporalRecordStatus,
    pub tags: Option<Vec<String>>,
    pub room_id: Option<String>,
    pub world_id: Option<String>,
    pub session_id: Option<String>,
    pub created_at: u128,
    pub updated_at: u128,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TemporalFactOptions {
    pub subject_kind: Option<RelationshipEndpointKind>,
    pub subject_id: Option<String>,
    pub predicate: Option<String>,
    pub object_kind: Option<RelationshipEndpointKind>,
    pub object_id: Option<String>,
    pub status: Option<TemporalRecordStatus>,
    pub valid_at: Option<u128>,
    pub include_inactive: bool,
    pub room_id: Option<String>,
    pub world_id: Option<String>,
    pub session_id: Option<String>,
    pub min_confidence: Option<f64>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NewTemporalRelationship {
    pub source_kind: RelationshipEndpointKind,
    pub source_id: String,
    pub source_name: String,
    pub target_kind: RelationshipEndpointKind,
    pub target_id: String,
    pub target_name: String,
    pub relationship_type: String,
    pub summary: Option<String>,
    pub strength: f64,
    pub confidence: f64,
    pub valid_from: Option<u128>,
    pub valid_to: Option<u128>,
    pub observed_at: Option<u128>,
    pub evidence_memory_ids: Vec<String>,
    pub supersedes_relationship_ids: Vec<String>,
    pub status: Option<TemporalRecordStatus>,
    pub tags: Option<Vec<String>>,
    pub room_id: Option<String>,
    pub world_id: Option<String>,
    pub session_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TemporalRelationship {
    pub id: String,
    pub source_kind: RelationshipEndpointKind,
    pub source_id: String,
    pub source_name: String,
    pub target_kind: RelationshipEndpointKind,
    pub target_id: String,
    pub target_name: String,
    pub relationship_type: String,
    pub summary: Option<String>,
    pub strength: f64,
    pub confidence: f64,
    pub valid_from: Option<u128>,
    pub valid_to: Option<u128>,
    pub observed_at: u128,
    pub evidence_memory_ids: Vec<String>,
    pub supersedes_relationship_ids: Vec<String>,
    pub status: TemporalRecordStatus,
    pub tags: Option<Vec<String>>,
    pub room_id: Option<String>,
    pub world_id: Option<String>,
    pub session_id: Option<String>,
    pub created_at: u128,
    pub updated_at: u128,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TemporalRelationshipOptions {
    pub source_kind: Option<RelationshipEndpointKind>,
    pub source_id: Option<String>,
    pub target_kind: Option<RelationshipEndpointKind>,
    pub target_id: Option<String>,
    pub relationship_type: Option<String>,
    pub status: Option<TemporalRecordStatus>,
    pub valid_at: Option<u128>,
    pub include_inactive: bool,
    pub room_id: Option<String>,
    pub world_id: Option<String>,
    pub session_id: Option<String>,
    pub min_strength: Option<f64>,
    pub min_confidence: Option<f64>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NewMemory {
    pub agent_id: String,
    pub agent_name: String,
    pub memory_type: MemoryType,
    pub content: String,
    pub importance: f64,
    pub tags: Option<Vec<String>>,
    pub scope: Option<MemoryScope>,
    pub room_id: Option<String>,
    pub world_id: Option<String>,
    pub session_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Memory {
    pub id: String,
    pub agent_id: String,
    pub agent_name: String,
    pub memory_type: MemoryType,
    pub content: String,
    pub importance: f64,
    pub created_at: u128,
    pub tags: Option<Vec<String>>,
    pub scope: MemoryScope,
    pub room_id: Option<String>,
    pub world_id: Option<String>,
    pub session_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MemorySearchResult {
    pub id: String,
    pub agent_id: String,
    pub agent_name: String,
    pub memory_type: MemoryType,
    pub content: String,
    pub importance: f64,
    pub created_at: u128,
    pub tags: Option<Vec<String>>,
    pub score: f64,
    pub scope: MemoryScope,
    pub room_id: Option<String>,
    pub world_id: Option<String>,
    pub session_id: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemorySearchOptions {
    pub agent_id: Option<String>,
    pub agent_name: Option<String>,
    pub memory_type: Option<MemoryType>,
    pub scope: Option<MemoryScope>,
    pub room_id: Option<String>,
    pub world_id: Option<String>,
    pub session_id: Option<String>,
    pub limit: Option<usize>,
    pub min_importance: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RecentMemoryOptions {
    pub agent_id: Option<String>,
    pub agent_name: Option<String>,
    pub scope: Option<MemoryScope>,
    pub room_id: Option<String>,
    pub world_id: Option<String>,
    pub session_id: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryEvaluationDecision {
    Store,
    Merge,
    Ignore,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MemoryEvaluationOptions {
    pub min_content_chars: usize,
    pub min_importance: f64,
}

impl Default for MemoryEvaluationOptions {
    fn default() -> Self {
        Self {
            min_content_chars: 12,
            min_importance: 0.15,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MemoryEvaluation {
    pub decision: MemoryEvaluationDecision,
    pub reason: String,
    pub score: f64,
    pub suggested_importance: f64,
    pub duplicate_memory_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MemoryEvaluationOutcome {
    pub evaluation: MemoryEvaluation,
    pub memory: Option<Memory>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MemoryRecallWeights {
    pub lexical: f64,
    pub vector: f64,
    pub relationship: f64,
    pub temporal: f64,
    pub recency: f64,
    pub importance: f64,
}

impl Default for MemoryRecallWeights {
    fn default() -> Self {
        Self {
            lexical: 0.55,
            vector: 0.15,
            relationship: 0.20,
            temporal: 0.00,
            recency: 0.05,
            importance: 0.05,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemoryRecallOptions {
    pub search: MemorySearchOptions,
    pub entity_id: Option<String>,
    pub agent_id: Option<String>,
    pub limit: Option<usize>,
    pub lexical_limit: Option<usize>,
    pub recent_limit: Option<usize>,
    pub relationship_limit: Option<usize>,
    pub temporal_limit: Option<usize>,
    pub temporal_intent_terms: Vec<String>,
    pub weights: Option<MemoryRecallWeights>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VectorMemoryHit {
    pub memory_id: String,
    pub score: f64,
}

pub trait MemoryVectorIndex {
    fn search(&self, query: &str, limit: usize) -> Vec<VectorMemoryHit>;
}

#[derive(Clone, Debug, PartialEq)]
pub struct MemoryRecallResult {
    pub memory: Memory,
    pub score: f64,
    pub lexical_score: f64,
    pub vector_score: f64,
    pub relationship_score: f64,
    pub temporal_score: f64,
    pub recency_score: f64,
    pub importance_score: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MemoryEvidenceTrace {
    pub memory: Memory,
    pub relationships: Vec<AgentRelationship>,
    pub entities: Vec<MemoryEntity>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemoryRetentionPolicy {
    pub max_age_millis: Option<u128>,
    pub min_importance: Option<f64>,
    pub max_memories: Option<usize>,
    pub decay_half_life_millis: Option<u128>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MemoryImportanceAdjustment {
    pub memory_id: String,
    pub previous_importance: f64,
    pub new_importance: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemoryRetentionReport {
    pub decayed_memories: Vec<MemoryImportanceAdjustment>,
    pub removed_memory_ids: Vec<String>,
    pub removed_relationship_ids: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemoryManagerSnapshot {
    pub memories: Vec<Memory>,
    pub memory_entities: Vec<MemoryEntity>,
    pub agent_relationships: Vec<AgentRelationship>,
    pub temporal_facts: Vec<TemporalFact>,
    pub temporal_relationships: Vec<TemporalRelationship>,
}

impl MemoryType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fact => "fact",
            Self::Observation => "observation",
            Self::TaskResult => "task_result",
            Self::Reflection => "reflection",
        }
    }

    pub fn parse(value: &str) -> Result<Self, ()> {
        match value {
            "fact" => Ok(Self::Fact),
            "observation" => Ok(Self::Observation),
            "task_result" => Ok(Self::TaskResult),
            "reflection" => Ok(Self::Reflection),
            _ => Err(()),
        }
    }
}

impl MemoryScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Shared => "shared",
            Self::Private => "private",
            Self::Room => "room",
        }
    }

    pub fn parse(value: &str) -> Result<Self, ()> {
        match value {
            "shared" => Ok(Self::Shared),
            "private" => Ok(Self::Private),
            "room" => Ok(Self::Room),
            _ => Err(()),
        }
    }
}

impl MemorySearchResult {
    pub(super) fn from_memory(memory: &Memory, score: f64) -> Self {
        Self {
            id: memory.id.clone(),
            agent_id: memory.agent_id.clone(),
            agent_name: memory.agent_name.clone(),
            memory_type: memory.memory_type,
            content: memory.content.clone(),
            importance: memory.importance,
            created_at: memory.created_at,
            tags: memory.tags.clone(),
            score,
            scope: memory.scope,
            room_id: memory.room_id.clone(),
            world_id: memory.world_id.clone(),
            session_id: memory.session_id.clone(),
        }
    }
}
