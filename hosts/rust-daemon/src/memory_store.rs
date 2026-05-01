use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anima_memory::{
    AgentRelationship, Memory, MemoryEntity, MemoryManager, MemoryManagerSnapshot, MemoryScope,
    MemoryType, RelationshipEndpointKind, TemporalFact, TemporalRecordStatus, TemporalRelationship,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

const STORE_VERSION: u32 = 1;
const SQLITE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS memory_store_snapshots (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    payload TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
"#;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MemoryStoreConfig {
    Json(PathBuf),
    Sqlite(PathBuf),
}

impl MemoryStoreConfig {
    pub(crate) fn path(&self) -> &PathBuf {
        match self {
            Self::Json(path) | Self::Sqlite(path) => path,
        }
    }

    pub(crate) const fn storage_label(&self) -> &'static str {
        match self {
            Self::Json(_) => "json",
            Self::Sqlite(_) => "sqlite",
        }
    }

    pub(crate) fn embedding_store_default(&self) -> Option<PathBuf> {
        match self {
            Self::Json(_) => None,
            Self::Sqlite(path) => Some(path.clone()),
        }
    }
}

pub(crate) fn save_memory_manager(
    config: Option<&MemoryStoreConfig>,
    manager: &MemoryManager,
) -> io::Result<()> {
    let Some(config) = config else {
        return Ok(());
    };
    save_memory_snapshot(config, &manager.snapshot())
}

pub(crate) fn load_memory_snapshot(
    config: &MemoryStoreConfig,
) -> io::Result<Option<MemoryManagerSnapshot>> {
    match config {
        MemoryStoreConfig::Json(path) => load_json_snapshot(path),
        MemoryStoreConfig::Sqlite(path) => load_sqlite_snapshot(path),
    }
}

fn save_memory_snapshot(
    config: &MemoryStoreConfig,
    snapshot: &MemoryManagerSnapshot,
) -> io::Result<()> {
    match config {
        MemoryStoreConfig::Json(path) => save_json_snapshot(path, snapshot),
        MemoryStoreConfig::Sqlite(path) => save_sqlite_snapshot(path, snapshot),
    }
}

fn save_json_snapshot(path: &Path, snapshot: &MemoryManagerSnapshot) -> io::Result<()> {
    ensure_parent_dir(path)?;
    let store = StoredMemoryStore::from(snapshot);
    let payload = serde_json::to_string_pretty(&store).map_err(serde_error)?;
    fs::write(path, payload)
}

fn load_json_snapshot(path: &Path) -> io::Result<Option<MemoryManagerSnapshot>> {
    if !path.exists() {
        return Ok(None);
    }
    let contents = fs::read_to_string(path)?;
    if contents.trim().is_empty() {
        return Ok(None);
    }
    parse_snapshot_json(&contents).map(Some)
}

fn save_sqlite_snapshot(path: &Path, snapshot: &MemoryManagerSnapshot) -> io::Result<()> {
    ensure_parent_dir(path)?;
    let conn = Connection::open(path).map_err(sqlite_error)?;
    ensure_sqlite_schema(&conn)?;
    let store = StoredMemoryStore::from(snapshot);
    let payload = serde_json::to_string(&store).map_err(serde_error)?;
    conn.execute(
        "INSERT INTO memory_store_snapshots (id, payload, updated_at) VALUES (1, ?1, ?2)
         ON CONFLICT(id) DO UPDATE SET payload = excluded.payload, updated_at = excluded.updated_at",
        params![payload, now_millis() as i64],
    )
    .map_err(sqlite_error)?;
    Ok(())
}

