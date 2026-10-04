use super::{
    MemoryEntityOptions, MemoryError, MemoryManager, MemoryPatch, MemoryScope, MemorySearchOptions,
    MemoryType, NewAgentRelationship, NewMemory, NewTemporalFact, NewTemporalRelationship,
    RelationshipEndpointKind,
};

fn new_memory(agent_id: &str, content: &str) -> NewMemory {
    NewMemory {
        agent_id: agent_id.into(),
        agent_name: format!("name-{agent_id}"),
        memory_type: MemoryType::Fact,
        content: content.into(),
        importance: 0.5,
        tags: Some(vec!["alpha".into()]),
        scope: Some(MemoryScope::Room),
        room_id: Some("room-1".into()),
        world_id: Some("world-1".into()),
        session_id: Some("session-1".into()),
    }
}

fn add(manager: &mut MemoryManager, agent_id: &str, content: &str) -> super::Memory {
    manager
        .add(new_memory(agent_id, content))
        .expect("memory should be added")
}

fn relationship(
    source: (RelationshipEndpointKind, &str),
    target: (RelationshipEndpointKind, &str),
    evidence: &[&str],
) -> NewAgentRelationship {
    NewAgentRelationship {
        source_kind: Some(source.0),
        source_agent_id: source.1.into(),
        source_agent_name: format!("name-{}", source.1),
        target_kind: Some(target.0),
        target_agent_id: target.1.into(),
        target_agent_name: format!("name-{}", target.1),
        relationship_type: "knows".into(),
        summary: None,
        strength: 0.5,
        confidence: 0.5,
        evidence_memory_ids: evidence.iter().map(|id| id.to_string()).collect(),
        tags: None,
        room_id: None,
        world_id: None,
        session_id: None,
    }
}

fn fact(
    subject: (RelationshipEndpointKind, &str),
    object: Option<(RelationshipEndpointKind, &str)>,
    evidence: &[&str],
) -> NewTemporalFact {
    NewTemporalFact {
        subject_kind: subject.0,
        subject_id: subject.1.into(),
        subject_name: format!("name-{}", subject.1),
        predicate: "likes".into(),
        object_kind: object.map(|o| o.0),
        object_id: object.map(|o| o.1.into()),
        object_name: object.map(|o| format!("name-{}", o.1)),
        value: if object.is_some() {
            None
        } else {
            Some("mint tea".into())
        },
        valid_from: None,
        valid_to: None,
        observed_at: Some(1_700_000_000_000),
        confidence: 0.8,
        evidence_memory_ids: evidence.iter().map(|id| id.to_string()).collect(),
        supersedes_fact_ids: Vec::new(),
        status: None,
        tags: None,
        room_id: None,
        world_id: None,
        session_id: None,
    }
}

fn temporal_relationship(
    source: (RelationshipEndpointKind, &str),
    target: (RelationshipEndpointKind, &str),
    evidence: &[&str],
) -> NewTemporalRelationship {
    NewTemporalRelationship {
        source_kind: source.0,
        source_id: source.1.into(),
        source_name: format!("name-{}", source.1),
        target_kind: target.0,
        target_id: target.1.into(),
        target_name: format!("name-{}", target.1),
        relationship_type: "trusts".into(),
        summary: None,
        strength: 0.5,
        confidence: 0.5,
        valid_from: None,
        valid_to: None,
        observed_at: Some(1_700_000_000_000),
        evidence_memory_ids: evidence.iter().map(|id| id.to_string()).collect(),
        supersedes_relationship_ids: Vec::new(),
        status: None,
        tags: None,
        room_id: None,
        world_id: None,
        session_id: None,
    }
}

fn found(manager: &MemoryManager, query: &str) -> Vec<String> {
    manager
        .search(query, MemorySearchOptions::default())
        .into_iter()
        .map(|result| result.id)
        .collect()
}

fn entity_exists(manager: &MemoryManager, kind: RelationshipEndpointKind, id: &str) -> bool {
    manager.get_entity(kind, id).is_some()
}

