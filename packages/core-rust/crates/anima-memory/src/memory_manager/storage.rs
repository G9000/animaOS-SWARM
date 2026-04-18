use std::fs::{exists, read_to_string, write};
use std::io;
use std::path::Path;

use super::storage_json::{deserialize_memories, serialize_memories};
use super::Memory;

pub(super) fn save_memories(path: &Path, memories: &[Memory]) -> io::Result<()> {
    write(path, serialize_memories(memories))
}

pub(super) fn load_memories(path: &Path) -> io::Result<Option<Vec<Memory>>> {
    if !exists(path)? {
        return Ok(None);
    }

    let raw = read_to_string(path)?;
    Ok(deserialize_memories(&raw).ok())
}
