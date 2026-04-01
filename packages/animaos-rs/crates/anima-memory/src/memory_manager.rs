use std::collections::HashMap;
use std::fs::{exists, read_to_string, write};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::bm25::BM25;

static NEXT_MEMORY_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryType {
    Fact,
    Observation,
    TaskResult,
    Reflection,
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

    pub fn add(&mut self, memory: NewMemory) -> Memory {
        let full = Memory {
            id: next_memory_id(),
            agent_id: memory.agent_id,
            agent_name: memory.agent_name,
            memory_type: memory.memory_type,
            content: memory.content,
            importance: memory.importance,
            created_at: now_millis(),
            tags: memory.tags,
        };

        self.memories.insert(full.id.clone(), full.clone());
        self.index
            .add_document(full.id.clone(), build_index_text(&full));
        full
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

        memories.sort_by(|left, right| right.created_at.cmp(&left.created_at));
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

        write(path, serialize_memories(&memories))
    }

    pub fn load(&mut self) -> io::Result<()> {
        let Some(path) = &self.storage_file else {
            return Ok(());
        };
        if !exists(path)? {
            return Ok(());
        }

        let raw = read_to_string(path)?;
        let Ok(memories) = deserialize_memories(&raw) else {
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

impl MemoryType {
    fn as_str(self) -> &'static str {
        match self {
            Self::Fact => "fact",
            Self::Observation => "observation",
            Self::TaskResult => "task_result",
            Self::Reflection => "reflection",
        }
    }

    fn parse(value: &str) -> Result<Self, ()> {
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
    fn from_memory(memory: &Memory, score: f64) -> Self {
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

fn serialize_memories(memories: &[Memory]) -> String {
    let mut output = String::from("[\n");
    for (index, memory) in memories.iter().enumerate() {
        if index > 0 {
            output.push_str(",\n");
        }
        output.push_str("  {");
        push_string_field(&mut output, "id", &memory.id);
        output.push(',');
        push_string_field(&mut output, "agentId", &memory.agent_id);
        output.push(',');
        push_string_field(&mut output, "agentName", &memory.agent_name);
        output.push(',');
        push_string_field(&mut output, "type", memory.memory_type.as_str());
        output.push(',');
        push_string_field(&mut output, "content", &memory.content);
        output.push(',');
        output.push_str(&format!("\"importance\":{}", memory.importance));
        output.push(',');
        output.push_str(&format!("\"createdAt\":{}", memory.created_at));
        output.push(',');
        output.push_str("\"tags\":");
        match &memory.tags {
            None => output.push_str("null"),
            Some(tags) => {
                output.push('[');
                for (tag_index, tag) in tags.iter().enumerate() {
                    if tag_index > 0 {
                        output.push(',');
                    }
                    output.push('"');
                    output.push_str(&escape_json(tag));
                    output.push('"');
                }
                output.push(']');
            }
        }
        output.push('}');
    }
    output.push_str("\n]\n");
    output
}

fn push_string_field(output: &mut String, key: &str, value: &str) {
    output.push('"');
    output.push_str(key);
    output.push_str("\":\"");
    output.push_str(&escape_json(value));
    output.push('"');
}

fn escape_json(value: &str) -> String {
    let mut escaped = String::new();
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn deserialize_memories(input: &str) -> Result<Vec<Memory>, ()> {
    JsonParser::new(input).parse_memories()
}

struct JsonParser<'a> {
    input: &'a [u8],
    position: usize,
}

impl<'a> JsonParser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input: input.as_bytes(),
            position: 0,
        }
    }

    fn parse_memories(&mut self) -> Result<Vec<Memory>, ()> {
        self.skip_whitespace();
        self.expect(b'[')?;
        self.skip_whitespace();

        let mut memories = Vec::new();
        if self.consume(b']') {
            return Ok(memories);
        }

        loop {
            memories.push(self.parse_memory()?);
            self.skip_whitespace();
            if self.consume(b']') {
                break;
            }
            self.expect(b',')?;
            self.skip_whitespace();
        }

        self.skip_whitespace();
        if self.position != self.input.len() {
            return Err(());
        }
        Ok(memories)
    }

    fn parse_memory(&mut self) -> Result<Memory, ()> {
        self.skip_whitespace();
        self.expect(b'{')?;

        let mut id = None;
        let mut agent_id = None;
        let mut agent_name = None;
        let mut memory_type = None;
        let mut content = None;
        let mut importance = None;
        let mut created_at = None;
        let mut tags = None;

        loop {
            self.skip_whitespace();
            if self.consume(b'}') {
                break;
            }

            let key = self.parse_string()?;
            self.skip_whitespace();
            self.expect(b':')?;
            self.skip_whitespace();

            match key.as_str() {
                "id" => id = Some(self.parse_string()?),
                "agentId" => agent_id = Some(self.parse_string()?),
                "agentName" => agent_name = Some(self.parse_string()?),
                "type" => memory_type = Some(MemoryType::parse(&self.parse_string()?)?),
                "content" => content = Some(self.parse_string()?),
                "importance" => {
                    importance = Some(self.parse_number()?.parse::<f64>().map_err(|_| ())?)
                }
                "createdAt" => {
                    created_at = Some(self.parse_number()?.parse::<u128>().map_err(|_| ())?)
                }
                "tags" => tags = Some(self.parse_tags()?),
                _ => return Err(()),
            }

            self.skip_whitespace();
            if self.consume(b'}') {
                break;
            }
            self.expect(b',')?;
        }

        Ok(Memory {
            id: id.ok_or(())?,
            agent_id: agent_id.ok_or(())?,
            agent_name: agent_name.ok_or(())?,
            memory_type: memory_type.ok_or(())?,
            content: content.ok_or(())?,
            importance: importance.ok_or(())?,
            created_at: created_at.ok_or(())?,
            tags: tags.ok_or(())?,
        })
    }

    fn parse_tags(&mut self) -> Result<Option<Vec<String>>, ()> {
        if self.consume_literal("null") {
            return Ok(None);
        }

        self.expect(b'[')?;
        self.skip_whitespace();

        let mut tags = Vec::new();
        if self.consume(b']') {
            return Ok(Some(tags));
        }

        loop {
            tags.push(self.parse_string()?);
            self.skip_whitespace();
            if self.consume(b']') {
                break;
            }
            self.expect(b',')?;
            self.skip_whitespace();
        }

        Ok(Some(tags))
    }

    fn parse_string(&mut self) -> Result<String, ()> {
        self.expect(b'"')?;
        let mut output = Vec::new();

        while let Some(byte) = self.peek() {
            self.position += 1;
            match byte {
                b'"' => return String::from_utf8(output).map_err(|_| ()),
                b'\\' => {
                    let Some(escaped) = self.peek() else {
                        return Err(());
                    };
                    self.position += 1;
                    match escaped {
                        b'"' => output.push(b'"'),
                        b'\\' => output.push(b'\\'),
                        b'n' => output.push(b'\n'),
                        b'r' => output.push(b'\r'),
                        b't' => output.push(b'\t'),
                        b'u' => {
                            let codepoint = self.parse_unicode_escape()?;
                            let mut buffer = [0_u8; 4];
                            let encoded = codepoint.encode_utf8(&mut buffer);
                            output.extend_from_slice(encoded.as_bytes());
                        }
                        _ => return Err(()),
                    }
                }
                byte => output.push(byte),
            }
        }

        Err(())
    }

    fn parse_unicode_escape(&mut self) -> Result<char, ()> {
        let mut value = 0_u32;
        for _ in 0..4 {
            let Some(byte) = self.peek() else {
                return Err(());
            };
            self.position += 1;
            value = (value << 4)
                | match byte {
                    b'0'..=b'9' => u32::from(byte - b'0'),
                    b'a'..=b'f' => u32::from(byte - b'a' + 10),
                    b'A'..=b'F' => u32::from(byte - b'A' + 10),
                    _ => return Err(()),
                };
        }

        char::from_u32(value).ok_or(())
    }

    fn parse_number(&mut self) -> Result<String, ()> {
        let start = self.position;
        while let Some(byte) = self.peek() {
            if byte.is_ascii_digit() || matches!(byte, b'.' | b'-') {
                self.position += 1;
            } else {
                break;
            }
        }

        if start == self.position {
            return Err(());
        }

        std::str::from_utf8(&self.input[start..self.position])
            .map(|value| value.to_string())
            .map_err(|_| ())
    }

    fn consume_literal(&mut self, literal: &str) -> bool {
        let bytes = literal.as_bytes();
        if self.input[self.position..].starts_with(bytes) {
            self.position += bytes.len();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, expected: u8) -> Result<(), ()> {
        if self.consume(expected) {
            Ok(())
        } else {
            Err(())
        }
    }

    fn consume(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.position).copied()
    }

    fn skip_whitespace(&mut self) {
        while let Some(byte) = self.peek() {
            if byte.is_ascii_whitespace() {
                self.position += 1;
            } else {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{remove_file, write};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread::sleep;
    use std::time::Duration;

    use super::{MemoryManager, MemorySearchOptions, MemoryType, NewMemory, RecentMemoryOptions};

    static NEXT_TEMP_FILE_ID: AtomicU64 = AtomicU64::new(0);

    fn base(overrides: impl FnOnce(&mut NewMemory)) -> NewMemory {
        let mut memory = NewMemory {
            agent_id: "agent-1".into(),
            agent_name: "researcher".into(),
            memory_type: MemoryType::Fact,
            content: "TypeScript is a statically typed language".into(),
            importance: 0.5,
            tags: None,
        };
        overrides(&mut memory);
        memory
    }

    fn temp_path(label: &str) -> std::path::PathBuf {
        let suffix = NEXT_TEMP_FILE_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("anima-memory-{label}-{suffix}.json"))
    }

    #[test]
    fn add_assigns_unique_ids() {
        let mut manager = MemoryManager::new();
        let a = manager.add(base(|memory| memory.content = "fact one".into()));
        let b = manager.add(base(|memory| memory.content = "fact two".into()));

        assert!(!a.id.is_empty());
        assert!(!b.id.is_empty());
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn add_sets_created_at_to_now() {
        let mut manager = MemoryManager::new();
        let before = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock should be after unix epoch")
            .as_millis();
        let memory = manager.add(base(|_| {}));
        let after = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock should be after unix epoch")
            .as_millis();

        assert!(memory.created_at >= before);
        assert!(memory.created_at <= after);
    }

    #[test]
    fn add_preserves_provided_fields() {
        let mut manager = MemoryManager::new();
        let memory = manager.add(base(|memory| {
            memory.agent_id = "a99".into();
            memory.agent_name = "writer".into();
            memory.memory_type = MemoryType::TaskResult;
            memory.content = "Task was completed successfully".into();
            memory.importance = 0.9;
            memory.tags = Some(vec!["done".into(), "verified".into()]);
        }));

        assert_eq!(memory.agent_id, "a99");
        assert_eq!(memory.agent_name, "writer");
        assert_eq!(memory.memory_type, MemoryType::TaskResult);
        assert_eq!(memory.content, "Task was completed successfully");
        assert_eq!(memory.importance, 0.9);
        assert_eq!(
            memory.tags,
            Some(vec!["done".to_string(), "verified".to_string()])
        );
    }

    #[test]
    fn add_increments_size() {
        let mut manager = MemoryManager::new();
        assert_eq!(manager.size(), 0);

        manager.add(base(|_| {}));
        assert_eq!(manager.size(), 1);

        manager.add(base(|_| {}));
        assert_eq!(manager.size(), 2);
    }

    #[test]
    fn add_makes_memories_immediately_searchable() {
        let mut manager = MemoryManager::new();
        manager.add(base(|memory| {
            memory.content = "pglite is an in-process SQLite database".into();
        }));

        let results = manager.search("SQLite database", MemorySearchOptions::default());
        assert!(!results.is_empty());
        assert!(results[0].content.contains("pglite"));
    }

    fn seeded_manager() -> MemoryManager {
        let mut manager = MemoryManager::new();
        manager.add(base(|memory| {
            memory.agent_id = "a1".into();
            memory.agent_name = "researcher".into();
            memory.memory_type = MemoryType::Fact;
            memory.content = "TypeScript is a statically typed superset of JavaScript".into();
            memory.importance = 0.9;
        }));
        manager.add(base(|memory| {
            memory.agent_id = "a1".into();
            memory.agent_name = "researcher".into();
            memory.memory_type = MemoryType::Observation;
            memory.content = "React hooks simplify stateful component logic".into();
            memory.importance = 0.7;
        }));
        manager.add(base(|memory| {
            memory.agent_id = "a2".into();
            memory.agent_name = "writer".into();
            memory.memory_type = MemoryType::Fact;
            memory.content = "BM25 is a probabilistic ranking algorithm for text search".into();
            memory.importance = 0.8;
        }));
        manager.add(base(|memory| {
            memory.agent_id = "a2".into();
            memory.agent_name = "writer".into();
            memory.memory_type = MemoryType::TaskResult;
            memory.content = "Wrote API documentation covering 12 endpoints".into();
            memory.importance = 0.3;
        }));
        manager.add(base(|memory| {
            memory.agent_id = "a3".into();
            memory.agent_name = "reviewer".into();
            memory.memory_type = MemoryType::Reflection;
            memory.content = "Code review revealed three potential null pointer exceptions".into();
            memory.importance = 0.6;
        }));
        manager
    }

    #[test]
    fn search_returns_relevant_results() {
        let manager = seeded_manager();
        let results = manager.search(
            "TypeScript JavaScript typed",
            MemorySearchOptions::default(),
        );

        assert!(!results.is_empty());
        assert!(results[0].content.contains("TypeScript"));
    }

    #[test]
    fn search_attaches_positive_scores() {
        let manager = seeded_manager();
        let results = manager.search("TypeScript", MemorySearchOptions::default());

        assert!(!results.is_empty());
        assert!(results.iter().all(|result| result.score > 0.0));
    }

    #[test]
    fn search_ranks_more_relevant_results_higher() {
        let manager = seeded_manager();
        let results = manager.search(
            "BM25 ranking algorithm text search",
            MemorySearchOptions::default(),
        );

        assert!(results[0].content.contains("BM25"));
    }

    #[test]
    fn search_returns_empty_when_nothing_matches() {
        let manager = seeded_manager();
        let results = manager.search(
            "quantum entanglement neutron stars",
            MemorySearchOptions::default(),
        );

        assert!(results.is_empty());
    }

    #[test]
    fn search_returns_empty_for_blank_queries() {
        let manager = seeded_manager();
        let results = manager.search("", MemorySearchOptions::default());

        assert!(results.is_empty());
    }

    #[test]
    fn search_filters_by_agent_id() {
        let manager = seeded_manager();
        let results = manager.search(
            "code review documentation",
            MemorySearchOptions {
                agent_id: Some("a2".into()),
                ..MemorySearchOptions::default()
            },
        );

        assert!(!results.is_empty());
        assert!(results.iter().all(|result| result.agent_id == "a2"));
    }

    #[test]
    fn search_returns_nothing_for_unknown_agent_id() {
        let manager = seeded_manager();
        let results = manager.search(
            "TypeScript",
            MemorySearchOptions {
                agent_id: Some("nonexistent".into()),
                ..MemorySearchOptions::default()
            },
        );

        assert!(results.is_empty());
    }

    #[test]
    fn search_filters_by_agent_name() {
        let manager = seeded_manager();
        let results = manager.search(
            "TypeScript React hooks",
            MemorySearchOptions {
                agent_name: Some("researcher".into()),
                ..MemorySearchOptions::default()
            },
        );

        assert!(!results.is_empty());
        assert!(results
            .iter()
            .all(|result| result.agent_name == "researcher"));
    }

    #[test]
    fn search_filters_by_memory_type() {
        let manager = seeded_manager();
        let results = manager.search(
            "code endpoints documentation",
            MemorySearchOptions {
                memory_type: Some(MemoryType::TaskResult),
                ..MemorySearchOptions::default()
            },
        );

        assert!(!results.is_empty());
        assert!(results
            .iter()
            .all(|result| result.memory_type == MemoryType::TaskResult));
    }

    #[test]
    fn search_filters_by_min_importance() {
        let manager = seeded_manager();
        let results = manager.search(
            "code review documentation TypeScript",
            MemorySearchOptions {
                min_importance: Some(0.5),
                ..MemorySearchOptions::default()
            },
        );

        assert!(!results.is_empty());
        assert!(results.iter().all(|result| result.importance >= 0.5));
    }

    #[test]
    fn search_includes_low_importance_when_threshold_is_zero() {
        let manager = seeded_manager();
        let results = manager.search(
            "documentation",
            MemorySearchOptions {
                min_importance: Some(0.0),
                ..MemorySearchOptions::default()
            },
        );

        assert!(results.iter().any(|result| result.importance < 0.5));
    }

    #[test]
    fn search_respects_limit() {
        let manager = seeded_manager();
        let results = manager.search(
            "code",
            MemorySearchOptions {
                limit: Some(2),
                ..MemorySearchOptions::default()
            },
        );

        assert!(results.len() <= 2);
    }

    #[test]
    fn search_combines_filters() {
        let manager = seeded_manager();
        let results = manager.search(
            "BM25 algorithm",
            MemorySearchOptions {
                agent_name: Some("writer".into()),
                memory_type: Some(MemoryType::Fact),
                min_importance: Some(0.5),
                limit: Some(5),
                ..MemorySearchOptions::default()
            },
        );

        assert!(results.iter().all(|result| result.agent_name == "writer"));
        assert!(results
            .iter()
            .all(|result| result.memory_type == MemoryType::Fact));
        assert!(results.iter().all(|result| result.importance >= 0.5));
    }

    #[test]
    fn get_recent_returns_newest_first() {
        let mut manager = MemoryManager::new();
        manager.add(base(|memory| memory.content = "oldest".into()));
        sleep(Duration::from_millis(10));
        manager.add(base(|memory| memory.content = "middle".into()));
        sleep(Duration::from_millis(10));
        manager.add(base(|memory| memory.content = "newest".into()));

        let recent = manager.get_recent(RecentMemoryOptions::default());
        assert_eq!(recent[0].content, "newest");
        assert_eq!(recent[1].content, "middle");
        assert_eq!(recent[2].content, "oldest");
    }

    #[test]
    fn get_recent_respects_limit() {
        let mut manager = MemoryManager::new();
        manager.add(base(|memory| memory.content = "a".into()));
        manager.add(base(|memory| memory.content = "b".into()));
        manager.add(base(|memory| memory.content = "c".into()));
        manager.add(base(|memory| memory.content = "d".into()));

        let recent = manager.get_recent(RecentMemoryOptions {
            limit: Some(2),
            ..RecentMemoryOptions::default()
        });

        assert_eq!(recent.len(), 2);
    }

    #[test]
    fn get_recent_filters_by_agent_id() {
        let mut manager = MemoryManager::new();
        manager.add(base(|memory| {
            memory.agent_id = "a1".into();
            memory.agent_name = "agent-a".into();
            memory.content = "a1 memory".into();
        }));
        manager.add(base(|memory| {
            memory.agent_id = "a2".into();
            memory.agent_name = "agent-b".into();
            memory.content = "a2 memory".into();
        }));
        manager.add(base(|memory| {
            memory.agent_id = "a1".into();
            memory.agent_name = "agent-a".into();
            memory.content = "a1 again".into();
        }));

        let recent = manager.get_recent(RecentMemoryOptions {
            agent_id: Some("a1".into()),
            ..RecentMemoryOptions::default()
        });

        assert_eq!(recent.len(), 2);
        assert!(recent.iter().all(|result| result.agent_id == "a1"));
    }

    #[test]
    fn get_recent_filters_by_agent_name() {
        let mut manager = MemoryManager::new();
        manager.add(base(|memory| {
            memory.agent_name = "researcher".into();
            memory.content = "research memory".into();
        }));
        manager.add(base(|memory| {
            memory.agent_name = "writer".into();
            memory.content = "writing memory".into();
        }));
        manager.add(base(|memory| {
            memory.agent_name = "researcher".into();
            memory.content = "more research".into();
        }));

        let recent = manager.get_recent(RecentMemoryOptions {
            agent_name: Some("researcher".into()),
            ..RecentMemoryOptions::default()
        });

        assert_eq!(recent.len(), 2);
        assert!(recent
            .iter()
            .all(|result| result.agent_name == "researcher"));
    }

    #[test]
    fn get_recent_returns_empty_when_no_memories_exist() {
        assert!(MemoryManager::new()
            .get_recent(RecentMemoryOptions::default())
            .is_empty());
    }

    #[test]
    fn forget_removes_memory_from_store() {
        let mut manager = MemoryManager::new();
        let memory = manager.add(base(|memory| memory.content = "temporary fact".into()));

        assert_eq!(manager.size(), 1);
        manager.forget(&memory.id);
        assert_eq!(manager.size(), 0);
    }

    #[test]
    fn forget_removes_memory_from_search_index() {
        let mut manager = MemoryManager::new();
        let memory = manager.add(base(|memory| {
            memory.content = "pglite is an in-process database".into();
        }));

        manager.forget(&memory.id);
        let results = manager.search("pglite in-process database", MemorySearchOptions::default());
        assert!(results.is_empty());
    }

    #[test]
    fn forget_leaves_other_memories_intact() {
        let mut manager = MemoryManager::new();
        let a = manager.add(base(|memory| {
            memory.content = "memory A about TypeScript".into()
        }));
        manager.add(base(|memory| {
            memory.content = "memory B about React".into()
        }));
        manager.forget(&a.id);

        assert_eq!(manager.size(), 1);
        let results = manager.search("React", MemorySearchOptions::default());
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("React"));
    }

    #[test]
    fn forget_is_a_noop_for_unknown_id() {
        let mut manager = MemoryManager::new();
        manager.add(base(|_| {}));

        manager.forget("non-existent-id");
        assert_eq!(manager.size(), 1);
    }

    #[test]
    fn clear_without_agent_id_clears_everything() {
        let mut manager = MemoryManager::new();
        manager.add(base(|memory| {
            memory.agent_id = "a1".into();
            memory.agent_name = "agent-a".into();
            memory.content = "agent A fact 1".into();
        }));
        manager.add(base(|memory| {
            memory.agent_id = "a1".into();
            memory.agent_name = "agent-a".into();
            memory.content = "agent A fact 2".into();
        }));
        manager.add(base(|memory| {
            memory.agent_id = "a2".into();
            memory.agent_name = "agent-b".into();
            memory.content = "agent B fact".into();
        }));

        manager.clear(None);
        assert_eq!(manager.size(), 0);
        assert!(manager
            .search("fact", MemorySearchOptions::default())
            .is_empty());
    }

    #[test]
    fn clear_with_agent_id_only_clears_that_agent() {
        let mut manager = MemoryManager::new();
        manager.add(base(|memory| {
            memory.agent_id = "a1".into();
            memory.agent_name = "agent-a".into();
            memory.content = "agent A fact 1".into();
        }));
        manager.add(base(|memory| {
            memory.agent_id = "a1".into();
            memory.agent_name = "agent-a".into();
            memory.content = "agent A fact 2".into();
        }));
        manager.add(base(|memory| {
            memory.agent_id = "a2".into();
            memory.agent_name = "agent-b".into();
            memory.content = "agent B fact".into();
        }));

        manager.clear(Some("a1"));
        assert_eq!(manager.size(), 1);
        assert_eq!(
            manager.get_recent(RecentMemoryOptions::default())[0].agent_id,
            "a2"
        );
    }

    #[test]
    fn clear_removes_cleared_memories_from_search_index() {
        let mut manager = MemoryManager::new();
        manager.add(base(|memory| {
            memory.agent_id = "a1".into();
            memory.agent_name = "agent-a".into();
            memory.content = "agent A fact 1".into();
        }));
        manager.add(base(|memory| {
            memory.agent_id = "a1".into();
            memory.agent_name = "agent-a".into();
            memory.content = "agent A fact 2".into();
        }));
        manager.add(base(|memory| {
            memory.agent_id = "a2".into();
            memory.agent_name = "agent-b".into();
            memory.content = "agent B fact".into();
        }));

        manager.clear(Some("a1"));
        let results = manager.search("agent B fact", MemorySearchOptions::default());
        assert!(!results.is_empty());
        assert!(results.iter().all(|result| result.agent_id == "a2"));
    }

    #[test]
    fn save_writes_memories_to_json_file() {
        let path = temp_path("save");
        let _ = remove_file(&path);

        let mut manager = MemoryManager::with_storage_file(path.clone());
        manager.add(base(|memory| memory.content = "saved fact".into()));
        manager.save().expect("save should succeed");

        let contents = std::fs::read_to_string(&path).expect("saved file should be readable");
        assert!(contents.contains("saved fact"));
        let _ = remove_file(&path);
    }

    #[test]
    fn load_restores_memories_from_json_file() {
        let path = temp_path("load");
        let _ = remove_file(&path);

        let mut manager = MemoryManager::with_storage_file(path.clone());
        manager.add(base(|memory| memory.content = "persisted memory".into()));
        manager.add(base(|memory| {
            memory.content = "another persisted memory".into();
            memory.agent_name = "writer".into();
        }));
        manager.save().expect("save should succeed");

        let mut reloaded = MemoryManager::with_storage_file(path);
        reloaded.load().expect("load should succeed");

        assert_eq!(reloaded.size(), 2);
        let _ = remove_file(reloaded.storage_file.as_ref().expect("path should exist"));
    }

    #[test]
    fn load_restores_search_index() {
        let path = temp_path("index");
        let _ = remove_file(&path);

        let mut manager = MemoryManager::with_storage_file(path.clone());
        manager.add(base(|memory| {
            memory.content = "Nx is a build system for monorepos".into();
        }));
        manager.save().expect("save should succeed");

        let mut reloaded = MemoryManager::with_storage_file(path);
        reloaded.load().expect("load should succeed");

        let results = reloaded.search("Nx monorepo build", MemorySearchOptions::default());
        assert!(!results.is_empty());
        assert!(results[0].content.contains("Nx"));
        let _ = remove_file(reloaded.storage_file.as_ref().expect("path should exist"));
    }

    #[test]
    fn load_preserves_id_and_created_at() {
        let path = temp_path("preserve");
        let _ = remove_file(&path);

        let mut manager = MemoryManager::with_storage_file(path.clone());
        let original = manager.add(base(|memory| memory.content = "to be preserved".into()));
        manager.save().expect("save should succeed");

        let mut reloaded = MemoryManager::with_storage_file(path);
        reloaded.load().expect("load should succeed");

        let restored = reloaded.get_recent(RecentMemoryOptions::default())[0].clone();
        assert_eq!(restored.id, original.id);
        assert_eq!(restored.created_at, original.created_at);
        assert_eq!(restored.content, original.content);
        let _ = remove_file(reloaded.storage_file.as_ref().expect("path should exist"));
    }

    #[test]
    fn load_is_a_noop_when_file_does_not_exist() {
        let path = temp_path("missing");
        let _ = remove_file(&path);

        let mut manager = MemoryManager::with_storage_file(path);
        manager
            .load()
            .expect("load should not fail for missing file");
        assert_eq!(manager.size(), 0);
    }

    #[test]
    fn load_is_a_noop_without_storage_file() {
        let mut manager = MemoryManager::new();
        manager
            .load()
            .expect("load should not fail without a configured file");
        assert_eq!(manager.size(), 0);
    }

    #[test]
    fn save_is_a_noop_without_storage_file() {
        let mut manager = MemoryManager::new();
        manager.add(base(|_| {}));
        manager
            .save()
            .expect("save should not fail without a configured file");
    }

    #[test]
    fn load_recovers_from_corrupted_file() {
        let path = temp_path("corrupted");
        let _ = remove_file(&path);
        write(&path, "{ this is not valid JSON }").expect("corrupted file should be written");

        let mut manager = MemoryManager::with_storage_file(path.clone());
        manager
            .load()
            .expect("load should not fail for corrupted JSON");
        assert_eq!(manager.size(), 0);
        let _ = remove_file(&path);
    }

    #[test]
    fn load_is_idempotent() {
        let path = temp_path("idempotent");
        let _ = remove_file(&path);

        let mut manager = MemoryManager::with_storage_file(path.clone());
        manager.add(base(|memory| memory.content = "unique memory".into()));
        manager.save().expect("save should succeed");

        let mut reloaded = MemoryManager::with_storage_file(path);
        reloaded.load().expect("first load should succeed");
        reloaded.load().expect("second load should succeed");

        assert_eq!(reloaded.size(), 1);
        let _ = remove_file(reloaded.storage_file.as_ref().expect("path should exist"));
    }

    #[test]
    fn save_can_be_called_multiple_times() {
        let path = temp_path("save-many");
        let _ = remove_file(&path);

        let mut manager = MemoryManager::with_storage_file(path.clone());
        manager.add(base(|memory| memory.content = "fact one".into()));
        manager.save().expect("first save should succeed");
        manager.add(base(|memory| memory.content = "fact two".into()));
        manager.save().expect("second save should succeed");

        let mut reloaded = MemoryManager::with_storage_file(path);
        reloaded.load().expect("load should succeed");
        assert_eq!(reloaded.size(), 2);
        let _ = remove_file(reloaded.storage_file.as_ref().expect("path should exist"));
    }

    #[test]
    fn load_preserves_unicode_content_and_tags() {
        let path = temp_path("unicode");
        let _ = remove_file(&path);

        let mut manager = MemoryManager::with_storage_file(path.clone());
        let original = manager.add(base(|memory| {
            memory.agent_name = "分析者".into();
            memory.content = "Café 猫 🚀".into();
            memory.tags = Some(vec!["naïve".into(), "测试".into()]);
        }));
        manager.save().expect("save should succeed");

        let mut reloaded = MemoryManager::with_storage_file(path);
        reloaded.load().expect("load should succeed");

        let restored = reloaded.get_recent(RecentMemoryOptions::default())[0].clone();
        assert_eq!(restored.agent_name, original.agent_name);
        assert_eq!(restored.content, original.content);
        assert_eq!(restored.tags, original.tags);
        let _ = remove_file(reloaded.storage_file.as_ref().expect("path should exist"));
    }

    #[test]
    fn summary_reflects_current_count_and_keeps_plural_bug_for_one() {
        let mut manager = MemoryManager::new();
        assert_eq!(manager.summary(), "0 memories");
        assert_ne!(manager.summary(), "1 memory");

        manager.add(base(|_| {}));
        assert_eq!(manager.summary(), "1 memories");

        manager.add(base(|_| {}));
        assert_eq!(manager.summary(), "2 memories");
    }

    #[test]
    fn size_is_zero_for_fresh_instance() {
        assert_eq!(MemoryManager::new().size(), 0);
    }
}