use RelationshipEndpointKind::{Agent, External, User};

#[test]
fn update_memory_reindexes_the_text_index() {
    let mut manager = MemoryManager::new();
    let memory = add(&mut manager, "a1", "zebracorn grazes quietly");

    let updated = manager
        .update_memory(
            &memory.id,
            MemoryPatch {
                content: Some("  quokkaroo hops loudly  ".into()),
                ..MemoryPatch::default()
            },
        )
        .unwrap()
        .expect("memory exists");

    assert_eq!(updated.id, memory.id);
    assert_eq!(updated.content, "quokkaroo hops loudly");
    assert!(found(&manager, "zebracorn").is_empty());
    assert_eq!(found(&manager, "quokkaroo"), vec![memory.id]);
}

#[test]
fn update_memory_keeps_identity_fields() {
    let mut manager = MemoryManager::new();
    let before = add(&mut manager, "a1", "original text");

    let after = manager
        .update_memory(
            &before.id,
            MemoryPatch {
                content: Some("changed text".into()),
                importance: Some(0.9),
                tags: Some(None),
            },
        )
        .unwrap()
        .unwrap();

    assert_eq!(after.id, before.id);
    assert_eq!(after.created_at, before.created_at);
    assert_eq!(after.agent_id, before.agent_id);
    assert_eq!(after.agent_name, before.agent_name);
    assert_eq!(after.memory_type, before.memory_type);
    assert_eq!(after.scope, before.scope);
    assert_eq!(after.room_id, before.room_id);
    assert_eq!(after.world_id, before.world_id);
    assert_eq!(after.session_id, before.session_id);
    assert_eq!(manager.get(&before.id), Some(after));
}

#[test]
fn update_memory_changes_importance_and_tags() {
    let mut manager = MemoryManager::new();
    let memory = add(&mut manager, "a1", "plain text");

    let replaced = manager
        .update_memory(
            &memory.id,
            MemoryPatch {
                importance: Some(0.9),
                tags: Some(Some(vec![
                    "gnuspecial".into(),
                    "gnuspecial".into(),
                    "b".into(),
                ])),
                ..MemoryPatch::default()
            },
        )
        .unwrap()
        .unwrap();
    assert_eq!(replaced.importance, 0.9);
    assert_eq!(
        replaced.tags,
        Some(vec!["gnuspecial".to_string(), "b".to_string()])
    );
    assert_eq!(found(&manager, "gnuspecial"), vec![memory.id.clone()]);

    let kept = manager
        .update_memory(&memory.id, MemoryPatch::default())
        .unwrap()
        .unwrap();
    assert_eq!(kept.tags, replaced.tags);

    let cleared = manager
        .update_memory(
            &memory.id,
            MemoryPatch {
                tags: Some(None),
                ..MemoryPatch::default()
            },
        )
        .unwrap()
        .unwrap();
    assert_eq!(cleared.tags, None);
    assert!(found(&manager, "gnuspecial").is_empty());

    let empty = manager
        .update_memory(
            &memory.id,
            MemoryPatch {
                tags: Some(Some(Vec::new())),
                ..MemoryPatch::default()
            },
        )
        .unwrap()
        .unwrap();
    assert_eq!(empty.tags, None);
}

#[test]
fn update_memory_rejects_invalid_input_without_changing_anything() {
    let mut manager = MemoryManager::new();
    let memory = add(&mut manager, "a1", "stable content");

    for importance in [1.5, f64::NAN] {
        let error = manager
            .update_memory(
                &memory.id,
                MemoryPatch {
                    importance: Some(importance),
                    ..MemoryPatch::default()
                },
            )
            .unwrap_err();
        assert_eq!(error, MemoryError::InvalidImportance);
        assert_eq!(manager.get(&memory.id), Some(memory.clone()));
    }

    let error = manager
        .update_memory(
            &memory.id,
            MemoryPatch {
                content: Some("   ".into()),
                ..MemoryPatch::default()
            },
        )
        .unwrap_err();
    assert_eq!(error, MemoryError::InvalidMemoryContent);
    assert_eq!(manager.get(&memory.id), Some(memory.clone()));

    let error = manager
        .update_memory(
            &memory.id,
            MemoryPatch {
                content: Some("valid but unsaved".into()),
                importance: Some(2.0),
                tags: Some(Some(vec!["unsaved".into()])),
            },
        )
        .unwrap_err();
    assert_eq!(error, MemoryError::InvalidImportance);
    assert_eq!(manager.get(&memory.id), Some(memory.clone()));
    assert!(found(&manager, "unsaved").is_empty());
}

