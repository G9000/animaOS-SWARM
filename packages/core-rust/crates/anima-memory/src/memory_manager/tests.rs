use std::thread::sleep;
use std::time::Duration;

use super::{
    AgentRelationshipOptions, MemoryEntityOptions, MemoryError, MemoryEvaluationDecision,
    MemoryEvaluationOptions, MemoryManager, MemoryRecallOptions, MemoryRetentionPolicy,
    MemoryScope, MemorySearchOptions, MemoryType, MemoryVectorIndex, NewAgentRelationship,
    NewMemory, NewMemoryEntity, NewTemporalFact, NewTemporalRelationship, RecentMemoryOptions,
    RelationshipEndpointKind, TemporalFactOptions, TemporalRecordStatus,
    TemporalRelationshipOptions, VectorMemoryHit,
};

fn base(overrides: impl FnOnce(&mut NewMemory)) -> NewMemory {
    let mut memory = NewMemory {
        agent_id: "agent-1".into(),
        agent_name: "researcher".into(),
        memory_type: MemoryType::Fact,
        content: "TypeScript is a statically typed language".into(),
        importance: 0.5,
        tags: None,
        scope: None,
        room_id: None,
        world_id: None,
        session_id: None,
    };
    overrides(&mut memory);
    memory
}

fn add_memory(manager: &mut MemoryManager, memory: NewMemory) -> super::Memory {
    manager.add(memory).expect("memory should be added")
}

fn base_relationship(overrides: impl FnOnce(&mut NewAgentRelationship)) -> NewAgentRelationship {
    let mut relationship = NewAgentRelationship {
        source_kind: None,
        source_agent_id: "planner".into(),
        source_agent_name: "Planner".into(),
        target_kind: None,
        target_agent_id: "critic".into(),
        target_agent_name: "Critic".into(),
        relationship_type: "collaborates_with".into(),
        summary: Some("Critic pressure-tests Planner's launch assumptions.".into()),
        strength: 0.8,
        confidence: 0.7,
        evidence_memory_ids: vec!["mem-1".into()],
        tags: Some(vec!["launch".into()]),
        room_id: Some("room-1".into()),
        world_id: Some("world-1".into()),
        session_id: Some("session-1".into()),
    };
    overrides(&mut relationship);
    relationship
}

fn base_temporal_fact(overrides: impl FnOnce(&mut NewTemporalFact)) -> NewTemporalFact {
    let mut fact = NewTemporalFact {
        subject_kind: RelationshipEndpointKind::User,
        subject_id: "user-leo".into(),
        subject_name: "Leo".into(),
        predicate: "prefers_drink".into(),
        object_kind: None,
        object_id: None,
        object_name: None,
        value: Some("mint tea".into()),
        valid_from: Some(1_700_000_000_000),
        valid_to: None,
        observed_at: Some(1_700_000_000_000),
        confidence: 0.84,
        evidence_memory_ids: Vec::new(),
        supersedes_fact_ids: Vec::new(),
        status: None,
        tags: Some(vec!["preference".into()]),
        room_id: Some("room-1".into()),
        world_id: Some("world-1".into()),
        session_id: Some("session-1".into()),
    };
    overrides(&mut fact);
    fact
}

fn base_temporal_relationship(
    overrides: impl FnOnce(&mut NewTemporalRelationship),
) -> NewTemporalRelationship {
    let mut relationship = NewTemporalRelationship {
        source_kind: RelationshipEndpointKind::Agent,
        source_id: "planner".into(),
        source_name: "Planner".into(),
        target_kind: RelationshipEndpointKind::Agent,
        target_id: "critic".into(),
        target_name: "Critic".into(),
        relationship_type: "trusts_for_launch_review".into(),
        summary: Some("Planner trusts Critic for launch review.".into()),
        strength: 0.65,
        confidence: 0.72,
        valid_from: Some(1_700_000_000_000),
        valid_to: None,
        observed_at: Some(1_700_000_000_000),
        evidence_memory_ids: Vec::new(),
        supersedes_relationship_ids: Vec::new(),
        status: None,
        tags: Some(vec!["agent-agent".into()]),
        room_id: Some("room-1".into()),
        world_id: Some("world-1".into()),
        session_id: Some("session-1".into()),
    };
    overrides(&mut relationship);
    relationship
}

struct StaticVectorIndex {
    hits: Vec<VectorMemoryHit>,
}

impl MemoryVectorIndex for StaticVectorIndex {
    fn search(&self, _query: &str, limit: usize) -> Vec<VectorMemoryHit> {
        self.hits.iter().take(limit).cloned().collect()
    }
}

#[test]
fn add_assigns_unique_ids() {
    let mut manager = MemoryManager::new();
    let a = add_memory(
        &mut manager,
        base(|memory| memory.content = "fact one".into()),
    );
    let b = add_memory(
        &mut manager,
        base(|memory| memory.content = "fact two".into()),
    );

    assert!(!a.id.is_empty());
    assert!(!b.id.is_empty());
    assert_ne!(a.id, b.id);
}

#[test]
fn add_sets_created_at_to_now() {
    let mut manager = MemoryManager::new();
    let before = anima_core::primitives::now_millis();
    let memory = add_memory(&mut manager, base(|_| {}));
    let after = anima_core::primitives::now_millis();

    assert!(memory.created_at >= before);
    assert!(memory.created_at <= after);
}

