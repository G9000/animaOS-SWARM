use anima_core::{MemoryKind, MemoryPortErrorCode, MemoryProvenance, MemoryScope, MemoryWrite};
use anima_memory::{
    core_memory_to_legacy, legacy_memory_to_core, memory_manager_port_unavailable,
    LegacyBridgeErrorCode, LegacyMemoryContext, Memory, MemoryScope as LegacyScope, MemoryType,
};

fn legacy(memory_type: MemoryType, scope: LegacyScope) -> Memory {
    Memory {
        id: format!("legacy-{}", memory_type.as_str()),
        agent_id: "agent-1".into(),
        agent_name: "Ada".into(),
        memory_type,
        content: "portable content".into(),
        importance: 0.7,
        created_at: 42,
        tags: Some(vec!["source".into()]),
        scope,
        room_id: Some("room-1".into()),
        world_id: Some("workspace-1".into()),
        session_id: Some("session-1".into()),
    }
}

#[test]
fn legacy_kinds_and_scopes_map_without_a_reverse_core_dependency() {
    for (legacy_kind, core_kind) in [
        (MemoryType::Fact, MemoryKind::Fact),
        (MemoryType::Observation, MemoryKind::Episode),
        (MemoryType::TaskResult, MemoryKind::Episode),
        (MemoryType::Reflection, MemoryKind::Reflection),
    ] {
        let mapped = legacy_memory_to_core(&legacy(legacy_kind, LegacyScope::Private)).unwrap();
        assert_eq!(mapped.kind, core_kind);
    }

    let owner = legacy_memory_to_core(&legacy(MemoryType::Fact, LegacyScope::Private)).unwrap();
    assert_eq!(owner.scope, MemoryScope::owner("agent-1").unwrap());
    let workspace = legacy_memory_to_core(&legacy(MemoryType::Fact, LegacyScope::Shared)).unwrap();
    assert_eq!(
        workspace.scope,
        MemoryScope::workspace("workspace-1").unwrap()
    );
    let session = legacy_memory_to_core(&legacy(MemoryType::Fact, LegacyScope::Room)).unwrap();
    assert_eq!(session.scope, MemoryScope::session("session-1").unwrap());
}

#[test]
fn legacy_episode_provenance_preserves_observation_and_task_result_subtype() {
    let observation =
        legacy_memory_to_core(&legacy(MemoryType::Observation, LegacyScope::Private)).unwrap();
    let task_result =
        legacy_memory_to_core(&legacy(MemoryType::TaskResult, LegacyScope::Private)).unwrap();
    assert_eq!(
        observation.provenance.source,
        "anima-memory/legacy/observation"
    );
    assert_eq!(
        task_result.provenance.source,
        "anima-memory/legacy/task_result"
    );
}

#[test]
fn bridge_rejects_core_kinds_and_lifecycle_semantics_the_legacy_manager_cannot_honor() {
    let context = LegacyMemoryContext::new("agent-1", "Ada").unwrap();
    for kind in [MemoryKind::Preference, MemoryKind::Procedure] {
        let write = MemoryWrite::new(
            "core-1",
            kind,
            MemoryScope::agent("agent-1").unwrap(),
            "content",
            0.7,
            MemoryProvenance::new("host", "agent-1", 1).unwrap(),
        )
        .unwrap();
        assert_eq!(
            core_memory_to_legacy(&write, &context).unwrap_err().code,
            LegacyBridgeErrorCode::UnsupportedKind
        );
    }

    let write = MemoryWrite::new(
        "core-2",
        MemoryKind::Fact,
        MemoryScope::owner("owner-1").unwrap(),
        "content",
        0.7,
        MemoryProvenance::new("host", "owner-1", 1).unwrap(),
    )
    .unwrap()
    .with_retention_deadline(99)
    .unwrap();
    assert_eq!(
        core_memory_to_legacy(&write, &context).unwrap_err().code,
        LegacyBridgeErrorCode::UnsupportedLifecycle
    );
    assert_eq!(
        memory_manager_port_unavailable().code,
        LegacyBridgeErrorCode::UnsupportedOperation
    );
    let _ = MemoryPortErrorCode::Unsupported;
}