#[test]
fn update_memory_for_an_unknown_id_is_none() {
    let mut manager = MemoryManager::new();
    let result = manager.update_memory(
        "missing",
        MemoryPatch {
            content: Some("x".into()),
            ..MemoryPatch::default()
        },
    );
    assert_eq!(result, Ok(None));
}

#[test]
fn an_empty_patch_returns_the_memory_unchanged() {
    let mut manager = MemoryManager::new();
    let memory = add(&mut manager, "a1", "unchanged content");
    let result = manager
        .update_memory(&memory.id, MemoryPatch::default())
        .unwrap();
    assert_eq!(result, Some(memory.clone()));
    assert_eq!(manager.get(&memory.id), Some(memory));
}

#[test]
fn delete_memory_removes_the_memory_and_its_index_entry() {
    let mut manager = MemoryManager::new();
    let memory = add(&mut manager, "a1", "ephemeral wombat note");
    add(&mut manager, "a1", "another note");
    let size = manager.size();

    let deletion = manager.delete_memory(&memory.id).expect("memory existed");

    assert_eq!(deletion, super::MemoryDeletion::default());
    assert_eq!(manager.get(&memory.id), None);
    assert!(found(&manager, "wombat").is_empty());
    assert_eq!(manager.size(), size - 1);
}

#[test]
fn delete_memory_removes_citations_from_facts_and_relationships() {
    let mut manager = MemoryManager::new();
    let a = add(&mut manager, "a1", "memory A");
    let b = add(&mut manager, "a1", "memory B");
    let kept_rel = manager
        .upsert_agent_relationship(relationship((Agent, "p"), (Agent, "q"), &[&a.id, &b.id]))
        .unwrap();
    let mut only_a = relationship((Agent, "p"), (Agent, "r"), &[&a.id]);
    only_a.relationship_type = "mentors".into();
    let removed_rel = manager.upsert_agent_relationship(only_a).unwrap();
    let fact_ab = manager
        .add_temporal_fact(fact((User, "u1"), None, &[&a.id, &b.id]))
        .unwrap();
    let fact_a = manager
        .add_temporal_fact(fact((User, "u2"), None, &[&a.id]))
        .unwrap();
    let temporal_rel = manager
        .add_temporal_relationship(temporal_relationship((Agent, "p"), (Agent, "q"), &[&a.id]))
        .unwrap();

    let deletion = manager.delete_memory(&a.id).expect("memory existed");

    assert_eq!(deletion.updated_relationship_ids, vec![kept_rel.id.clone()]);
    assert_eq!(
        deletion.removed_relationship_ids,
        vec![removed_rel.id.clone()]
    );
    let mut fact_ids = vec![fact_ab.id.clone(), fact_a.id.clone()];
    fact_ids.sort();
    assert_eq!(deletion.updated_fact_ids, fact_ids);
    assert_eq!(
        deletion.updated_temporal_relationship_ids,
        vec![temporal_rel.id.clone()]
    );

    let snapshot = manager.snapshot();
    assert_eq!(snapshot.agent_relationships.len(), 1);
    assert_eq!(snapshot.agent_relationships[0].id, kept_rel.id);
    assert_eq!(
        snapshot.agent_relationships[0].evidence_memory_ids,
        vec![b.id.clone()]
    );
    assert_eq!(
        manager
            .get_temporal_fact(&fact_ab.id)
            .unwrap()
            .evidence_memory_ids,
        vec![b.id.clone()]
    );
    assert!(manager
        .get_temporal_fact(&fact_a.id)
        .unwrap()
        .evidence_memory_ids
        .is_empty());
    assert!(manager
        .get_temporal_relationship(&temporal_rel.id)
        .unwrap()
        .evidence_memory_ids
        .is_empty());
}

