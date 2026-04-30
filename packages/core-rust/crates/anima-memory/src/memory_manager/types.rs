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

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemoryRecallOptions {
    pub search: MemorySearchOptions,
    pub entity_id: Option<String>,
    pub agent_id: Option<String>,
    pub limit: Option<usize>,
    pub lexical_limit: Option<usize>,
    pub recent_limit: Option<usize>,
    pub relationship_limit: Option<usize>,
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
    pub recency_score: f64,
    pub importance_score: f64,
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
