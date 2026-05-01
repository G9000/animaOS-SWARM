use std::fs::{exists, read_to_string, write};
use std::io;
use std::path::Path;

use super::storage_json::{deserialize_memory_store, serialize_memory_store};
#[cfg(feature = "sqlite")]
use super::storage_sqlite::{load_sqlite_memory_store, save_sqlite_memory_store};
use super::{AgentRelationship, Memory, MemoryEntity};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MemoryStorageFormat {
    Json,
    #[cfg(feature = "sqlite")]
    Sqlite,
}

impl Default for MemoryStorageFormat {
    fn default() -> Self {
        Self::Json
    }
}

pub(super) struct MemoryStore {
    pub(super) memories: Vec<Memory>,
    pub(super) memory_entities: Vec<MemoryEntity>,
    pub(super) agent_relationships: Vec<AgentRelationship>,
}

pub(super) fn save_memory_store(
    format: MemoryStorageFormat,
    path: &Path,
    memories: &[Memory],
    memory_entities: &[MemoryEntity],
    agent_relationships: &[AgentRelationship],
) -> io::Result<()> {
    match format {
        MemoryStorageFormat::Json => write(
            path,
            serialize_memory_store(memories, memory_entities, agent_relationships),
        ),
        #[cfg(feature = "sqlite")]
        MemoryStorageFormat::Sqlite => {
            save_sqlite_memory_store(path, memories, memory_entities, agent_relationships)
        }
    }
}

pub(super) fn load_memory_store(
    format: MemoryStorageFormat,
    path: &Path,
) -> io::Result<Option<MemoryStore>> {
    if !exists(path)? {
        return Ok(None);
    }

    match format {
        MemoryStorageFormat::Json => {
            let raw = read_to_string(path)?;
            Ok(deserialize_memory_store(&raw).ok())
        }
        #[cfg(feature = "sqlite")]
        MemoryStorageFormat::Sqlite => load_sqlite_memory_store(path),
    }
}