#[test]
fn delete_memory_keeps_relationships_that_never_cited_it() {
    let mut manager = MemoryManager::new();
    let a = add(&mut manager, "a1", "memory A");
    let b = add(&mut manager, "a1", "memory B");
    let no_evidence = manager
        .upsert_agent_relationship(relationship((Agent, "p"), (Agent, "q"), &[]))
        .unwrap();
    let mut other = relationship((Agent, "p"), (Agent, "r"), &[&b.id]);
    other.relationship_type = "mentors".into();
    let cites_b = manager.upsert_agent_relationship(other).unwrap();

    let deletion = manager.delete_memory(&a.id).unwrap();

    assert_eq!(deletion, super::MemoryDeletion::default());
    let snapshot = manager.snapshot();
    let ids: Vec<_> = snapshot
        .agent_relationships
        .iter()
        .map(|r| r.id.clone())
        .collect();
    assert!(ids.contains(&no_evidence.id));
    assert!(ids.contains(&cites_b.id));
    let cites_b_after = snapshot
        .agent_relationships
        .iter()
        .find(|r| r.id == cites_b.id)
        .unwrap();
    assert_eq!(cites_b_after.evidence_memory_ids, vec![b.id]);
}

#[test]
fn delete_memory_for_an_unknown_id_is_none() {
    let mut manager = MemoryManager::new();
    assert_eq!(manager.delete_memory("missing"), None);
}

#[test]
fn delete_entity_removes_relationships_at_both_ends() {
    let mut manager = MemoryManager::new();
    manager
        .upsert_agent_relationship(relationship((User, "u1"), (External, "x1"), &[]))
        .unwrap();
    manager
        .upsert_agent_relationship(relationship((External, "x2"), (User, "u1"), &[]))
        .unwrap();
    let mut unrelated = relationship((External, "x1"), (External, "x2"), &[]);
    unrelated.relationship_type = "unrelated".into();
    let unrelated = manager.upsert_agent_relationship(unrelated).unwrap();
    manager
        .add_temporal_relationship(temporal_relationship((User, "u1"), (External, "x1"), &[]))
        .unwrap();
    manager
        .add_temporal_relationship(temporal_relationship((External, "x2"), (User, "u1"), &[]))
        .unwrap();
    let other_temporal = manager
        .add_temporal_relationship(temporal_relationship(
            (External, "x1"),
            (External, "x2"),
            &[],
        ))
        .unwrap();

    let deletion = manager.delete_entity(User, "u1").unwrap().expect("exists");

    assert_eq!(deletion.removed_relationship_ids.len(), 2);
    assert_eq!(deletion.removed_temporal_relationship_ids.len(), 2);
    assert!(deletion.removed_fact_ids.is_empty());
    assert!(!entity_exists(&manager, User, "u1"));
    assert!(entity_exists(&manager, External, "x1"));
    assert!(entity_exists(&manager, External, "x2"));
    let snapshot = manager.snapshot();
    assert_eq!(snapshot.agent_relationships.len(), 1);
    assert_eq!(snapshot.agent_relationships[0].id, unrelated.id);
    assert_eq!(snapshot.temporal_relationships.len(), 1);
    assert_eq!(snapshot.temporal_relationships[0].id, other_temporal.id);
}