fn load_sqlite_snapshot(path: &Path) -> io::Result<Option<MemoryManagerSnapshot>> {
    if !path.exists() {
        return Ok(None);
    }
    let conn = Connection::open(path).map_err(sqlite_error)?;
    ensure_sqlite_schema(&conn)?;
    let payload: Option<String> = conn
        .query_row(
            "SELECT payload FROM memory_store_snapshots WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(sqlite_error)?;
    payload
        .map(|payload| parse_snapshot_json(&payload))
        .transpose()
}

fn ensure_sqlite_schema(conn: &Connection) -> io::Result<()> {
    conn.execute_batch(SQLITE_SCHEMA).map_err(sqlite_error)
}

fn parse_snapshot_json(contents: &str) -> io::Result<MemoryManagerSnapshot> {
    let value = serde_json::from_str::<serde_json::Value>(contents).map_err(serde_error)?;
    if value.is_array() {
        let memories = serde_json::from_value::<Vec<StoredMemory>>(value).map_err(serde_error)?;
        return snapshot_from_store(StoredMemoryStore {
            version: STORE_VERSION,
            memories,
            entities: Vec::new(),
            agent_relationships: Vec::new(),
            temporal_facts: Vec::new(),
            temporal_relationships: Vec::new(),
        });
    }
    let store = serde_json::from_value::<StoredMemoryStore>(value).map_err(serde_error)?;
    snapshot_from_store(store)
}

fn snapshot_from_store(store: StoredMemoryStore) -> io::Result<MemoryManagerSnapshot> {
    if store.version > STORE_VERSION {
        return Err(invalid_data(format!(
            "unsupported memory store version: {}",
            store.version
        )));
    }
    let memories = store
        .memories
        .into_iter()
        .map(StoredMemory::into_domain)
        .collect::<io::Result<Vec<_>>>()?;
    let memory_entities = store
        .entities
        .into_iter()
        .map(StoredMemoryEntity::into_domain)
        .collect::<io::Result<Vec<_>>>()?;
    let agent_relationships = store
        .agent_relationships
        .into_iter()
        .map(StoredAgentRelationship::into_domain)
        .collect::<io::Result<Vec<_>>>()?;
    let temporal_facts = store
        .temporal_facts
        .into_iter()
        .map(StoredTemporalFact::into_domain)
        .collect::<io::Result<Vec<_>>>()?;
    let temporal_relationships = store
        .temporal_relationships
        .into_iter()
        .map(StoredTemporalRelationship::into_domain)
        .collect::<io::Result<Vec<_>>>()?;

    Ok(MemoryManagerSnapshot {
        memories,
        memory_entities,
        agent_relationships,
        temporal_facts,
        temporal_relationships,
    })
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredMemoryStore {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    memories: Vec<StoredMemory>,
    #[serde(default)]
    entities: Vec<StoredMemoryEntity>,
    #[serde(default)]
    agent_relationships: Vec<StoredAgentRelationship>,
    #[serde(default)]
    temporal_facts: Vec<StoredTemporalFact>,
    #[serde(default)]
    temporal_relationships: Vec<StoredTemporalRelationship>,
}

impl From<&MemoryManagerSnapshot> for StoredMemoryStore {
    fn from(snapshot: &MemoryManagerSnapshot) -> Self {
        Self {
            version: STORE_VERSION,
            memories: snapshot.memories.iter().map(StoredMemory::from).collect(),
            entities: snapshot
                .memory_entities
                .iter()
                .map(StoredMemoryEntity::from)
                .collect(),
            agent_relationships: snapshot
                .agent_relationships
                .iter()
                .map(StoredAgentRelationship::from)
                .collect(),
            temporal_facts: snapshot
                .temporal_facts
                .iter()
                .map(StoredTemporalFact::from)
                .collect(),
            temporal_relationships: snapshot
                .temporal_relationships
                .iter()
                .map(StoredTemporalRelationship::from)
                .collect(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredMemory {
    id: String,
    agent_id: String,
    agent_name: String,
    #[serde(rename = "type")]
    memory_type: String,
    content: String,
    importance: f64,
    created_at: u128,
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    room_id: Option<String>,
    #[serde(default)]
    world_id: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
}

impl From<&Memory> for StoredMemory {
    fn from(memory: &Memory) -> Self {
        Self {
            id: memory.id.clone(),
            agent_id: memory.agent_id.clone(),
            agent_name: memory.agent_name.clone(),
            memory_type: memory.memory_type.as_str().into(),
            content: memory.content.clone(),
            importance: memory.importance,
            created_at: memory.created_at,
            tags: memory.tags.clone(),
            scope: Some(memory.scope.as_str().into()),
            room_id: memory.room_id.clone(),
            world_id: memory.world_id.clone(),
            session_id: memory.session_id.clone(),
        }
    }
}

impl StoredMemory {
    fn into_domain(self) -> io::Result<Memory> {
        Ok(Memory {
            id: self.id,
            agent_id: self.agent_id,
            agent_name: self.agent_name,
            memory_type: parse_memory_type(&self.memory_type)?,
            content: self.content,
            importance: validate_probability("importance", self.importance)?,
            created_at: self.created_at,
            tags: self.tags,
            scope: self
                .scope
                .as_deref()
                .map(parse_memory_scope)
                .transpose()?
                .unwrap_or_else(|| default_scope(&self.room_id)),
            room_id: self.room_id,
            world_id: self.world_id,
            session_id: self.session_id,
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredMemoryEntity {
    kind: String,
    id: String,
    name: String,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    summary: Option<String>,
    created_at: u128,
    updated_at: u128,
}

impl From<&MemoryEntity> for StoredMemoryEntity {
    fn from(entity: &MemoryEntity) -> Self {
        Self {
            kind: entity.kind.as_str().into(),
            id: entity.id.clone(),
            name: entity.name.clone(),
            aliases: entity.aliases.clone(),
            summary: entity.summary.clone(),
            created_at: entity.created_at,
            updated_at: entity.updated_at,
        }
    }
}

impl StoredMemoryEntity {
    fn into_domain(self) -> io::Result<MemoryEntity> {
        Ok(MemoryEntity {
            kind: parse_endpoint_kind(&self.kind)?,
            id: self.id,
            name: self.name,
            aliases: self.aliases,
            summary: self.summary,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredAgentRelationship {
    id: String,
    source_kind: String,
    source_agent_id: String,
    source_agent_name: String,
    target_kind: String,
    target_agent_id: String,
    target_agent_name: String,
    relationship_type: String,
    #[serde(default)]
    summary: Option<String>,
    strength: f64,
    confidence: f64,
    #[serde(default)]
    evidence_memory_ids: Vec<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default)]
    room_id: Option<String>,
    #[serde(default)]
    world_id: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    created_at: u128,
    updated_at: u128,
}

impl From<&AgentRelationship> for StoredAgentRelationship {
    fn from(relationship: &AgentRelationship) -> Self {
        Self {
            id: relationship.id.clone(),
            source_kind: relationship.source_kind.as_str().into(),
            source_agent_id: relationship.source_agent_id.clone(),
            source_agent_name: relationship.source_agent_name.clone(),
            target_kind: relationship.target_kind.as_str().into(),
            target_agent_id: relationship.target_agent_id.clone(),
            target_agent_name: relationship.target_agent_name.clone(),
            relationship_type: relationship.relationship_type.clone(),
            summary: relationship.summary.clone(),
            strength: relationship.strength,
            confidence: relationship.confidence,
            evidence_memory_ids: relationship.evidence_memory_ids.clone(),
            tags: relationship.tags.clone(),
            room_id: relationship.room_id.clone(),
            world_id: relationship.world_id.clone(),
            session_id: relationship.session_id.clone(),
            created_at: relationship.created_at,
            updated_at: relationship.updated_at,
        }
    }
}

impl StoredAgentRelationship {
    fn into_domain(self) -> io::Result<AgentRelationship> {
        Ok(AgentRelationship {
            id: self.id,
            source_kind: parse_endpoint_kind(&self.source_kind)?,
            source_agent_id: self.source_agent_id,
            source_agent_name: self.source_agent_name,
            target_kind: parse_endpoint_kind(&self.target_kind)?,
            target_agent_id: self.target_agent_id,
            target_agent_name: self.target_agent_name,
            relationship_type: self.relationship_type,
            summary: self.summary,
            strength: validate_probability("relationship strength", self.strength)?,
            confidence: validate_probability("relationship confidence", self.confidence)?,
            evidence_memory_ids: self.evidence_memory_ids,
            tags: self.tags,
            room_id: self.room_id,
            world_id: self.world_id,
            session_id: self.session_id,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredTemporalFact {
    id: String,
    subject_kind: String,
    subject_id: String,
    subject_name: String,
    predicate: String,
    #[serde(default)]
    object_kind: Option<String>,
    #[serde(default)]
    object_id: Option<String>,
    #[serde(default)]
    object_name: Option<String>,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    valid_from: Option<u128>,
    #[serde(default)]
    valid_to: Option<u128>,
    observed_at: u128,
    confidence: f64,
    #[serde(default)]
    evidence_memory_ids: Vec<String>,
    #[serde(default)]
    supersedes_fact_ids: Vec<String>,
    #[serde(default = "active_status")]
    status: String,
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default)]
    room_id: Option<String>,
    #[serde(default)]
    world_id: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    created_at: u128,
    updated_at: u128,
}

impl From<&TemporalFact> for StoredTemporalFact {
    fn from(fact: &TemporalFact) -> Self {
        Self {
            id: fact.id.clone(),
            subject_kind: fact.subject_kind.as_str().into(),
            subject_id: fact.subject_id.clone(),
            subject_name: fact.subject_name.clone(),
            predicate: fact.predicate.clone(),
            object_kind: fact.object_kind.map(|kind| kind.as_str().into()),
            object_id: fact.object_id.clone(),
            object_name: fact.object_name.clone(),
            value: fact.value.clone(),
            valid_from: fact.valid_from,
            valid_to: fact.valid_to,
            observed_at: fact.observed_at,
            confidence: fact.confidence,
            evidence_memory_ids: fact.evidence_memory_ids.clone(),
            supersedes_fact_ids: fact.supersedes_fact_ids.clone(),
            status: fact.status.as_str().into(),
            tags: fact.tags.clone(),
            room_id: fact.room_id.clone(),
            world_id: fact.world_id.clone(),
            session_id: fact.session_id.clone(),
            created_at: fact.created_at,
            updated_at: fact.updated_at,
        }
    }
}

impl StoredTemporalFact {
    fn into_domain(self) -> io::Result<TemporalFact> {
        Ok(TemporalFact {
            id: self.id,
            subject_kind: parse_endpoint_kind(&self.subject_kind)?,
            subject_id: self.subject_id,
            subject_name: self.subject_name,
            predicate: self.predicate,
            object_kind: self
                .object_kind
                .as_deref()
                .map(parse_endpoint_kind)
                .transpose()?,
            object_id: self.object_id,
            object_name: self.object_name,
            value: self.value,
            valid_from: self.valid_from,
            valid_to: self.valid_to,
            observed_at: self.observed_at,
            confidence: validate_probability("temporal fact confidence", self.confidence)?,
            evidence_memory_ids: self.evidence_memory_ids,
            supersedes_fact_ids: self.supersedes_fact_ids,
            status: parse_temporal_status(&self.status)?,
            tags: self.tags,
            room_id: self.room_id,
            world_id: self.world_id,
            session_id: self.session_id,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredTemporalRelationship {
    id: String,
    source_kind: String,
    source_id: String,
    source_name: String,
    target_kind: String,
    target_id: String,
    target_name: String,
    relationship_type: String,
    #[serde(default)]
    summary: Option<String>,
    strength: f64,
    confidence: f64,
    #[serde(default)]
    valid_from: Option<u128>,
    #[serde(default)]
    valid_to: Option<u128>,
    observed_at: u128,
    #[serde(default)]
    evidence_memory_ids: Vec<String>,
    #[serde(default)]
    supersedes_relationship_ids: Vec<String>,
    #[serde(default = "active_status")]
    status: String,
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default)]
    room_id: Option<String>,
    #[serde(default)]
    world_id: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    created_at: u128,
    updated_at: u128,
}

impl From<&TemporalRelationship> for StoredTemporalRelationship {
    fn from(relationship: &TemporalRelationship) -> Self {
        Self {
            id: relationship.id.clone(),
            source_kind: relationship.source_kind.as_str().into(),
            source_id: relationship.source_id.clone(),
            source_name: relationship.source_name.clone(),
            target_kind: relationship.target_kind.as_str().into(),
            target_id: relationship.target_id.clone(),
            target_name: relationship.target_name.clone(),
            relationship_type: relationship.relationship_type.clone(),
            summary: relationship.summary.clone(),
            strength: relationship.strength,
            confidence: relationship.confidence,
            valid_from: relationship.valid_from,
            valid_to: relationship.valid_to,
            observed_at: relationship.observed_at,
            evidence_memory_ids: relationship.evidence_memory_ids.clone(),
            supersedes_relationship_ids: relationship.supersedes_relationship_ids.clone(),
            status: relationship.status.as_str().into(),
            tags: relationship.tags.clone(),
            room_id: relationship.room_id.clone(),
            world_id: relationship.world_id.clone(),
            session_id: relationship.session_id.clone(),
            created_at: relationship.created_at,
            updated_at: relationship.updated_at,
        }
    }
}

impl StoredTemporalRelationship {
    fn into_domain(self) -> io::Result<TemporalRelationship> {
        Ok(TemporalRelationship {
            id: self.id,
            source_kind: parse_endpoint_kind(&self.source_kind)?,
            source_id: self.source_id,
            source_name: self.source_name,
            target_kind: parse_endpoint_kind(&self.target_kind)?,
            target_id: self.target_id,
            target_name: self.target_name,
            relationship_type: self.relationship_type,
            summary: self.summary,
            strength: validate_probability("temporal relationship strength", self.strength)?,
            confidence: validate_probability("temporal relationship confidence", self.confidence)?,
            valid_from: self.valid_from,
            valid_to: self.valid_to,
            observed_at: self.observed_at,
            evidence_memory_ids: self.evidence_memory_ids,
            supersedes_relationship_ids: self.supersedes_relationship_ids,
            status: parse_temporal_status(&self.status)?,
            tags: self.tags,
            room_id: self.room_id,
            world_id: self.world_id,
            session_id: self.session_id,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

fn parse_memory_type(value: &str) -> io::Result<MemoryType> {
    MemoryType::parse(value).map_err(|_| invalid_data(format!("invalid memory type: {value}")))
}

fn parse_memory_scope(value: &str) -> io::Result<MemoryScope> {
    MemoryScope::parse(value).map_err(|_| invalid_data(format!("invalid memory scope: {value}")))
}

fn parse_endpoint_kind(value: &str) -> io::Result<RelationshipEndpointKind> {
    RelationshipEndpointKind::from_str(value)
        .map_err(|_| invalid_data(format!("invalid relationship endpoint kind: {value}")))
}

fn parse_temporal_status(value: &str) -> io::Result<TemporalRecordStatus> {
    TemporalRecordStatus::parse(value)
        .map_err(|_| invalid_data(format!("invalid temporal status: {value}")))
}

fn validate_probability(label: &str, value: f64) -> io::Result<f64> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(value)
    } else {
        Err(invalid_data(format!("{label} must be between 0 and 1")))
    }
}

fn default_scope(room_id: &Option<String>) -> MemoryScope {
    if room_id.as_deref().is_some_and(|value| !value.is_empty()) {
        MemoryScope::Room
    } else {
        MemoryScope::Private
    }
}

fn active_status() -> String {
    TemporalRecordStatus::Active.as_str().into()
}

fn ensure_parent_dir(path: &Path) -> io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn serde_error(error: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

fn sqlite_error(error: rusqlite::Error) -> io::Error {
    io::Error::new(io::ErrorKind::Other, error)
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use std::fs::remove_file;
    use std::sync::atomic::{AtomicU64, Ordering};

    use anima_memory::{
        AgentRelationshipOptions, MemorySearchOptions, NewAgentRelationship, NewMemory,
        NewTemporalFact, NewTemporalRelationship, RecentMemoryOptions, TemporalFactOptions,
        TemporalRelationshipOptions,
    };

    use super::*;

    static NEXT_TEMP_FILE_ID: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn json_round_trip_preserves_memory_graph_and_temporal_records() {
        let path = temp_path("json", "json");
        let _ = remove_file(&path);
        let config = MemoryStoreConfig::Json(path.clone());
        let manager = sample_manager();

        save_memory_manager(Some(&config), &manager).expect("json save should succeed");
        let snapshot = load_memory_snapshot(&config)
            .expect("json load should succeed")
            .expect("json snapshot should exist");
        let mut reloaded = MemoryManager::new();
        reloaded.replace_snapshot(snapshot);

        assert_eq!(reloaded.size(), 1);
        assert_eq!(reloaded.relationship_count(), 1);
        assert_eq!(reloaded.temporal_fact_count(), 1);
        assert_eq!(reloaded.temporal_relationship_count(), 1);
        assert!(!reloaded
            .search("rollback review", MemorySearchOptions::default())
            .is_empty());
        assert_eq!(
            reloaded.get_recent(RecentMemoryOptions::default())[0].id,
            manager.get_recent(RecentMemoryOptions::default())[0].id
        );
        let _ = remove_file(&path);
    }

    #[test]
    fn sqlite_round_trip_replaces_previous_snapshot() {
        let path = temp_path("sqlite", "sqlite");
        let _ = remove_file(&path);
        let config = MemoryStoreConfig::Sqlite(path.clone());
        let mut manager = sample_manager();

        save_memory_manager(Some(&config), &manager).expect("first sqlite save should succeed");
        manager.clear(None);
        manager
            .add(NewMemory {
                agent_id: "planner".into(),
                agent_name: "Planner".into(),
                memory_type: MemoryType::Fact,
                content: "Fresh host snapshot memory".into(),
                importance: 0.8,
                tags: None,
                scope: None,
                room_id: None,
                world_id: None,
                session_id: None,
            })
            .expect("fresh memory should add");
        save_memory_manager(Some(&config), &manager).expect("second sqlite save should succeed");

        let snapshot = load_memory_snapshot(&config)
            .expect("sqlite load should succeed")
            .expect("sqlite snapshot should exist");
        let mut reloaded = MemoryManager::new();
        reloaded.replace_snapshot(snapshot);

        assert_eq!(reloaded.size(), 1);
        assert!(reloaded
            .search("fresh snapshot", MemorySearchOptions::default())
            .iter()
            .any(|memory| memory.content == "Fresh host snapshot memory"));
        assert!(!reloaded
            .search("rollback review", MemorySearchOptions::default())
            .iter()
            .any(|memory| memory.content.contains("rollback review")));
        let _ = remove_file(&path);
    }

    fn sample_manager() -> MemoryManager {
        let mut manager = MemoryManager::new();
        let memory = manager
            .add(NewMemory {
                agent_id: "planner".into(),
                agent_name: "Planner".into(),
                memory_type: MemoryType::Fact,
                content: "Critic owns rollback review for the release.".into(),
                importance: 0.82,
                tags: Some(vec!["runtime".into(), "relationship".into()]),
                scope: Some(MemoryScope::Room),
                room_id: Some("room-1".into()),
                world_id: Some("world-1".into()),
                session_id: Some("session-1".into()),
            })
            .expect("memory should add");
        manager
            .upsert_agent_relationship(NewAgentRelationship {
                source_kind: Some(RelationshipEndpointKind::Agent),
                source_agent_id: "planner".into(),
                source_agent_name: "Planner".into(),
                target_kind: Some(RelationshipEndpointKind::Agent),
                target_agent_id: "critic".into(),
                target_agent_name: "Critic".into(),
                relationship_type: "delegates_review_to".into(),
                summary: Some("Planner delegates release review to Critic.".into()),
                strength: 0.7,
                confidence: 0.9,
                evidence_memory_ids: vec![memory.id.clone()],
                tags: Some(vec!["runtime".into()]),
                room_id: Some("room-1".into()),
                world_id: Some("world-1".into()),
                session_id: Some("session-1".into()),
            })
            .expect("relationship should add");
        manager
            .add_temporal_fact(NewTemporalFact {
                subject_kind: RelationshipEndpointKind::Agent,
                subject_id: "critic".into(),
                subject_name: "Critic".into(),
                predicate: "owns_task".into(),
                object_kind: None,
                object_id: None,
                object_name: None,
                value: Some("rollback review".into()),
                valid_from: Some(1_700_000_000_000),
                valid_to: None,
                observed_at: Some(1_700_000_000_100),
                confidence: 0.88,
                evidence_memory_ids: vec![memory.id.clone()],
                supersedes_fact_ids: Vec::new(),
                status: None,
                tags: Some(vec!["runtime".into()]),
                room_id: Some("room-1".into()),
                world_id: Some("world-1".into()),
                session_id: Some("session-1".into()),
            })
            .expect("temporal fact should add");
        manager
            .add_temporal_relationship(NewTemporalRelationship {
                source_kind: RelationshipEndpointKind::Agent,
                source_id: "planner".into(),
                source_name: "Planner".into(),
                target_kind: RelationshipEndpointKind::Agent,
                target_id: "critic".into(),
                target_name: "Critic".into(),
                relationship_type: "delegates_review_to".into(),
                summary: Some("Planner delegated rollback review to Critic.".into()),
                strength: 0.7,
                confidence: 0.9,
                valid_from: Some(1_700_000_000_000),
                valid_to: None,
                observed_at: Some(1_700_000_000_100),
                evidence_memory_ids: vec![memory.id],
                supersedes_relationship_ids: Vec::new(),
                status: None,
                tags: Some(vec!["runtime".into()]),
                room_id: Some("room-1".into()),
                world_id: Some("world-1".into()),
                session_id: Some("session-1".into()),
            })
            .expect("temporal relationship should add");
        assert_eq!(
            manager
                .list_agent_relationships(AgentRelationshipOptions::default())
                .len(),
            1
        );
        assert_eq!(
            manager
                .list_temporal_facts(TemporalFactOptions::default())
                .len(),
            1
        );
        assert_eq!(
            manager
                .list_temporal_relationships(TemporalRelationshipOptions::default())
                .len(),
            1
        );
        manager
    }

    fn temp_path(label: &str, extension: &str) -> PathBuf {
        let suffix = NEXT_TEMP_FILE_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("anima-daemon-memory-{label}-{suffix}.{extension}"))
    }
}