#[test]
fn add_preserves_provided_fields() {
    let mut manager = MemoryManager::new();
    let memory = add_memory(
        &mut manager,
        base(|memory| {
            memory.agent_id = "a99".into();
            memory.agent_name = "writer".into();
            memory.memory_type = MemoryType::TaskResult;
            memory.content = "Task was completed successfully".into();
            memory.importance = 0.9;
            memory.tags = Some(vec!["done".into(), "verified".into()]);
            memory.scope = Some(MemoryScope::Room);
            memory.room_id = Some("room-1".into());
            memory.world_id = Some("world-1".into());
            memory.session_id = Some("session-1".into());
        }),
    );

    assert_eq!(memory.agent_id, "a99");
    assert_eq!(memory.agent_name, "writer");
    assert_eq!(memory.memory_type, MemoryType::TaskResult);
    assert_eq!(memory.content, "Task was completed successfully");
    assert_eq!(memory.importance, 0.9);
    assert_eq!(
        memory.tags,
        Some(vec!["done".to_string(), "verified".to_string()])
    );
    assert_eq!(memory.scope, MemoryScope::Room);
    assert_eq!(memory.room_id.as_deref(), Some("room-1"));
    assert_eq!(memory.world_id.as_deref(), Some("world-1"));
    assert_eq!(memory.session_id.as_deref(), Some("session-1"));
}

#[test]
fn add_defaults_room_scope_when_room_id_is_present() {
    let mut manager = MemoryManager::new();
    let memory = add_memory(
        &mut manager,
        base(|memory| memory.room_id = Some("room-1".into())),
    );

    assert_eq!(memory.scope, MemoryScope::Room);
}

#[test]
fn add_defaults_private_scope_without_room_id() {
    let mut manager = MemoryManager::new();
    let memory = add_memory(&mut manager, base(|_| {}));

    assert_eq!(memory.scope, MemoryScope::Private);
}

#[test]
fn add_rejects_non_finite_importance() {
    let mut manager = MemoryManager::new();

    for importance in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let result = manager.add(base(|memory| memory.importance = importance));
        assert!(
            result.is_err(),
            "importance {importance:?} should be rejected"
        );
    }

    assert_eq!(manager.size(), 0);
}

#[test]
fn add_rejects_out_of_range_importance() {
    let mut manager = MemoryManager::new();

    for importance in [-0.1, 1.1] {
        let result = manager.add(base(|memory| memory.importance = importance));
        assert!(
            result.is_err(),
            "importance {importance:?} should be rejected"
        );
    }

    assert_eq!(manager.size(), 0);
}

#[test]
fn add_increments_size() {
    let mut manager = MemoryManager::new();
    assert_eq!(manager.size(), 0);

    add_memory(&mut manager, base(|_| {}));
    assert_eq!(manager.size(), 1);

    add_memory(&mut manager, base(|_| {}));
    assert_eq!(manager.size(), 2);
}

#[test]
fn add_makes_memories_immediately_searchable() {
    let mut manager = MemoryManager::new();
    add_memory(
        &mut manager,
        base(|memory| {
            memory.content = "pglite is an in-process SQLite database".into();
        }),
    );

    let results = manager.search("SQLite database", MemorySearchOptions::default());
    assert!(!results.is_empty());
    assert!(results[0].content.contains("pglite"));
}

fn seeded_manager() -> MemoryManager {
    let mut manager = MemoryManager::new();
    add_memory(
        &mut manager,
        base(|memory| {
            memory.agent_id = "a1".into();
            memory.agent_name = "researcher".into();
            memory.memory_type = MemoryType::Fact;
            memory.content = "TypeScript is a statically typed superset of JavaScript".into();
            memory.importance = 0.9;
        }),
    );
    add_memory(
        &mut manager,
        base(|memory| {
            memory.agent_id = "a1".into();
            memory.agent_name = "researcher".into();
            memory.memory_type = MemoryType::Observation;
            memory.content = "React hooks simplify stateful component logic".into();
            memory.importance = 0.7;
        }),
    );
    add_memory(
        &mut manager,
        base(|memory| {
            memory.agent_id = "a2".into();
            memory.agent_name = "writer".into();
            memory.memory_type = MemoryType::Fact;
            memory.content = "BM25 is a probabilistic ranking algorithm for text search".into();
            memory.importance = 0.8;
        }),
    );
    add_memory(
        &mut manager,
        base(|memory| {
            memory.agent_id = "a2".into();
            memory.agent_name = "writer".into();
            memory.memory_type = MemoryType::TaskResult;
            memory.content = "Wrote API documentation covering 12 endpoints".into();
            memory.importance = 0.3;
        }),
    );
    add_memory(
        &mut manager,
        base(|memory| {
            memory.agent_id = "a3".into();
            memory.agent_name = "reviewer".into();
            memory.memory_type = MemoryType::Reflection;
            memory.content = "Code review revealed three potential null pointer exceptions".into();
            memory.importance = 0.6;
        }),
    );
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
fn search_filters_by_scope_and_room() {
    let mut manager = MemoryManager::new();
    add_memory(
        &mut manager,
        base(|memory| {
            memory.content = "shared planning note".into();
            memory.scope = Some(MemoryScope::Room);
            memory.room_id = Some("room-a".into());
        }),
    );
    add_memory(
        &mut manager,
        base(|memory| {
            memory.content = "shared planning note".into();
            memory.scope = Some(MemoryScope::Room);
            memory.room_id = Some("room-b".into());
        }),
    );

    let results = manager.search(
        "planning note",
        MemorySearchOptions {
            scope: Some(MemoryScope::Room),
            room_id: Some("room-a".into()),
            ..MemorySearchOptions::default()
        },
    );

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].room_id.as_deref(), Some("room-a"));
    assert_eq!(results[0].scope, MemoryScope::Room);
}

#[test]
fn upsert_entity_creates_and_merges_identity_record() {
    let mut manager = MemoryManager::new();
    let first = manager
        .upsert_entity(NewMemoryEntity {
            kind: RelationshipEndpointKind::User,
            id: "user-1".into(),
            name: "Leo".into(),
            aliases: vec!["leoca".into()],
            summary: Some("Primary playground user".into()),
        })
        .expect("entity should be created");

    let updated = manager
        .upsert_entity(NewMemoryEntity {
            kind: RelationshipEndpointKind::User,
            id: "user-1".into(),
            name: "Leo C".into(),
            aliases: vec!["leoca".into(), "g9000".into()],
            summary: None,
        })
        .expect("entity should be updated");

    assert_eq!(first.id, updated.id);
    assert_eq!(updated.kind, RelationshipEndpointKind::User);
    assert_eq!(updated.name, "Leo C");
    assert_eq!(
        updated.aliases,
        vec!["leoca".to_string(), "g9000".to_string()]
    );
    assert_eq!(updated.summary.as_deref(), Some("Primary playground user"));
    assert_eq!(manager.entity_count(), 1);
}

