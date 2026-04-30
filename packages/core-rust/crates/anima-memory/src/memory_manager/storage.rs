use std::fs::{exists, read_to_string, write};
use std::io;
use std::path::Path;

use super::storage_json::{deserialize_memory_store, serialize_memory_store};
use super::{AgentRelationship, Memory, MemoryEntity};

pub(super) struct MemoryStore {
    pub(super) memories: Vec<Memory>,
    pub(super) memory_entities: Vec<MemoryEntity>,
    pub(super) agent_relationships: Vec<AgentRelationship>,
}

pub(super) fn save_memory_store(
    path: &Path,
    memories: &[Memory],
    memory_entities: &[MemoryEntity],
    agent_relationships: &[AgentRelationship],
) -> io::Result<()> {
    write(
        path,
        serialize_memory_store(memories, memory_entities, agent_relationships),
    )
}

pub(super) fn load_memory_store(path: &Path) -> io::Result<Option<MemoryStore>> {
    if !exists(path)? {
        return Ok(None);
    }

    let raw = read_to_string(path)?;
    Ok(deserialize_memory_store(&raw).ok())
}