#[test]
fn delete_entity_removes_facts_with_it_as_subject_or_object() {
    let mut manager = MemoryManager::new();
    let as_subject = manager
        .add_temporal_fact(fact((User, "u1"), None, &[]))
        .unwrap();
    let as_object = manager
        .add_temporal_fact(fact((External, "x1"), Some((User, "u1")), &[]))
        .unwrap();
    let unrelated = manager
        .add_temporal_fact(fact((External, "x1"), None, &[]))
        .unwrap();
    let mut superseding = fact((External, "x2"), None, &[]);
    superseding.supersedes_fact_ids = vec![as_subject.id.clone(), unrelated.id.clone()];
    let superseding = manager.add_temporal_fact(superseding).unwrap();

    let deletion = manager.delete_entity(User, "u1").unwrap().unwrap();

    let mut expected = vec![as_subject.id.clone(), as_object.id.clone()];
    expected.sort();
    assert_eq!(deletion.removed_fact_ids, expected);
    assert_eq!(manager.get_temporal_fact(&as_subject.id), None);
    assert_eq!(manager.get_temporal_fact(&as_object.id), None);
    assert!(manager.get_temporal_fact(&unrelated.id).is_some());
    assert_eq!(
        manager
            .get_temporal_fact(&superseding.id)
            .unwrap()
            .supersedes_fact_ids,
        vec![unrelated.id]
    );
}

#[test]
fn delete_entity_refuses_while_it_owns_memories() {
    let mut manager = MemoryManager::new();
    let first = add(&mut manager, "owner", "first memory");
    let second = add(&mut manager, "owner", "second memory");
    manager
        .upsert_agent_relationship(relationship((Agent, "owner"), (User, "u1"), &[&first.id]))
        .unwrap();
    let entities = manager.entity_count();
    let relationships = manager.relationship_count();

    let error = manager.delete_entity(Agent, "owner").unwrap_err();

    assert_eq!(error, MemoryError::EntityOwnsMemories);
    assert_eq!(manager.entity_count(), entities);
    assert_eq!(manager.relationship_count(), relationships);
    assert_eq!(manager.size(), 2);

    manager.delete_memory(&first.id).unwrap();
    manager.delete_memory(&second.id).unwrap();
    let deletion = manager.delete_entity(Agent, "owner").unwrap();
    assert!(deletion.is_some());
    assert!(!entity_exists(&manager, Agent, "owner"));
}

#[test]
fn delete_entity_survives_a_snapshot_round_trip() {
    let mut manager = MemoryManager::new();
    manager
        .upsert_agent_relationship(relationship((User, "u1"), (External, "x1"), &[]))
        .unwrap();
    manager
        .add_temporal_fact(fact((User, "u1"), None, &[]))
        .unwrap();
    manager.delete_entity(User, "u1").unwrap().unwrap();

    let mut restored = MemoryManager::new();
    restored.replace_snapshot(manager.snapshot());

    let entities = restored.list_entities(MemoryEntityOptions::default());
    assert!(entities
        .iter()
        .all(|entity| !(entity.kind == User && entity.id == "u1")));
    assert!(entities.iter().any(|entity| entity.id == "x1"));
}

#[test]
fn delete_entity_for_an_unknown_key_is_none() {
    let mut manager = MemoryManager::new();
    assert_eq!(manager.delete_entity(User, "missing"), Ok(None));
}

#[test]
fn an_entity_is_matched_by_kind_and_id() {
    let mut manager = MemoryManager::new();
    manager
        .upsert_agent_relationship(relationship((User, "same"), (Agent, "same"), &[]))
        .unwrap();
    manager
        .add_temporal_fact(fact((Agent, "same"), None, &[]))
        .unwrap();

    let user_deletion = manager.delete_entity(User, "same").unwrap().unwrap();
    assert_eq!(user_deletion.removed_relationship_ids.len(), 1);
    assert!(user_deletion.removed_fact_ids.is_empty());
    assert!(!entity_exists(&manager, User, "same"));
    assert!(entity_exists(&manager, Agent, "same"));

    let agent_deletion = manager.delete_entity(Agent, "same").unwrap().unwrap();
    assert!(agent_deletion.removed_relationship_ids.is_empty());
    assert_eq!(agent_deletion.removed_fact_ids.len(), 1);
    assert!(!entity_exists(&manager, Agent, "same"));
}