#[test]
fn list_entities_filters_by_kind_and_alias() {
    let mut manager = MemoryManager::new();
    manager
        .upsert_entity(NewMemoryEntity {
            kind: RelationshipEndpointKind::User,
            id: "user-1".into(),
            name: "Leo".into(),
            aliases: vec!["operator".into()],
            summary: None,
        })
        .expect("user entity should be created");
    manager
        .upsert_entity(NewMemoryEntity {
            kind: RelationshipEndpointKind::Agent,
            id: "agent-1".into(),
            name: "Planner".into(),
            aliases: vec!["operator".into()],
            summary: None,
        })
        .expect("agent entity should be created");

    let users = manager.list_entities(MemoryEntityOptions {
        kind: Some(RelationshipEndpointKind::User),
        alias: Some("operator".into()),
        ..MemoryEntityOptions::default()
    });

    assert_eq!(users.len(), 1);
    assert_eq!(users[0].id, "user-1");
}

#[test]
fn add_registers_agent_entity_without_rejecting_memory() {
    let mut manager = MemoryManager::new();
    add_memory(
        &mut manager,
        base(|memory| {
            memory.agent_id = "agent-entity".into();
            memory.agent_name = "Entity Agent".into();
        }),
    );

    let entity = manager
        .get_entity(RelationshipEndpointKind::Agent, "agent-entity")
        .expect("agent entity should be registered");

    assert_eq!(entity.name, "Entity Agent");
}

#[test]
fn evaluate_new_memory_detects_exact_duplicate() {
    let mut manager = MemoryManager::new();
    let existing = add_memory(&mut manager, base(|_| {}));

    let evaluation = manager
        .evaluate_new_memory(
            &base(|memory| memory.content = " TypeScript   is a statically typed language ".into()),
            MemoryEvaluationOptions::default(),
        )
        .expect("evaluation should succeed");

    assert_eq!(evaluation.decision, MemoryEvaluationDecision::Merge);
    assert_eq!(
        evaluation.duplicate_memory_id.as_deref(),
        Some(existing.id.as_str())
    );
}

#[test]
fn add_evaluated_ignores_low_value_short_memory() {
    let mut manager = MemoryManager::new();
    let outcome = manager
        .add_evaluated(
            base(|memory| {
                memory.content = "ok".into();
                memory.importance = 0.05;
            }),
            MemoryEvaluationOptions::default(),
        )
        .expect("evaluated add should succeed");

    assert_eq!(
        outcome.evaluation.decision,
        MemoryEvaluationDecision::Ignore
    );
    assert!(outcome.memory.is_none());
    assert_eq!(manager.size(), 0);
}

#[test]
fn add_evaluated_stores_distinct_memory_with_suggested_importance() {
    let mut manager = MemoryManager::new();
    let outcome = manager
        .add_evaluated(
            base(|memory| {
                memory.content = "The user prefers concise design review notes.".into();
                memory.importance = 0.4;
                memory.tags = Some(vec!["preference".into()]);
                memory.world_id = Some("world-1".into());
            }),
            MemoryEvaluationOptions::default(),
        )
        .expect("evaluated add should succeed");

    assert_eq!(outcome.evaluation.decision, MemoryEvaluationDecision::Store);
    let stored = outcome.memory.expect("memory should be stored");
    assert!(stored.importance > 0.4);
    assert_eq!(manager.size(), 1);
}

#[test]
fn upsert_agent_relationship_creates_agent_edge() {
    let mut manager = MemoryManager::new();

    let relationship = manager
        .upsert_agent_relationship(base_relationship(|_| {}))
        .expect("relationship should be created");

    assert!(!relationship.id.is_empty());
    assert_eq!(relationship.source_kind, RelationshipEndpointKind::Agent);
    assert_eq!(relationship.source_agent_id, "planner");
    assert_eq!(relationship.target_kind, RelationshipEndpointKind::Agent);
    assert_eq!(relationship.target_agent_id, "critic");
    assert_eq!(relationship.relationship_type, "collaborates_with");
    assert_eq!(relationship.evidence_memory_ids, vec!["mem-1".to_string()]);
    assert_eq!(manager.relationship_count(), 1);
}

#[test]
fn upsert_agent_relationship_supports_agent_user_edges() {
    let mut manager = MemoryManager::new();
    let relationship = manager
        .upsert_agent_relationship(base_relationship(|relationship| {
            relationship.target_kind = Some(RelationshipEndpointKind::User);
            relationship.target_agent_id = "user-1".into();
            relationship.target_agent_name = "User".into();
            relationship.relationship_type = "responds_to".into();
        }))
        .expect("agent-user relationship should be created");

    assert_eq!(relationship.source_kind, RelationshipEndpointKind::Agent);
    assert_eq!(relationship.target_kind, RelationshipEndpointKind::User);
    assert_eq!(relationship.target_agent_id, "user-1");

    let relationships = manager.list_agent_relationships(AgentRelationshipOptions {
        entity_id: Some("user-1".into()),
        target_kind: Some(RelationshipEndpointKind::User),
        ..AgentRelationshipOptions::default()
    });

    assert_eq!(relationships.len(), 1);
    assert_eq!(relationships[0].relationship_type, "responds_to");
}

