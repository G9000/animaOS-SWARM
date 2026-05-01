use std::collections::HashMap;
use std::io;
use std::path::Path;

use rusqlite::types::Type;
use rusqlite::{params, Connection, Row, Transaction};

use super::storage::MemoryStore;
use super::{
    validate_importance, AgentRelationship, Memory, MemoryEntity, MemoryScope, MemoryType,
    RelationshipEndpointKind,
};

pub(super) fn save_sqlite_memory_store(
    path: &Path,
    memories: &[Memory],
    memory_entities: &[MemoryEntity],
    agent_relationships: &[AgentRelationship],
) -> io::Result<()> {
    let mut connection = open_connection(path)?;
    ensure_schema(&connection)?;

    let transaction = connection.transaction().map_err(sqlite_error)?;
    clear_store(&transaction)?;
    insert_memories(&transaction, memories)?;
    insert_entities(&transaction, memory_entities)?;
    insert_relationships(&transaction, agent_relationships)?;
    transaction.commit().map_err(sqlite_error)
}

pub(super) fn load_sqlite_memory_store(path: &Path) -> io::Result<Option<MemoryStore>> {
    let connection = open_connection(path)?;
    ensure_schema(&connection)?;

    Ok(Some(MemoryStore {
        memories: load_memories(&connection)?,
        memory_entities: load_entities(&connection)?,
        agent_relationships: load_relationships(&connection)?,
    }))
}

fn open_connection(path: &Path) -> io::Result<Connection> {
    let connection = Connection::open(path).map_err(sqlite_error)?;
    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(sqlite_error)?;
    Ok(connection)
}

