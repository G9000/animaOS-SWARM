#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryType {
    Fact,
    Observation,
    TaskResult,
    Reflection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryError {
    InvalidImportance,
}

impl MemoryError {
    pub const fn message(self) -> &'static str {
        match self {
            Self::InvalidImportance => "importance must be between 0 and 1",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct NewMemory {
    pub agent_id: String,
    pub agent_name: String,
    pub memory_type: MemoryType,
    pub content: String,
    pub importance: f64,
    pub tags: Option<Vec<String>>,
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
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemorySearchOptions {
    pub agent_id: Option<String>,
    pub agent_name: Option<String>,
    pub memory_type: Option<MemoryType>,
    pub limit: Option<usize>,
    pub min_importance: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RecentMemoryOptions {
    pub agent_id: Option<String>,
    pub agent_name: Option<String>,
    pub limit: Option<usize>,
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
        }
    }
}