#[test]
fn upsert_agent_relationship_registers_endpoint_entities() {
    let mut manager = MemoryManager::new();
    manager
        .upsert_agent_relationship(base_relationship(|relationship| {
            relationship.target_kind = Some(RelationshipEndpointKind::User);
            relationship.target_agent_id = "user-1".into();
            relationship.target_agent_name = "Leo".into();
        }))
        .expect("relationship should be created");

    let source = manager
        .get_entity(RelationshipEndpointKind::Agent, "planner")
        .expect("source entity should exist");
    let target = manager
        .get_entity(RelationshipEndpointKind::User, "user-1")
        .expect("target entity should exist");

    assert_eq!(source.name, "Planner");
    assert_eq!(target.name, "Leo");
}

#[test]
fn trace_memory_returns_relationship_evidence_and_entities() {
    let mut manager = MemoryManager::new();
    let memory = add_memory(
        &mut manager,
        base(|memory| {
            memory.agent_id = "planner".into();
            memory.agent_name = "Planner".into();
            memory.content = "The launch plan needs a rollback rehearsal.".into();
        }),
    );
    manager
        .upsert_agent_relationship(base_relationship(|relationship| {
            relationship.target_kind = Some(RelationshipEndpointKind::User);
            relationship.target_agent_id = "user-1".into();
            relationship.target_agent_name = "Leo".into();
            relationship.relationship_type = "responds_to".into();
            relationship.evidence_memory_ids = vec![memory.id.clone()];
        }))
        .expect("relationship should be created");

    let trace = manager
        .trace_memory(&memory.id)
        .expect("trace should exist for stored memory");

    assert_eq!(trace.memory.id, memory.id);
    assert_eq!(trace.relationships.len(), 1);
    assert_eq!(
        trace.relationships[0].target_kind,
        RelationshipEndpointKind::User
    );
    assert!(trace
        .entities
        .iter()
        .any(|entity| entity.kind == RelationshipEndpointKind::User && entity.id == "user-1"));
}

#[test]
fn temporal_fact_supersession_prefers_current_fact_by_default() {
    let mut manager = MemoryManager::new();
    let january = add_memory(
        &mut manager,
        base(|memory| {
            memory.content = "In January, Leo preferred espresso before demos.".into();
        }),
    );
    let april = add_memory(
        &mut manager,
        base(|memory| {
            memory.content = "In April, Leo switched to mint tea before demos.".into();
        }),
    );
    let old_fact = manager
        .add_temporal_fact(base_temporal_fact(|fact| {
            fact.value = Some("espresso".into());
            fact.valid_from = Some(1_704_067_200_000);
            fact.observed_at = Some(1_704_067_200_000);
            fact.evidence_memory_ids = vec![january.id.clone()];
        }))
        .expect("old fact should be added");
    let current_fact = manager
        .add_temporal_fact(base_temporal_fact(|fact| {
            fact.value = Some("mint tea".into());
            fact.valid_from = Some(1_711_923_200_000);
            fact.observed_at = Some(1_711_923_200_000);
            fact.evidence_memory_ids = vec![april.id.clone()];
            fact.supersedes_fact_ids = vec![old_fact.id.clone()];
        }))
        .expect("current fact should be added");

    let current = manager.list_temporal_facts(TemporalFactOptions {
        subject_id: Some("user-leo".into()),
        predicate: Some("prefers_drink".into()),
        valid_at: Some(1_713_000_000_000),
        ..TemporalFactOptions::default()
    });
    let historical = manager.list_temporal_facts(TemporalFactOptions {
        subject_id: Some("user-leo".into()),
        predicate: Some("prefers_drink".into()),
        valid_at: Some(1_705_000_000_000),
        include_inactive: true,
        ..TemporalFactOptions::default()
    });
    let superseded = manager
        .get_temporal_fact(&old_fact.id)
        .expect("superseded fact should still exist");

    assert_eq!(current.len(), 1);
    assert_eq!(current[0].id, current_fact.id);
    assert_eq!(current[0].value.as_deref(), Some("mint tea"));
    assert_eq!(historical.len(), 1);
    assert_eq!(historical[0].id, old_fact.id);
    assert_eq!(superseded.status, TemporalRecordStatus::Superseded);
    assert_eq!(superseded.valid_to, Some(1_711_923_200_000));
    assert_eq!(manager.temporal_fact_count(), 2);
}

#[test]
fn temporal_fact_rejects_invalid_ranges_and_empty_values() {
    let mut manager = MemoryManager::new();

    let range_error = manager
        .add_temporal_fact(base_temporal_fact(|fact| {
            fact.valid_from = Some(200);
            fact.valid_to = Some(100);
        }))
        .expect_err("invalid temporal range should fail");
    let empty_error = manager
        .add_temporal_fact(base_temporal_fact(|fact| {
            fact.object_id = None;
            fact.object_name = None;
            fact.value = None;
        }))
        .expect_err("fact without object or value should fail");

    assert_eq!(range_error, MemoryError::InvalidTemporalValidityRange);
    assert_eq!(empty_error, MemoryError::InvalidTemporalObject);
}

#[test]
fn recall_uses_active_temporal_fact_evidence() {
    let mut manager = MemoryManager::new();
    let evidence = add_memory(
        &mut manager,
        base(|memory| {
            memory.content = "In April, Leo switched to mint tea before demos.".into();
            memory.importance = 0.4;
        }),
    );
    manager
        .add_temporal_fact(base_temporal_fact(|fact| {
            fact.predicate = "prefers_drink".into();
            fact.value = Some("mint tea".into());
            fact.evidence_memory_ids = vec![evidence.id.clone()];
        }))
        .expect("temporal fact should add");

    let results = manager.recall(
        "What drink does Leo prefer before demos?",
        MemoryRecallOptions {
            lexical_limit: Some(0),
            recent_limit: Some(0),
            relationship_limit: Some(0),
            temporal_limit: Some(5),
            limit: Some(3),
            ..MemoryRecallOptions::default()
        },
    );

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].memory.id, evidence.id);
    assert!(results[0].temporal_score > 0.8);
    assert_eq!(results[0].lexical_score, 0.0);
}

