//! Explicit conversions between the legacy in-memory manager and `anima_core` ports.
//!
//! `MemoryManager` is intentionally not exposed as a `MemoryPort`: it has no durable revision
//! compare-and-swap, correction lineage, or auditable forgetting semantics. Presenting it as one
//! would make portable hosts believe guarantees exist that the legacy manager cannot provide.

use anima_core::{
    MemoryKind as CoreMemoryKind, MemoryPortError, MemoryProvenance, MemoryRecord, MemoryRetention,
    MemoryScope as CoreMemoryScope, MemoryWrite,
};

use crate::{Memory, MemoryScope, MemoryType, NewMemory};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LegacyBridgeErrorCode {
    InvalidInput,
    UnsupportedKind,
    UnsupportedScope,
    UnsupportedLifecycle,
    UnsupportedOperation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LegacyBridgeError {
    pub code: LegacyBridgeErrorCode,
    pub message: String,
}

impl LegacyBridgeError {
    fn new(code: LegacyBridgeErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for LegacyBridgeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for LegacyBridgeError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LegacyMemoryContext {
    pub agent_id: String,
    pub agent_name: String,
}

impl LegacyMemoryContext {
    pub fn new(
        agent_id: impl Into<String>,
        agent_name: impl Into<String>,
    ) -> Result<Self, LegacyBridgeError> {
        Ok(Self {
            agent_id: non_empty(agent_id.into(), "legacy agent ID")?,
            agent_name: non_empty(agent_name.into(), "legacy agent name")?,
        })
    }
}

/// Converts a persisted legacy memory into a portable core record.
pub fn legacy_memory_to_core(memory: &Memory) -> Result<MemoryRecord, LegacyBridgeError> {
    let write = legacy_fields_to_core_write(
        memory.id.clone(),
        memory.memory_type,
        memory.scope,
        &memory.agent_id,
        &memory.content,
        memory.importance,
        memory.created_at,
        memory.room_id.as_deref(),
        memory.world_id.as_deref(),
        memory.session_id.as_deref(),
    )?;
    MemoryRecord::from_write(write, 1).map_err(core_error)
}

/// Converts an unstored legacy `NewMemory` into a portable core write.
/// The caller supplies an ID and observation timestamp because the legacy input has neither.
pub fn legacy_new_memory_to_core(
    memory: &NewMemory,
    id: impl Into<String>,
    observed_at_ms: u64,
) -> Result<MemoryWrite, LegacyBridgeError> {
    legacy_fields_to_core_write(
        id.into(),
        memory.memory_type,
        memory.scope.unwrap_or_else(|| {
            if memory.room_id.is_some() {
                MemoryScope::Room
            } else {
                MemoryScope::Private
            }
        }),
        &memory.agent_id,
        &memory.content,
        memory.importance,
        observed_at_ms,
        memory.room_id.as_deref(),
        memory.world_id.as_deref(),
        memory.session_id.as_deref(),
    )
}

/// Converts the subset of a core write that the legacy manager can faithfully store.
/// Preference and Procedure have no legacy `MemoryType`; corrections and forgetting are likewise
/// intentionally absent rather than emulated with destructive legacy operations.
pub fn core_memory_to_legacy(
    memory: &MemoryWrite,
    context: &LegacyMemoryContext,
) -> Result<NewMemory, LegacyBridgeError> {
    if memory.retention() != &MemoryRetention::default() {
        return Err(LegacyBridgeError::new(
            LegacyBridgeErrorCode::UnsupportedLifecycle,
            "legacy MemoryManager cannot honor core retention, correction, forgetting, or CAS semantics",
        ));
    }

    let memory_type = match memory.kind() {
        CoreMemoryKind::Fact => MemoryType::Fact,
        CoreMemoryKind::Episode
            if memory.provenance().source() == "anima-memory/legacy/task_result" =>
        {
            MemoryType::TaskResult
        }
        CoreMemoryKind::Episode => MemoryType::Observation,
        CoreMemoryKind::Reflection => MemoryType::Reflection,
        CoreMemoryKind::Preference | CoreMemoryKind::Procedure => {
            return Err(LegacyBridgeError::new(
                LegacyBridgeErrorCode::UnsupportedKind,
                format!(
                    "legacy MemoryManager cannot represent core {:?} memories",
                    memory.kind()
                ),
            ));
        }
    };

    let (scope, room_id, world_id, session_id) = match memory.scope() {
        CoreMemoryScope::Owner(_) => {
            return Err(LegacyBridgeError::new(
                LegacyBridgeErrorCode::UnsupportedScope,
                "legacy MemoryManager has no owner scope and must not coerce it to private",
            ));
        }
        CoreMemoryScope::Agent(agent_id) if agent_id.as_str() == context.agent_id => {
            (MemoryScope::Private, None, None, None)
        }
        CoreMemoryScope::Agent(_) => {
            return Err(LegacyBridgeError::new(
                LegacyBridgeErrorCode::UnsupportedScope,
                "legacy private scope cannot preserve a different core agent scope ID",
            ));
        }
        CoreMemoryScope::Session(session_id) => (
            MemoryScope::Room,
            Some(session_id.as_str().to_owned()),
            None,
            Some(session_id.as_str().to_owned()),
        ),
        CoreMemoryScope::Workspace(workspace_id) => (
            MemoryScope::Shared,
            None,
            Some(workspace_id.as_str().to_owned()),
            None,
        ),
    };

    Ok(NewMemory {
        agent_id: context.agent_id.clone(),
        agent_name: context.agent_name.clone(),
        memory_type,
        content: memory.content().to_owned(),
        importance: memory.confidence(),
        tags: None,
        scope: Some(scope),
        room_id,
        world_id,
        session_id,
    })
}

/// Reports why a full `MemoryPort` adapter is unavailable for `MemoryManager`.
pub fn memory_manager_port_unavailable() -> LegacyBridgeError {
    LegacyBridgeError::new(
        LegacyBridgeErrorCode::UnsupportedOperation,
        "MemoryManager is not a MemoryPort: it cannot enforce revision CAS, correction lineage, or auditable forgetting",
    )
}

#[allow(clippy::too_many_arguments)]
fn legacy_fields_to_core_write(
    id: String,
    memory_type: MemoryType,
    legacy_scope: MemoryScope,
    agent_id: &str,
    content: &str,
    importance: f64,
    observed_at_ms: u64,
    room_id: Option<&str>,
    world_id: Option<&str>,
    session_id: Option<&str>,
) -> Result<MemoryWrite, LegacyBridgeError> {
    let kind = match memory_type {
        MemoryType::Fact => CoreMemoryKind::Fact,
        MemoryType::Observation | MemoryType::TaskResult => CoreMemoryKind::Episode,
        MemoryType::Reflection => CoreMemoryKind::Reflection,
    };
    let scope = match legacy_scope {
        MemoryScope::Private => CoreMemoryScope::agent(agent_id.to_owned()),
        MemoryScope::Shared => CoreMemoryScope::workspace(
            required_context(world_id, "world ID for shared legacy memory")?.to_owned(),
        ),
        MemoryScope::Room => CoreMemoryScope::session(
            required_context(
                session_id.or(room_id),
                "session or room ID for room legacy memory",
            )?
            .to_owned(),
        ),
    }
    .map_err(core_error)?;
    let subtype = memory_type.as_str();
    let provenance = MemoryProvenance::new(
        format!("anima-memory/legacy/{subtype}"),
        format!("legacy:{subtype}:{agent_id}"),
        observed_at_ms,
    )
    .map_err(core_error)?;
    MemoryWrite::new(id, kind, scope, content.to_owned(), importance, provenance)
        .map_err(core_error)
}

fn required_context<'a>(value: Option<&'a str>, field: &str) -> Result<&'a str, LegacyBridgeError> {
    value
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            LegacyBridgeError::new(
                LegacyBridgeErrorCode::UnsupportedScope,
                format!("legacy mapping requires {field}"),
            )
        })
}

fn non_empty(value: String, field: &str) -> Result<String, LegacyBridgeError> {
    let value = value.trim().to_owned();
    if value.is_empty() {
        Err(LegacyBridgeError::new(
            LegacyBridgeErrorCode::InvalidInput,
            format!("{field} must not be empty"),
        ))
    } else {
        Ok(value)
    }
}

fn core_error(error: MemoryPortError) -> LegacyBridgeError {
    LegacyBridgeError::new(LegacyBridgeErrorCode::InvalidInput, error.message)
}
