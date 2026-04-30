use std::fs::{remove_file, write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::sleep;
use std::time::Duration;

use super::{
    MemoryManager, MemoryScope, MemorySearchOptions, MemoryType, NewMemory, RecentMemoryOptions,
};

static NEXT_TEMP_FILE_ID: AtomicU64 = AtomicU64::new(0);

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

fn temp_path(label: &str) -> std::path::PathBuf {
    let suffix = NEXT_TEMP_FILE_ID.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("anima-memory-{label}-{suffix}.json"))
}

fn add_memory(manager: &mut MemoryManager, memory: NewMemory) -> super::Memory {
    manager.add(memory).expect("memory should be added")
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
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock should be after unix epoch")
        .as_millis();
    let memory = add_memory(&mut manager, base(|_| {}));
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
fn save_writes_memories_to_json_file() {
    let path = temp_path("save");
    let _ = remove_file(&path);

    let mut manager = MemoryManager::with_storage_file(path.clone());
    add_memory(
        &mut manager,
        base(|memory| memory.content = "saved fact".into()),
    );
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
    add_memory(
        &mut manager,
        base(|memory| memory.content = "persisted memory".into()),
    );
    add_memory(
        &mut manager,
        base(|memory| {
            memory.content = "another persisted memory".into();
            memory.agent_name = "writer".into();
        }),
    );
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
    add_memory(
        &mut manager,
        base(|memory| {
            memory.content = "Nx is a build system for monorepos".into();
        }),
    );
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
    let original = add_memory(
        &mut manager,
        base(|memory| memory.content = "to be preserved".into()),
    );
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
    add_memory(&mut manager, base(|_| {}));
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
fn load_ignores_out_of_range_importance_in_json_file() {
    let path = temp_path("invalid-importance");
    let _ = remove_file(&path);
    std::fs::write(
        &path,
        r#"[{"id":"mem-1","agentId":"agent-1","agentName":"researcher","type":"fact","content":"bad importance","importance":1.1,"createdAt":123,"tags":null}]"#,
    )
    .expect("fixture written");

    let mut manager = MemoryManager::with_storage_file(path.clone());
    manager
        .load()
        .expect("load should recover from invalid importance");

    assert_eq!(manager.size(), 0, "invalid importance should not be loaded");
    let _ = remove_file(&path);
}

#[test]
fn load_is_idempotent() {
    let path = temp_path("idempotent");
    let _ = remove_file(&path);

    let mut manager = MemoryManager::with_storage_file(path.clone());
    add_memory(
        &mut manager,
        base(|memory| memory.content = "unique memory".into()),
    );
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
    add_memory(
        &mut manager,
        base(|memory| memory.content = "fact one".into()),
    );
    manager.save().expect("first save should succeed");
    add_memory(
        &mut manager,
        base(|memory| memory.content = "fact two".into()),
    );
    manager.save().expect("second save should succeed");

    let mut reloaded = MemoryManager::with_storage_file(path);
    reloaded.load().expect("load should succeed");
    assert_eq!(reloaded.size(), 2);
    let _ = remove_file(reloaded.storage_file.as_ref().expect("path should exist"));
}

#[test]
fn save_escapes_control_characters_in_json_file() {
    let path = temp_path("control-chars");
    let _ = remove_file(&path);

    let mut manager = MemoryManager::with_storage_file(path.clone());
    add_memory(
        &mut manager,
        base(|memory| {
            memory.content = format!("has{}backspace{}formfeed", '\u{0008}', '\u{000c}');
        }),
    );
    manager.save().expect("save should succeed");

    let contents = std::fs::read_to_string(&path).expect("saved file should be readable");
    assert!(
        contents.contains("\\u0008"),
        "saved JSON should escape backspace: {contents:?}"
    );
    assert!(
        contents.contains("\\u000c"),
        "saved JSON should escape form-feed: {contents:?}"
    );
    let _ = remove_file(&path);
}

#[test]
fn load_decodes_surrogate_pair_unicode_escape() {
    let path = temp_path("surrogate-pair");
    let _ = remove_file(&path);
    std::fs::write(
        &path,
        r#"[{"id":"mem-1","agentId":"agent-1","agentName":"researcher","type":"fact","content":"launch \ud83d\ude80","importance":0.8,"createdAt":123,"tags":["emoji"]}]"#,
    )
    .expect("fixture written");

    let mut manager = MemoryManager::with_storage_file(path.clone());
    manager.load().expect("load should succeed");

    let memories = manager.get_recent(RecentMemoryOptions::default());
    assert_eq!(memories.len(), 1);
    assert_eq!(memories[0].content, "launch 🚀");
    let _ = remove_file(&path);
}

#[test]
fn load_ignores_unescaped_newline_in_json_string() {
    let path = temp_path("raw-newline");
    let _ = remove_file(&path);
    std::fs::write(
        &path,
        format!(
            "[{{\"id\":\"mem-1\",\"agentId\":\"agent-1\",\"agentName\":\"researcher\",\"type\":\"fact\",\"content\":\"bad{}json\",\"importance\":0.8,\"createdAt\":123,\"tags\":null}}]",
            '\n'
        ),
    )
    .expect("fixture written");

    let mut manager = MemoryManager::with_storage_file(path.clone());
    manager
        .load()
        .expect("load should recover from malformed file");

    assert_eq!(manager.size(), 0, "malformed file should not be loaded");
    let _ = remove_file(&path);
}

#[test]
fn load_preserves_unicode_content_and_tags() {
    let path = temp_path("unicode");
    let _ = remove_file(&path);

    let mut manager = MemoryManager::with_storage_file(path.clone());
    let original = add_memory(
        &mut manager,
        base(|memory| {
            memory.agent_name = "分析者".into();
            memory.content = "Café 猫 🚀".into();
            memory.tags = Some(vec!["naïve".into(), "测试".into()]);
        }),
    );
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

    add_memory(&mut manager, base(|_| {}));
    assert_eq!(manager.summary(), "1 memories");

    add_memory(&mut manager, base(|_| {}));
    assert_eq!(manager.summary(), "2 memories");
}

#[test]
fn size_is_zero_for_fresh_instance() {
    assert_eq!(MemoryManager::new().size(), 0);
}