#[test]
fn recall_temporal_fact_accepts_custom_intent_terms() {
    let mut manager = MemoryManager::new();
    let evidence = add_memory(
        &mut manager,
        base(|memory| {
            memory.content = "Leo switched to yerba mate before demos.".into();
            memory.importance = 0.4;
        }),
    );
    manager
        .add_temporal_fact(base_temporal_fact(|fact| {
            fact.predicate = "prefers_drink".into();
            fact.value = Some("yerba mate".into());
            fact.evidence_memory_ids = vec![evidence.id.clone()];
        }))
        .expect("temporal fact should add");

    let results = manager.recall(
        "Leo bebida",
        MemoryRecallOptions {
            lexical_limit: Some(0),
            recent_limit: Some(0),
            relationship_limit: Some(0),
            temporal_limit: Some(5),
            temporal_intent_terms: vec!["bebida".into()],
            limit: Some(3),
            ..MemoryRecallOptions::default()
        },
    );

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].memory.id, evidence.id);
    assert!(results[0].temporal_score > 0.8);
}

#[test]
fn recall_ignores_superseded_temporal_fact_evidence() {
    let mut manager = MemoryManager::new();
    let old_evidence = add_memory(
        &mut manager,
        base(|memory| {
            memory.content = "In January, Leo preferred espresso before demos.".into();
        }),
    );
    let current_evidence = add_memory(
        &mut manager,
        base(|memory| {
            memory.content = "In April, Leo switched to mint tea before demos.".into();
        }),
    );
    let old_fact = manager
        .add_temporal_fact(base_temporal_fact(|fact| {
            fact.value = Some("espresso".into());
            fact.evidence_memory_ids = vec![old_evidence.id.clone()];
        }))
        .expect("old fact should add");
    manager
        .add_temporal_fact(base_temporal_fact(|fact| {
            fact.value = Some("mint tea".into());
            fact.evidence_memory_ids = vec![current_evidence.id.clone()];
            fact.supersedes_fact_ids = vec![old_fact.id.clone()];
        }))
        .expect("current fact should add");

    let results = manager.recall(
        "What does Leo prefer before demos?",
        MemoryRecallOptions {
            lexical_limit: Some(0),
            recent_limit: Some(0),
            temporal_limit: Some(5),
            limit: Some(3),
            ..MemoryRecallOptions::default()
        },
    );

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].memory.id, current_evidence.id);
    assert!(results[0].temporal_score > 0.8);
}

#[test]
fn recall_temporal_fact_respects_context_filters() {
    let mut manager = MemoryManager::new();
    let evidence = add_memory(
        &mut manager,
        base(|memory| {
            memory.content = "Leo prefers mint tea in the private planning room.".into();
            memory.scope = Some(MemoryScope::Room);
            memory.room_id = Some("room-2".into());
        }),
    );
    manager
        .add_temporal_fact(base_temporal_fact(|fact| {
            fact.value = Some("mint tea".into());
            fact.evidence_memory_ids = vec![evidence.id];
            fact.room_id = Some("room-2".into());
        }))
        .expect("temporal fact should add");

    let results = manager.recall(
        "What drink does Leo prefer?",
        MemoryRecallOptions {
            lexical_limit: Some(0),
            recent_limit: Some(0),
            temporal_limit: Some(5),
            limit: Some(3),
            search: MemorySearchOptions {
                scope: Some(MemoryScope::Room),
                room_id: Some("room-1".into()),
                ..MemorySearchOptions::default()
            },
            ..MemoryRecallOptions::default()
        },
    );

    assert!(results.is_empty());
}

#[test]
fn recall_temporal_text_fallback_requires_subject_anchor() {
    let mut manager = MemoryManager::new();
    let leo_memory = add_memory(
        &mut manager,
        base(|memory| {
            memory.content = "Leo prefers mint tea before demos.".into();
        }),
    );
    add_memory(
        &mut manager,
        base(|memory| {
            memory.content = "Maya prefers mint tea before finance reviews.".into();
        }),
    );
    manager
        .add_temporal_fact(base_temporal_fact(|fact| {
            fact.value = Some("mint tea".into());
            fact.evidence_memory_ids = Vec::new();
        }))
        .expect("temporal fact should add");

    let results = manager.recall(
        "What drink does Leo prefer?",
        MemoryRecallOptions {
            lexical_limit: Some(0),
            recent_limit: Some(0),
            temporal_limit: Some(5),
            limit: Some(3),
            ..MemoryRecallOptions::default()
        },
    );

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].memory.id, leo_memory.id);
    assert!(results[0].temporal_score > 0.7);
}

#[test]
fn temporal_relationship_supersession_tracks_agent_relationship_evolution() {
    let mut manager = MemoryManager::new();
    let old_relationship = manager
        .add_temporal_relationship(base_temporal_relationship(|relationship| {
            relationship.summary = Some("Planner has limited confidence in Critic.".into());
            relationship.strength = 0.35;
            relationship.valid_from = Some(1_704_067_200_000);
            relationship.observed_at = Some(1_704_067_200_000);
        }))
        .expect("old relationship should be added");
    let current_relationship = manager
        .add_temporal_relationship(base_temporal_relationship(|relationship| {
            relationship.summary = Some("Planner now trusts Critic for launch review.".into());
            relationship.strength = 0.91;
            relationship.valid_from = Some(1_711_923_200_000);
            relationship.observed_at = Some(1_711_923_200_000);
            relationship.supersedes_relationship_ids = vec![old_relationship.id.clone()];
        }))
        .expect("current relationship should be added");

    let current = manager.list_temporal_relationships(TemporalRelationshipOptions {
        source_id: Some("planner".into()),
        target_id: Some("critic".into()),
        valid_at: Some(1_713_000_000_000),
        ..TemporalRelationshipOptions::default()
    });
    let superseded = manager
        .get_temporal_relationship(&old_relationship.id)
        .expect("superseded relationship should remain traceable");

    assert_eq!(current.len(), 1);
    assert_eq!(current[0].id, current_relationship.id);
    assert_eq!(current[0].strength, 0.91);
    assert_eq!(superseded.status, TemporalRecordStatus::Superseded);
    assert_eq!(superseded.valid_to, Some(1_711_923_200_000));
    assert_eq!(manager.temporal_relationship_count(), 2);
}

