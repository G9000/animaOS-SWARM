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

use self::storage::{load_memories, save_memories};
pub use self::types::{
    Memory, MemoryError, MemorySearchOptions, MemorySearchResult, MemoryType, NewMemory,
    RecentMemoryOptions,
};
use crate::bm25::BM25;

static NEXT_MEMORY_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
pub struct MemoryManager {
    memories: HashMap<String, Memory>,
    index: BM25,
    storage_file: Option<PathBuf>,
}

impl MemoryManager {
    pub fn new() -> Self {
        Self {
            memories: HashMap::new(),
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
        };

        self.memories.insert(full.id.clone(), full.clone());
        self.index
            .add_document(full.id.clone(), build_index_text(&full));
        Ok(full)
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

    pub fn forget(&mut self, id: &str) {
        self.memories.remove(id);
        self.index.remove_document(id);
    }

    pub fn clear(&mut self, agent_id: Option<&str>) {
        match agent_id {
            None => {
                self.memories.clear();
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

        save_memories(path, &memories)
    }

    pub fn load(&mut self) -> io::Result<()> {
        let Some(path) = &self.storage_file else {
            return Ok(());
        };

        let Some(memories) = load_memories(path)? else {
            return Ok(());
        };

        for memory in memories {
            self.memories.insert(memory.id.clone(), memory.clone());
            self.index
                .add_document(memory.id.clone(), build_index_text(&memory));
        }

        Ok(())
    }

    pub fn size(&self) -> usize {
        self.memories.len()
    }

    pub fn summary(&self) -> String {
        format!("{} memories", self.memories.len())
    }
}

fn build_index_text(memory: &Memory) -> String {
    let mut parts = vec![
        memory.content.clone(),
        memory.memory_type.as_str().to_string(),
        memory.agent_name.clone(),
    ];
    if let Some(tags) = &memory.tags {
        parts.extend(tags.iter().cloned());
    }
    parts.join(" ")
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

fn memory_id_sequence(id: &str) -> u64 {
    id.rsplit('-')
        .next()
        .and_then(|suffix| suffix.parse::<u64>().ok())
        .unwrap_or(0)
}