fn ensure_schema(connection: &Connection) -> io::Result<()> {
    connection
        .execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS memory_schema (
                key TEXT PRIMARY KEY NOT NULL,
                value TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS memories (
                id TEXT PRIMARY KEY NOT NULL,
                agent_id TEXT NOT NULL,
                agent_name TEXT NOT NULL,
                memory_type TEXT NOT NULL,
                content TEXT NOT NULL,
                importance REAL NOT NULL,
                created_at TEXT NOT NULL,
                tags_present INTEGER NOT NULL DEFAULT 0,
                scope TEXT NOT NULL,
                room_id TEXT,
                world_id TEXT,
                session_id TEXT
            );

            CREATE TABLE IF NOT EXISTS memory_tags (
                memory_id TEXT NOT NULL,
                position INTEGER NOT NULL,
                tag TEXT NOT NULL,
                PRIMARY KEY (memory_id, position),
                FOREIGN KEY (memory_id) REFERENCES memories(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS memory_entities (
                kind TEXT NOT NULL,
                id TEXT NOT NULL,
                name TEXT NOT NULL,
                summary TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (kind, id)
            );

            CREATE TABLE IF NOT EXISTS entity_aliases (
                entity_kind TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                position INTEGER NOT NULL,
                alias TEXT NOT NULL,
                PRIMARY KEY (entity_kind, entity_id, position),
                FOREIGN KEY (entity_kind, entity_id) REFERENCES memory_entities(kind, id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS agent_relationships (
                id TEXT PRIMARY KEY NOT NULL,
                source_kind TEXT NOT NULL,
                source_agent_id TEXT NOT NULL,
                source_agent_name TEXT NOT NULL,
                target_kind TEXT NOT NULL,
                target_agent_id TEXT NOT NULL,
                target_agent_name TEXT NOT NULL,
                relationship_type TEXT NOT NULL,
                summary TEXT,
                strength REAL NOT NULL,
                confidence REAL NOT NULL,
                tags_present INTEGER NOT NULL DEFAULT 0,
                room_id TEXT,
                world_id TEXT,
                session_id TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS relationship_evidence (
                relationship_id TEXT NOT NULL,
                position INTEGER NOT NULL,
                memory_id TEXT NOT NULL,
                PRIMARY KEY (relationship_id, position),
                FOREIGN KEY (relationship_id) REFERENCES agent_relationships(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS relationship_tags (
                relationship_id TEXT NOT NULL,
                position INTEGER NOT NULL,
                tag TEXT NOT NULL,
                PRIMARY KEY (relationship_id, position),
                FOREIGN KEY (relationship_id) REFERENCES agent_relationships(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_memories_agent ON memories(agent_id);
            CREATE INDEX IF NOT EXISTS idx_memories_context ON memories(scope, room_id, world_id, session_id);
            CREATE INDEX IF NOT EXISTS idx_relationships_source ON agent_relationships(source_kind, source_agent_id);
            CREATE INDEX IF NOT EXISTS idx_relationships_target ON agent_relationships(target_kind, target_agent_id);
            "#,
        )
        .map_err(sqlite_error)?;
    connection
        .execute(
            "INSERT OR REPLACE INTO memory_schema(key, value) VALUES ('version', '1')",
            [],
        )
        .map_err(sqlite_error)?;
    Ok(())
}

fn clear_store(transaction: &Transaction<'_>) -> io::Result<()> {
    transaction
        .execute_batch(
            r#"
            DELETE FROM memory_tags;
            DELETE FROM entity_aliases;
            DELETE FROM relationship_evidence;
            DELETE FROM relationship_tags;
            DELETE FROM agent_relationships;
            DELETE FROM memory_entities;
            DELETE FROM memories;
            "#,
        )
        .map_err(sqlite_error)
}

fn insert_memories(transaction: &Transaction<'_>, memories: &[Memory]) -> io::Result<()> {
    for memory in memories {
        transaction
            .execute(
                r#"
                INSERT INTO memories(
                    id, agent_id, agent_name, memory_type, content, importance, created_at,
                    tags_present, scope, room_id, world_id, session_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                "#,
                params![
                    &memory.id,
                    &memory.agent_id,
                    &memory.agent_name,
                    memory.memory_type.as_str(),
                    &memory.content,
                    memory.importance,
                    memory.created_at.to_string(),
                    memory.tags.is_some() as i64,
                    memory.scope.as_str(),
                    memory.room_id.as_deref(),
                    memory.world_id.as_deref(),
                    memory.session_id.as_deref(),
                ],
            )
            .map_err(sqlite_error)?;

        if let Some(tags) = &memory.tags {
            for (position, tag) in tags.iter().enumerate() {
                transaction
                    .execute(
                        "INSERT INTO memory_tags(memory_id, position, tag) VALUES (?1, ?2, ?3)",
                        params![&memory.id, position as i64, tag],
                    )
                    .map_err(sqlite_error)?;
            }
        }
    }
    Ok(())
}

fn insert_entities(transaction: &Transaction<'_>, entities: &[MemoryEntity]) -> io::Result<()> {
    for entity in entities {
        transaction
            .execute(
                r#"
                INSERT INTO memory_entities(kind, id, name, summary, created_at, updated_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                "#,
                params![
                    entity.kind.as_str(),
                    &entity.id,
                    &entity.name,
                    entity.summary.as_deref(),
                    entity.created_at.to_string(),
                    entity.updated_at.to_string(),
                ],
            )
            .map_err(sqlite_error)?;

        for (position, alias) in entity.aliases.iter().enumerate() {
            transaction
                .execute(
                    r#"
                    INSERT INTO entity_aliases(entity_kind, entity_id, position, alias)
                    VALUES (?1, ?2, ?3, ?4)
                    "#,
                    params![entity.kind.as_str(), &entity.id, position as i64, alias],
                )
                .map_err(sqlite_error)?;
        }
    }
    Ok(())
}

fn insert_relationships(
    transaction: &Transaction<'_>,
    relationships: &[AgentRelationship],
) -> io::Result<()> {
    for relationship in relationships {
        transaction
            .execute(
                r#"
                INSERT INTO agent_relationships(
                    id, source_kind, source_agent_id, source_agent_name, target_kind,
                    target_agent_id, target_agent_name, relationship_type, summary, strength,
                    confidence, tags_present, room_id, world_id, session_id, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
                "#,
                params![
                    &relationship.id,
                    relationship.source_kind.as_str(),
                    &relationship.source_agent_id,
                    &relationship.source_agent_name,
                    relationship.target_kind.as_str(),
                    &relationship.target_agent_id,
                    &relationship.target_agent_name,
                    &relationship.relationship_type,
                    relationship.summary.as_deref(),
                    relationship.strength,
                    relationship.confidence,
                    relationship.tags.is_some() as i64,
                    relationship.room_id.as_deref(),
                    relationship.world_id.as_deref(),
                    relationship.session_id.as_deref(),
                    relationship.created_at.to_string(),
                    relationship.updated_at.to_string(),
                ],
            )
            .map_err(sqlite_error)?;

        for (position, memory_id) in relationship.evidence_memory_ids.iter().enumerate() {
            transaction
                .execute(
                    r#"
                    INSERT INTO relationship_evidence(relationship_id, position, memory_id)
                    VALUES (?1, ?2, ?3)
                    "#,
                    params![&relationship.id, position as i64, memory_id],
                )
                .map_err(sqlite_error)?;
        }
        if let Some(tags) = &relationship.tags {
            for (position, tag) in tags.iter().enumerate() {
                transaction
                    .execute(
                        r#"
                        INSERT INTO relationship_tags(relationship_id, position, tag)
                        VALUES (?1, ?2, ?3)
                        "#,
                        params![&relationship.id, position as i64, tag],
                    )
                    .map_err(sqlite_error)?;
            }
        }
    }
    Ok(())
}

fn load_memories(connection: &Connection) -> io::Result<Vec<Memory>> {
    let mut tags = load_memory_tags(connection)?;
    let mut statement = connection
        .prepare(
            r#"
            SELECT id, agent_id, agent_name, memory_type, content, importance, created_at,
                   tags_present, scope, room_id, world_id, session_id
            FROM memories
            ORDER BY CAST(created_at AS INTEGER), id
            "#,
        )
        .map_err(sqlite_error)?;
    let mut rows = statement.query([]).map_err(sqlite_error)?;
    let mut memories = Vec::new();

    while let Some(row) = rows.next().map_err(sqlite_error)? {
        let id: String = row.get(0).map_err(sqlite_error)?;
        let memory_type = parse_memory_type(row, 3)?;
        let scope = parse_memory_scope(row, 8)?;
        let importance = parse_unit_interval(row, 5, "invalid memory importance")?;
        let tags_present = truthy_i64(row, 7)?;
        memories.push(Memory {
            id: id.clone(),
            agent_id: row.get(1).map_err(sqlite_error)?,
            agent_name: row.get(2).map_err(sqlite_error)?,
            memory_type,
            content: row.get(4).map_err(sqlite_error)?,
            importance,
            created_at: parse_u128(row, 6)?,
            tags: tags_present.then(|| tags.remove(&id).unwrap_or_default()),
            scope,
            room_id: row.get(9).map_err(sqlite_error)?,
            world_id: row.get(10).map_err(sqlite_error)?,
            session_id: row.get(11).map_err(sqlite_error)?,
        });
    }

    Ok(memories)
}

fn load_entities(connection: &Connection) -> io::Result<Vec<MemoryEntity>> {
    let mut aliases = load_entity_aliases(connection)?;
    let mut statement = connection
        .prepare(
            r#"
            SELECT kind, id, name, summary, created_at, updated_at
            FROM memory_entities
            ORDER BY CAST(created_at AS INTEGER), kind, id
            "#,
        )
        .map_err(sqlite_error)?;
    let mut rows = statement.query([]).map_err(sqlite_error)?;
    let mut entities = Vec::new();

    while let Some(row) = rows.next().map_err(sqlite_error)? {
        let kind = parse_endpoint_kind(row, 0)?;
        let id: String = row.get(1).map_err(sqlite_error)?;
        entities.push(MemoryEntity {
            kind,
            id: id.clone(),
            name: row.get(2).map_err(sqlite_error)?,
            aliases: aliases
                .remove(&compound_key(kind.as_str(), &id))
                .unwrap_or_default(),
            summary: row.get(3).map_err(sqlite_error)?,
            created_at: parse_u128(row, 4)?,
            updated_at: parse_u128(row, 5)?,
        });
    }

    Ok(entities)
}

fn load_relationships(connection: &Connection) -> io::Result<Vec<AgentRelationship>> {
    let mut evidence = load_relationship_evidence(connection)?;
    let mut tags = load_relationship_tags(connection)?;
    let mut statement = connection
        .prepare(
            r#"
            SELECT id, source_kind, source_agent_id, source_agent_name, target_kind,
                   target_agent_id, target_agent_name, relationship_type, summary, strength,
                   confidence, tags_present, room_id, world_id, session_id, created_at, updated_at
            FROM agent_relationships
            ORDER BY CAST(created_at AS INTEGER), id
            "#,
        )
        .map_err(sqlite_error)?;
    let mut rows = statement.query([]).map_err(sqlite_error)?;
    let mut relationships = Vec::new();

    while let Some(row) = rows.next().map_err(sqlite_error)? {
        let id: String = row.get(0).map_err(sqlite_error)?;
        let tags_present = truthy_i64(row, 11)?;
        relationships.push(AgentRelationship {
            id: id.clone(),
            source_kind: parse_endpoint_kind(row, 1)?,
            source_agent_id: row.get(2).map_err(sqlite_error)?,
            source_agent_name: row.get(3).map_err(sqlite_error)?,
            target_kind: parse_endpoint_kind(row, 4)?,
            target_agent_id: row.get(5).map_err(sqlite_error)?,
            target_agent_name: row.get(6).map_err(sqlite_error)?,
            relationship_type: row.get(7).map_err(sqlite_error)?,
            summary: row.get(8).map_err(sqlite_error)?,
            strength: parse_unit_interval(row, 9, "invalid relationship strength")?,
            confidence: parse_unit_interval(row, 10, "invalid relationship confidence")?,
            evidence_memory_ids: evidence.remove(&id).unwrap_or_default(),
            tags: tags_present.then(|| tags.remove(&id).unwrap_or_default()),
            room_id: row.get(12).map_err(sqlite_error)?,
            world_id: row.get(13).map_err(sqlite_error)?,
            session_id: row.get(14).map_err(sqlite_error)?,
            created_at: parse_u128(row, 15)?,
            updated_at: parse_u128(row, 16)?,
        });
    }

    Ok(relationships)
}

fn load_memory_tags(connection: &Connection) -> io::Result<HashMap<String, Vec<String>>> {
    load_string_children(
        connection,
        "SELECT memory_id, tag FROM memory_tags ORDER BY memory_id, position",
    )
}

fn load_entity_aliases(connection: &Connection) -> io::Result<HashMap<String, Vec<String>>> {
    let mut statement = connection
        .prepare(
            "SELECT entity_kind, entity_id, alias FROM entity_aliases ORDER BY entity_kind, entity_id, position",
        )
        .map_err(sqlite_error)?;
    let mut rows = statement.query([]).map_err(sqlite_error)?;
    let mut values: HashMap<String, Vec<String>> = HashMap::new();

    while let Some(row) = rows.next().map_err(sqlite_error)? {
        let kind: String = row.get(0).map_err(sqlite_error)?;
        let id: String = row.get(1).map_err(sqlite_error)?;
        let alias: String = row.get(2).map_err(sqlite_error)?;
        values
            .entry(compound_key(&kind, &id))
            .or_default()
            .push(alias);
    }

    Ok(values)
}

fn load_relationship_evidence(connection: &Connection) -> io::Result<HashMap<String, Vec<String>>> {
    load_string_children(
        connection,
        "SELECT relationship_id, memory_id FROM relationship_evidence ORDER BY relationship_id, position",
    )
}

fn load_relationship_tags(connection: &Connection) -> io::Result<HashMap<String, Vec<String>>> {
    load_string_children(
        connection,
        "SELECT relationship_id, tag FROM relationship_tags ORDER BY relationship_id, position",
    )
}

fn load_string_children(
    connection: &Connection,
    query: &str,
) -> io::Result<HashMap<String, Vec<String>>> {
    let mut statement = connection.prepare(query).map_err(sqlite_error)?;
    let mut rows = statement.query([]).map_err(sqlite_error)?;
    let mut values: HashMap<String, Vec<String>> = HashMap::new();

    while let Some(row) = rows.next().map_err(sqlite_error)? {
        let parent_id: String = row.get(0).map_err(sqlite_error)?;
        let value: String = row.get(1).map_err(sqlite_error)?;
        values.entry(parent_id).or_default().push(value);
    }

    Ok(values)
}

fn parse_memory_type(row: &Row<'_>, column: usize) -> io::Result<MemoryType> {
    let value: String = row.get(column).map_err(sqlite_error)?;
    MemoryType::parse(&value).map_err(|_| decode_error(column, Type::Text, "invalid memory type"))
}

fn parse_memory_scope(row: &Row<'_>, column: usize) -> io::Result<MemoryScope> {
    let value: String = row.get(column).map_err(sqlite_error)?;
    MemoryScope::parse(&value).map_err(|_| decode_error(column, Type::Text, "invalid memory scope"))
}

fn parse_endpoint_kind(row: &Row<'_>, column: usize) -> io::Result<RelationshipEndpointKind> {
    let value: String = row.get(column).map_err(sqlite_error)?;
    RelationshipEndpointKind::from_str(&value)
        .map_err(|_| decode_error(column, Type::Text, "invalid relationship endpoint kind"))
}

fn parse_u128(row: &Row<'_>, column: usize) -> io::Result<u128> {
    let value: String = row.get(column).map_err(sqlite_error)?;
    value.parse::<u128>().map_err(|error| {
        sqlite_error(rusqlite::Error::FromSqlConversionFailure(
            column,
            Type::Text,
            Box::new(error),
        ))
    })
}

fn truthy_i64(row: &Row<'_>, column: usize) -> io::Result<bool> {
    let value: i64 = row.get(column).map_err(sqlite_error)?;
    Ok(value != 0)
}

fn parse_unit_interval(row: &Row<'_>, column: usize, message: &'static str) -> io::Result<f64> {
    let value: f64 = row.get(column).map_err(sqlite_error)?;
    validate_importance(value).map_err(|_| decode_error(column, Type::Real, message))
}

fn compound_key(left: &str, right: &str) -> String {
    format!("{left}:{right}")
}

fn decode_error(column: usize, kind: Type, message: &'static str) -> io::Error {
    sqlite_error(rusqlite::Error::FromSqlConversionFailure(
        column,
        kind,
        Box::new(io::Error::new(io::ErrorKind::InvalidData, message)),
    ))
}

fn sqlite_error(error: rusqlite::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}