#[test]
fn list_agent_relationships_agent_filter_ignores_non_agent_endpoints() {
    let mut manager = MemoryManager::new();
    manager
        .upsert_agent_relationship(base_relationship(|relationship| {
            relationship.target_kind = Some(RelationshipEndpointKind::User);
            relationship.target_agent_id = "critic".into();
            relationship.target_agent_name = "Critic User".into();
            relationship.relationship_type = "responds_to".into();
        }))
        .expect("agent-user relationship should be created");

    let agent_relationships = manager.list_agent_relationships(AgentRelationshipOptions {
        agent_id: Some("critic".into()),
        ..AgentRelationshipOptions::default()
    });
    let entity_relationships = manager.list_agent_relationships(AgentRelationshipOptions {
        entity_id: Some("critic".into()),
        target_kind: Some(RelationshipEndpointKind::User),
        ..AgentRelationshipOptions::default()
    });

    assert!(agent_relationships.is_empty());
    assert_eq!(entity_relationships.len(), 1);
}

#[test]
fn upsert_agent_relationship_rejects_blank_endpoint_ids() {
    let mut manager = MemoryManager::new();
    let error = manager
        .upsert_agent_relationship(base_relationship(|relationship| {
            relationship.source_agent_id = "  ".into();
        }))
        .expect_err("blank endpoint should be rejected");

    assert_eq!(error, MemoryError::InvalidRelationshipEndpoint);
}

#[test]
fn upsert_agent_relationship_merges_existing_edge_evidence() {
    let mut manager = MemoryManager::new();
    let first = manager
        .upsert_agent_relationship(base_relationship(|_| {}))
        .expect("relationship should be created");

    let updated = manager
        .upsert_agent_relationship(base_relationship(|relationship| {
            relationship.summary = Some("Critic is the right reviewer before launch.".into());
            relationship.strength = 0.95;
            relationship.confidence = 0.9;
            relationship.evidence_memory_ids = vec!["mem-1".into(), "mem-2".into()];
            relationship.tags = Some(vec!["launch".into(), "review".into()]);
        }))
        .expect("relationship should update");

    assert_eq!(updated.id, first.id);
    assert_eq!(
        updated.summary.as_deref(),
        Some("Critic is the right reviewer before launch.")
    );
    assert_eq!(updated.strength, 0.95);
    assert_eq!(updated.confidence, 0.9);
    assert_eq!(
        updated.evidence_memory_ids,
        vec!["mem-1".to_string(), "mem-2".to_string()]
    );
    assert_eq!(
        updated.tags,
        Some(vec!["launch".to_string(), "review".to_string()])
    );
    assert_eq!(manager.relationship_count(), 1);
}

#[test]
fn list_agent_relationships_filters_by_agent_and_world() {
    let mut manager = MemoryManager::new();
    manager
        .upsert_agent_relationship(base_relationship(|_| {}))
        .expect("relationship should be created");
    manager
        .upsert_agent_relationship(base_relationship(|relationship| {
            relationship.source_agent_id = "writer".into();
            relationship.source_agent_name = "Writer".into();
            relationship.target_agent_id = "researcher".into();
            relationship.target_agent_name = "Researcher".into();
            relationship.world_id = Some("world-2".into());
            relationship.strength = 0.4;
        }))
        .expect("second relationship should be created");

    let relationships = manager.list_agent_relationships(AgentRelationshipOptions {
        agent_id: Some("critic".into()),
        world_id: Some("world-1".into()),
        min_strength: Some(0.5),
        ..AgentRelationshipOptions::default()
    });

    assert_eq!(relationships.len(), 1);
    assert_eq!(relationships[0].source_agent_id, "planner");
    assert_eq!(relationships[0].target_agent_id, "critic");
}

#[test]
fn clear_with_agent_id_removes_agent_relationships() {
    let mut manager = MemoryManager::new();
    manager
        .upsert_agent_relationship(base_relationship(|_| {}))
        .expect("relationship should be created");

    manager.clear(Some("critic"));

    assert_eq!(manager.relationship_count(), 0);
}

#[test]
fn apply_retention_removes_low_importance_memory_and_orphan_relationship() {
    let mut manager = MemoryManager::new();
    let memory = add_memory(
        &mut manager,
        base(|memory| {
            memory.agent_id = "planner".into();
            memory.agent_name = "Planner".into();
            memory.importance = 0.1;
        }),
    );
    manager
        .upsert_agent_relationship(base_relationship(|relationship| {
            relationship.evidence_memory_ids = vec![memory.id.clone()];
        }))
        .expect("relationship should be created");

    let report = manager
        .apply_retention(MemoryRetentionPolicy {
            min_importance: Some(0.2),
            ..MemoryRetentionPolicy::default()
        })
        .expect("retention should succeed");

    assert_eq!(report.removed_memory_ids, vec![memory.id]);
    assert_eq!(report.removed_relationship_ids.len(), 1);
    assert_eq!(manager.size(), 0);
    assert_eq!(manager.relationship_count(), 0);
}

#[test]
fn apply_retention_decays_old_memory_importance() {
    let mut manager = MemoryManager::new();
    let memory = add_memory(
        &mut manager,
        base(|memory| {
            memory.content = "Durable but fading memory".into();
            memory.importance = 0.8;
        }),
    );

    let report = manager
        .apply_retention_at(
            MemoryRetentionPolicy {
                decay_half_life_millis: Some(1),
                ..MemoryRetentionPolicy::default()
            },
            memory.created_at + 1,
        )
        .expect("retention should succeed");
    let retained = manager.get(&memory.id).expect("memory should remain");

    assert_eq!(report.decayed_memories.len(), 1);
    assert!(retained.importance < 0.8);
    assert!(retained.importance > 0.0);
}

#[test]
fn recall_uses_relationship_evidence_without_keyword_match() {
    let mut manager = MemoryManager::new();
    let memory = add_memory(
        &mut manager,
        base(|memory| {
            memory.agent_id = "planner".into();
            memory.agent_name = "Planner".into();
            memory.content = "Launch checklist requires a rollback rehearsal.".into();
            memory.importance = 0.7;
            memory.world_id = Some("world-1".into());
        }),
    );
    manager
        .upsert_agent_relationship(base_relationship(|relationship| {
            relationship.target_kind = Some(RelationshipEndpointKind::User);
            relationship.target_agent_id = "user-1".into();
            relationship.target_agent_name = "Leo".into();
            relationship.relationship_type = "responds_to".into();
            relationship.evidence_memory_ids = vec![memory.id.clone()];
            relationship.world_id = Some("world-1".into());
        }))
        .expect("relationship should be created");

    let results = manager.recall(
        "unrelated query text",
        MemoryRecallOptions {
            entity_id: Some("user-1".into()),
            recent_limit: Some(0),
            limit: Some(5),
            search: MemorySearchOptions {
                world_id: Some("world-1".into()),
                ..MemorySearchOptions::default()
            },
            ..MemoryRecallOptions::default()
        },
    );

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].memory.id, memory.id);
    assert!(results[0].relationship_score > 0.5);
}

#[test]
fn recall_with_vector_index_can_retrieve_semantic_hit() {
    let mut manager = MemoryManager::new();
    add_memory(
        &mut manager,
        base(|memory| memory.content = "Lexical only note about TypeScript".into()),
    );
    let vector_memory = add_memory(
        &mut manager,
        base(|memory| {
            memory.content = "User likes quiet operational dashboards".into();
            memory.importance = 0.8;
        }),
    );
    let vector_index = StaticVectorIndex {
        hits: vec![VectorMemoryHit {
            memory_id: vector_memory.id.clone(),
            score: 0.92,
        }],
    };

    let results = manager.recall_with_vector_index(
        "semantic design preference",
        MemoryRecallOptions {
            recent_limit: Some(0),
            limit: Some(3),
            ..MemoryRecallOptions::default()
        },
        Some(&vector_index),
    );

    assert_eq!(results[0].memory.id, vector_memory.id);
    assert_eq!(results[0].vector_score, 1.0);
}

#[test]
fn recall_prioritizes_exact_lexical_evidence_over_noisy_vector_hit() {
    let mut manager = MemoryManager::new();
    let exact_memory = add_memory(
        &mut manager,
        base(|memory| {
            memory.content =
                "Melanie's kids love nature. The kids love forest hikes and nature views.".into();
            memory.importance = 0.55;
        }),
    );
    let noisy_vector_memory = add_memory(
        &mut manager,
        base(|memory| {
            memory.content = "Melanie likes learning about animals at the museum.".into();
            memory.importance = 0.55;
        }),
    );
    let vector_index = StaticVectorIndex {
        hits: vec![VectorMemoryHit {
            memory_id: noisy_vector_memory.id,
            score: 1.0,
        }],
    };

    let results = manager.recall_with_vector_index(
        "What nature and forest things do Melanie's kids love?",
        MemoryRecallOptions {
            recent_limit: Some(0),
            limit: Some(2),
            ..MemoryRecallOptions::default()
        },
        Some(&vector_index),
    );

    assert_eq!(results[0].memory.id, exact_memory.id);
    assert!(results[0].lexical_score > results[1].lexical_score);
    assert_eq!(results[1].vector_score, 1.0);
}

#[test]
fn get_recent_returns_newest_first() {
    let mut manager = MemoryManager::new();
    add_memory(
        &mut manager,
        base(|memory| memory.content = "oldest".into()),
    );
    sleep(Duration::from_millis(10));
    add_memory(
        &mut manager,
        base(|memory| memory.content = "middle".into()),
    );
    sleep(Duration::from_millis(10));
    add_memory(
        &mut manager,
        base(|memory| memory.content = "newest".into()),
    );

    let recent = manager.get_recent(RecentMemoryOptions::default());
    assert_eq!(recent[0].content, "newest");
    assert_eq!(recent[1].content, "middle");
    assert_eq!(recent[2].content, "oldest");
}

#[test]
fn get_recent_breaks_same_timestamp_ties_by_insertion_order() {
    let mut manager = MemoryManager::new();
    let first = add_memory(&mut manager, base(|memory| memory.content = "first".into()));
    let second = add_memory(
        &mut manager,
        base(|memory| memory.content = "second".into()),
    );

    manager
        .memories
        .get_mut(&first.id)
        .expect("first memory should exist")
        .created_at = 100;
    manager
        .memories
        .get_mut(&second.id)
        .expect("second memory should exist")
        .created_at = 100;

    let recent = manager.get_recent(RecentMemoryOptions::default());

    assert_eq!(recent[0].content, "second");
    assert_eq!(recent[1].content, "first");
}

#[test]
fn get_recent_respects_limit() {
    let mut manager = MemoryManager::new();
    add_memory(&mut manager, base(|memory| memory.content = "a".into()));
    add_memory(&mut manager, base(|memory| memory.content = "b".into()));
    add_memory(&mut manager, base(|memory| memory.content = "c".into()));
    add_memory(&mut manager, base(|memory| memory.content = "d".into()));

    let recent = manager.get_recent(RecentMemoryOptions {
        limit: Some(2),
        ..RecentMemoryOptions::default()
    });

    assert_eq!(recent.len(), 2);
}

#[test]
fn get_recent_filters_by_agent_id() {
    let mut manager = MemoryManager::new();
    add_memory(
        &mut manager,
        base(|memory| {
            memory.agent_id = "a1".into();
            memory.agent_name = "agent-a".into();
            memory.content = "a1 memory".into();
        }),
    );
    add_memory(
        &mut manager,
        base(|memory| {
            memory.agent_id = "a2".into();
            memory.agent_name = "agent-b".into();
            memory.content = "a2 memory".into();
        }),
    );
    add_memory(
        &mut manager,
        base(|memory| {
            memory.agent_id = "a1".into();
            memory.agent_name = "agent-a".into();
            memory.content = "a1 again".into();
        }),
    );

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
    add_memory(
        &mut manager,
        base(|memory| {
            memory.agent_name = "researcher".into();
            memory.content = "research memory".into();
        }),
    );
    add_memory(
        &mut manager,
        base(|memory| {
            memory.agent_name = "writer".into();
            memory.content = "writing memory".into();
        }),
    );
    add_memory(
        &mut manager,
        base(|memory| {
            memory.agent_name = "researcher".into();
            memory.content = "more research".into();
        }),
    );

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
fn get_recent_filters_by_session_id() {
    let mut manager = MemoryManager::new();
    add_memory(
        &mut manager,
        base(|memory| {
            memory.content = "older session".into();
            memory.session_id = Some("session-a".into());
        }),
    );
    add_memory(
        &mut manager,
        base(|memory| {
            memory.content = "newer session".into();
            memory.session_id = Some("session-b".into());
        }),
    );

    let recent = manager.get_recent(RecentMemoryOptions {
        session_id: Some("session-a".into()),
        ..RecentMemoryOptions::default()
    });

    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].content, "older session");
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
    let memory = add_memory(
        &mut manager,
        base(|memory| memory.content = "temporary fact".into()),
    );

    assert_eq!(manager.size(), 1);
    manager.forget(&memory.id);
    assert_eq!(manager.size(), 0);
}

#[test]
fn forget_removes_memory_from_search_index() {
    let mut manager = MemoryManager::new();
    let memory = add_memory(
        &mut manager,
        base(|memory| {
            memory.content = "pglite is an in-process database".into();
        }),
    );

    manager.forget(&memory.id);
    let results = manager.search("pglite in-process database", MemorySearchOptions::default());
    assert!(results.is_empty());
}

#[test]
fn forget_leaves_other_memories_intact() {
    let mut manager = MemoryManager::new();
    let a = add_memory(
        &mut manager,
        base(|memory| memory.content = "memory A about TypeScript".into()),
    );
    add_memory(
        &mut manager,
        base(|memory| memory.content = "memory B about React".into()),
    );
    manager.forget(&a.id);

    assert_eq!(manager.size(), 1);
    let results = manager.search("React", MemorySearchOptions::default());
    assert_eq!(results.len(), 1);
    assert!(results[0].content.contains("React"));
}

#[test]
fn forget_is_a_noop_for_unknown_id() {
    let mut manager = MemoryManager::new();
    add_memory(&mut manager, base(|_| {}));

    manager.forget("non-existent-id");
    assert_eq!(manager.size(), 1);
}

#[test]
fn clear_without_agent_id_clears_everything() {
    let mut manager = MemoryManager::new();
    add_memory(
        &mut manager,
        base(|memory| {
            memory.agent_id = "a1".into();
            memory.agent_name = "agent-a".into();
            memory.content = "agent A fact 1".into();
        }),
    );
    add_memory(
        &mut manager,
        base(|memory| {
            memory.agent_id = "a1".into();
            memory.agent_name = "agent-a".into();
            memory.content = "agent A fact 2".into();
        }),
    );
    add_memory(
        &mut manager,
        base(|memory| {
            memory.agent_id = "a2".into();
            memory.agent_name = "agent-b".into();
            memory.content = "agent B fact".into();
        }),
    );

    manager.clear(None);
    assert_eq!(manager.size(), 0);
    assert!(manager
        .search("fact", MemorySearchOptions::default())
        .is_empty());
}

#[test]
fn clear_with_agent_id_only_clears_that_agent() {
    let mut manager = MemoryManager::new();
    add_memory(
        &mut manager,
        base(|memory| {
            memory.agent_id = "a1".into();
            memory.agent_name = "agent-a".into();
            memory.content = "agent A fact 1".into();
        }),
    );
    add_memory(
        &mut manager,
        base(|memory| {
            memory.agent_id = "a1".into();
            memory.agent_name = "agent-a".into();
            memory.content = "agent A fact 2".into();
        }),
    );
    add_memory(
        &mut manager,
        base(|memory| {
            memory.agent_id = "a2".into();
            memory.agent_name = "agent-b".into();
            memory.content = "agent B fact".into();
        }),
    );

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
    add_memory(
        &mut manager,
        base(|memory| {
            memory.agent_id = "a1".into();
            memory.agent_name = "agent-a".into();
            memory.content = "agent A fact 1".into();
        }),
    );
    add_memory(
        &mut manager,
        base(|memory| {
            memory.agent_id = "a1".into();
            memory.agent_name = "agent-a".into();
            memory.content = "agent A fact 2".into();
        }),
    );
    add_memory(
        &mut manager,
        base(|memory| {
            memory.agent_id = "a2".into();
            memory.agent_name = "agent-b".into();
            memory.content = "agent B fact".into();
        }),
    );

    manager.clear(Some("a1"));
    let results = manager.search("agent B fact", MemorySearchOptions::default());
    assert!(!results.is_empty());
    assert!(results.iter().all(|result| result.agent_id == "a2"));
}

#[test]
fn summary_reflects_current_count_and_keeps_plural_bug_for_one() {
    let mut manager = MemoryManager::new();
    assert_eq!(manager.summary(), "0 memories");
    assert_ne!(manager.summary(), "1 memory");

    add_memory(&mut manager, base(|_| {}));
    assert_eq!(manager.summary(), "1 memories");

    add_memory(&mut manager, base(|_| {}));
    assert_eq!(manager.summary(), "2 memories");
}

#[test]
fn size_is_zero_for_fresh_instance() {
    assert_eq!(MemoryManager::new().size(), 0);
}
