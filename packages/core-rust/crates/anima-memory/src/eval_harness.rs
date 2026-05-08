use crate::memory_manager::{
    MemoryEvaluationDecision, MemoryEvaluationOptions, MemoryManager, MemoryRecallOptions,
    MemoryRecallResult, MemoryRetentionPolicy, MemoryScope, MemorySearchOptions, MemoryType,
    MemoryVectorIndex, NewAgentRelationship, NewMemory, RecentMemoryOptions,
    RelationshipEndpointKind, VectorMemoryHit,
};

#[derive(Clone, Debug, PartialEq)]
pub struct MemoryEvalCase {
    pub name: String,
    pub seed_memories: Vec<NewMemory>,
    pub seed_relationships: Vec<MemoryEvalRelationshipSeed>,
    pub checks: Vec<MemoryEvalCheck>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MemoryEvalRelationshipSeed {
    pub relationship: NewAgentRelationship,
    pub evidence_content_contains: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MemoryEvalVectorHitSeed {
    pub memory_content_contains: String,
    pub score: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MemoryEvalCheck {
    EvaluateDecision {
        name: String,
        candidate: NewMemory,
        options: MemoryEvaluationOptions,
        expected_decision: MemoryEvaluationDecision,
    },
    RecallTopN {
        name: String,
        query: String,
        options: MemoryRecallOptions,
        expected_content_contains: String,
        top_n: usize,
        require_relationship_score: bool,
    },
    RecallExcludesTopN {
        name: String,
        query: String,
        options: MemoryRecallOptions,
        excluded_content_contains: String,
        top_n: usize,
    },
    VectorRecallTopN {
        name: String,
        query: String,
        options: MemoryRecallOptions,
        vector_hits: Vec<MemoryEvalVectorHitSeed>,
        expected_content_contains: String,
        top_n: usize,
    },
    VectorRecallExcludesTopN {
        name: String,
        query: String,
        options: MemoryRecallOptions,
        vector_hits: Vec<MemoryEvalVectorHitSeed>,
        excluded_content_contains: String,
        top_n: usize,
    },
    Trace {
        name: String,
        memory_content_contains: String,
        min_relationships: usize,
        required_entity_id: Option<String>,
    },
    Retention {
        name: String,
        policy: MemoryRetentionPolicy,
        removed_content_contains: Vec<String>,
        retained_content_contains: Vec<String>,
    },
    RetentionDecay {
        name: String,
        policy: MemoryRetentionPolicy,
        elapsed_millis: u64,
        decayed_content_contains: String,
        retained_content_contains: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct MemoryEvalReport {
    pub cases: Vec<MemoryEvalCaseResult>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MemoryEvalCaseResult {
    pub name: String,
    pub checks: Vec<MemoryEvalCheckResult>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MemoryEvalCheckResult {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

impl MemoryEvalReport {
    pub fn passed(&self) -> bool {
        self.cases
            .iter()
            .all(|case| case.checks.iter().all(|check| check.passed))
    }

    pub fn total_checks(&self) -> usize {
        self.cases.iter().map(|case| case.checks.len()).sum()
    }

    pub fn passed_checks(&self) -> usize {
        self.cases
            .iter()
            .flat_map(|case| case.checks.iter())
            .filter(|check| check.passed)
            .count()
    }

    pub fn failure_messages(&self) -> Vec<String> {
        self.cases
            .iter()
            .flat_map(|case| {
                case.checks
                    .iter()
                    .filter(|check| !check.passed)
                    .map(|check| format!("{} / {}: {}", case.name, check.name, check.detail))
            })
            .collect()
    }
}

pub fn run_memory_eval_cases(cases: &[MemoryEvalCase]) -> MemoryEvalReport {
    MemoryEvalReport {
        cases: cases.iter().map(run_memory_eval_case).collect(),
    }
}

pub fn run_memory_eval_checks(
    name: impl Into<String>,
    manager: &mut MemoryManager,
    checks: &[MemoryEvalCheck],
) -> MemoryEvalCaseResult {
    MemoryEvalCaseResult {
        name: name.into(),
        checks: checks
            .iter()
            .map(|check| run_check(manager, check))
            .collect(),
    }
}

pub fn baseline_memory_eval_cases() -> Vec<MemoryEvalCase> {
    vec![
        relationship_recall_case(),
        agent_handoff_case(),
        room_world_isolation_case(),
        vector_recall_case(),
        evaluated_write_case(),
        retention_case(),
        decay_case(),
    ]
}

fn run_memory_eval_case(case: &MemoryEvalCase) -> MemoryEvalCaseResult {
    let mut manager = MemoryManager::new();
    let mut setup_checks = Vec::new();

    for memory in &case.seed_memories {
        if let Err(error) = manager.add(memory.clone()) {
            setup_checks.push(MemoryEvalCheckResult::fail(
                "setup memory",
                format!("failed to add seed memory: {}", error.message()),
            ));
        }
    }

    for seed in &case.seed_relationships {
        let mut relationship = seed.relationship.clone();
        for expected in &seed.evidence_content_contains {
            match find_memory_id_by_content(&manager, expected) {
                Some(memory_id) => relationship.evidence_memory_ids.push(memory_id),
                None => setup_checks.push(MemoryEvalCheckResult::fail(
                    "setup relationship evidence",
                    format!("no seed memory content contained {expected:?}"),
                )),
            }
        }

        if setup_checks.iter().all(|check| check.passed) {
            if let Err(error) = manager.upsert_agent_relationship(relationship) {
                setup_checks.push(MemoryEvalCheckResult::fail(
                    "setup relationship",
                    format!("failed to add seed relationship: {}", error.message()),
                ));
            }
        }
    }

    let mut result = run_memory_eval_checks(case.name.clone(), &mut manager, &case.checks);
    if !setup_checks.is_empty() {
        setup_checks.extend(result.checks);
        result.checks = setup_checks;
    }
    result
}

fn run_check(manager: &mut MemoryManager, check: &MemoryEvalCheck) -> MemoryEvalCheckResult {
    match check {
        MemoryEvalCheck::EvaluateDecision {
            name,
            candidate,
            options,
            expected_decision,
        } => match manager.add_evaluated(candidate.clone(), options.clone()) {
            Ok(outcome) if outcome.evaluation.decision == *expected_decision => {
                MemoryEvalCheckResult::pass(
                    name,
                    format!("decision was {:?}", outcome.evaluation.decision),
                )
            }
            Ok(outcome) => MemoryEvalCheckResult::fail(
                name,
                format!(
                    "expected {:?}, got {:?}: {}",
                    expected_decision, outcome.evaluation.decision, outcome.evaluation.reason
                ),
            ),
            Err(error) => {
                MemoryEvalCheckResult::fail(name, format!("evaluation failed: {}", error.message()))
            }
        },
        MemoryEvalCheck::RecallTopN {
            name,
            query,
            options,
            expected_content_contains,
            top_n,
            require_relationship_score,
        } => {
            let results = manager.recall(query, options.clone());
            assert_recall_top_n(
                name,
                &results,
                expected_content_contains,
                *top_n,
                *require_relationship_score,
                false,
            )
        }
        MemoryEvalCheck::RecallExcludesTopN {
            name,
            query,
            options,
            excluded_content_contains,
            top_n,
        } => {
            let results = manager.recall(query, options.clone());
            assert_recall_excludes_top_n(name, &results, excluded_content_contains, *top_n)
        }
        MemoryEvalCheck::VectorRecallTopN {
            name,
            query,
            options,
            vector_hits,
            expected_content_contains,
            top_n,
        } => {
            let mut hits = Vec::new();
            for hit in vector_hits {
                let Some(memory_id) =
                    find_memory_id_by_content(manager, &hit.memory_content_contains)
                else {
                    return MemoryEvalCheckResult::fail(
                        name,
                        format!(
                            "no vector seed memory content contained {:?}",
                            hit.memory_content_contains
                        ),
                    );
                };
                hits.push(VectorMemoryHit {
                    memory_id,
                    score: hit.score,
                });
            }
            let vector_index = SeededMemoryVectorIndex { hits };
            let results =
                manager.recall_with_vector_index(query, options.clone(), Some(&vector_index));
            assert_recall_top_n(
                name,
                &results,
                expected_content_contains,
                *top_n,
                false,
                true,
            )
        }
        MemoryEvalCheck::VectorRecallExcludesTopN {
            name,
            query,
            options,
            vector_hits,
            excluded_content_contains,
            top_n,
        } => {
            let Some(vector_index) = seeded_vector_index(manager, vector_hits) else {
                return MemoryEvalCheckResult::fail(
                    name,
                    "one or more vector seed memories could not be resolved".to_string(),
                );
            };
            let results =
                manager.recall_with_vector_index(query, options.clone(), Some(&vector_index));
            let top_n = (*top_n).max(1);
            if results
                .iter()
                .take(top_n)
                .any(|result| result.memory.content.contains(excluded_content_contains))
            {
                return MemoryEvalCheckResult::fail(
                    name,
                    format!(
                        "excluded content {:?} appeared in top {top_n}: {:?}",
                        excluded_content_contains,
                        results
                            .iter()
                            .take(top_n)
                            .map(|result| result.memory.content.as_str())
                            .collect::<Vec<_>>()
                    ),
                );
            }
            MemoryEvalCheckResult::pass(name, format!("excluded content stayed out of top {top_n}"))
        }
        MemoryEvalCheck::Trace {
            name,
            memory_content_contains,
            min_relationships,
            required_entity_id,
        } => {
            let Some(memory_id) = find_memory_id_by_content(manager, memory_content_contains)
            else {
                return MemoryEvalCheckResult::fail(
                    name,
                    format!("no memory content contained {memory_content_contains:?}"),
                );
            };
            let Some(trace) = manager.trace_memory(&memory_id) else {
                return MemoryEvalCheckResult::fail(name, "trace was missing".to_string());
            };
            if trace.relationships.len() < *min_relationships {
                return MemoryEvalCheckResult::fail(
                    name,
                    format!(
                        "expected at least {min_relationships} relationships, got {}",
                        trace.relationships.len()
                    ),
                );
            }
            if let Some(entity_id) = required_entity_id {
                if !trace.entities.iter().any(|entity| entity.id == *entity_id) {
                    return MemoryEvalCheckResult::fail(
                        name,
                        format!("trace did not include entity {entity_id:?}"),
                    );
                }
            }
            MemoryEvalCheckResult::pass(
                name,
                format!(
                    "trace had {} relationships and {} entities",
                    trace.relationships.len(),
                    trace.entities.len()
                ),
            )
        }
        MemoryEvalCheck::Retention {
            name,
            policy,
            removed_content_contains,
            retained_content_contains,
        } => {
            let before = all_memories(manager);
            let report = match manager.apply_retention(policy.clone()) {
                Ok(report) => report,
                Err(error) => {
                    return MemoryEvalCheckResult::fail(
                        name,
                        format!("retention failed: {}", error.message()),
                    )
                }
            };

            for expected in removed_content_contains {
                let Some(memory) = before
                    .iter()
                    .find(|memory| memory.content.contains(expected))
                else {
                    return MemoryEvalCheckResult::fail(
                        name,
                        format!("no pre-retention memory contained {expected:?}"),
                    );
                };
                if !report.removed_memory_ids.iter().any(|id| id == &memory.id) {
                    return MemoryEvalCheckResult::fail(
                        name,
                        format!("memory {:?} was not removed", memory.content),
                    );
                }
            }

            for expected in retained_content_contains {
                let retained = all_memories(manager)
                    .iter()
                    .any(|memory| memory.content.contains(expected));
                if !retained {
                    return MemoryEvalCheckResult::fail(
                        name,
                        format!("expected retained memory containing {expected:?}"),
                    );
                }
            }

            MemoryEvalCheckResult::pass(
                name,
                format!(
                    "removed {} memories and {} relationships",
                    report.removed_memory_ids.len(),
                    report.removed_relationship_ids.len()
                ),
            )
        }
        MemoryEvalCheck::RetentionDecay {
            name,
            policy,
            elapsed_millis,
            decayed_content_contains,
            retained_content_contains,
        } => {
            let before = all_memories(manager);
            let Some(decayed_memory) = before
                .iter()
                .find(|memory| memory.content.contains(decayed_content_contains))
            else {
                return MemoryEvalCheckResult::fail(
                    name,
                    format!("no pre-retention memory contained {decayed_content_contains:?}"),
                );
            };
            let now = before
                .iter()
                .map(|memory| memory.created_at)
                .max()
                .unwrap_or_default()
                .saturating_add(*elapsed_millis);
            let report = match manager.apply_retention_at(policy.clone(), now) {
                Ok(report) => report,
                Err(error) => {
                    return MemoryEvalCheckResult::fail(
                        name,
                        format!("decay retention failed: {}", error.message()),
                    )
                }
            };
            if !report
                .decayed_memories
                .iter()
                .any(|adjustment| adjustment.memory_id == decayed_memory.id)
            {
                return MemoryEvalCheckResult::fail(
                    name,
                    format!("memory {:?} was not decayed", decayed_memory.content),
                );
            }
            let retained = all_memories(manager)
                .iter()
                .any(|memory| memory.content.contains(retained_content_contains));
            if !retained {
                return MemoryEvalCheckResult::fail(
                    name,
                    format!("expected retained memory containing {retained_content_contains:?}"),
                );
            }
            MemoryEvalCheckResult::pass(
                name,
                format!("decayed {} memories", report.decayed_memories.len()),
            )
        }
    }
}

fn assert_recall_top_n(
    name: &str,
    results: &[MemoryRecallResult],
    expected_content_contains: &str,
    top_n: usize,
    require_relationship_score: bool,
    require_vector_score: bool,
) -> MemoryEvalCheckResult {
    let top_n = top_n.max(1);
    let matching = results
        .iter()
        .take(top_n)
        .find(|result| result.memory.content.contains(expected_content_contains));
    match matching {
        Some(result)
            if (!require_relationship_score || result.relationship_score > 0.0)
                && (!require_vector_score || result.vector_score > 0.0) =>
        {
            MemoryEvalCheckResult::pass(
                name,
                format!(
                    "found {:?} with score {:.3}",
                    result.memory.content, result.score
                ),
            )
        }
        Some(result) if require_relationship_score && result.relationship_score <= 0.0 => {
            MemoryEvalCheckResult::fail(
                name,
                format!(
                    "found expected memory but relationship_score was {:.3}",
                    result.relationship_score
                ),
            )
        }
        Some(result) if require_vector_score => MemoryEvalCheckResult::fail(
            name,
            format!(
                "found expected memory but vector_score was {:.3}",
                result.vector_score
            ),
        ),
        Some(result) => MemoryEvalCheckResult::fail(
            name,
            format!(
                "found expected memory but required score gates failed: relationship={:.3}, vector={:.3}",
                result.relationship_score, result.vector_score
            ),
        ),
        None => MemoryEvalCheckResult::fail(
            name,
            format!(
                "expected top {top_n} recall to contain {:?}; got {:?}",
                expected_content_contains,
                results
                    .iter()
                    .take(top_n)
                    .map(|result| result.memory.content.as_str())
                    .collect::<Vec<_>>()
            ),
        ),
    }
}

fn assert_recall_excludes_top_n(
    name: &str,
    results: &[MemoryRecallResult],
    excluded_content_contains: &str,
    top_n: usize,
) -> MemoryEvalCheckResult {
    let top_n = top_n.max(1);
    if results
        .iter()
        .take(top_n)
        .any(|result| result.memory.content.contains(excluded_content_contains))
    {
        return MemoryEvalCheckResult::fail(
            name,
            format!(
                "excluded content {:?} appeared in top {top_n}: {:?}",
                excluded_content_contains,
                results
                    .iter()
                    .take(top_n)
                    .map(|result| result.memory.content.as_str())
                    .collect::<Vec<_>>()
            ),
        );
    }
    MemoryEvalCheckResult::pass(name, format!("excluded content stayed out of top {top_n}"))
}

struct SeededMemoryVectorIndex {
    hits: Vec<VectorMemoryHit>,
}

impl MemoryVectorIndex for SeededMemoryVectorIndex {
    fn search(&self, _query: &str, limit: usize) -> Vec<VectorMemoryHit> {
        self.hits.iter().take(limit).cloned().collect()
    }
}

fn seeded_vector_index(
    manager: &MemoryManager,
    vector_hits: &[MemoryEvalVectorHitSeed],
) -> Option<SeededMemoryVectorIndex> {
    let mut hits = Vec::new();
    for hit in vector_hits {
        let memory_id = find_memory_id_by_content(manager, &hit.memory_content_contains)?;
        hits.push(VectorMemoryHit {
            memory_id,
            score: hit.score,
        });
    }
    Some(SeededMemoryVectorIndex { hits })
}

impl MemoryEvalCheckResult {
    fn pass(name: &str, detail: String) -> Self {
        Self {
            name: name.to_string(),
            passed: true,
            detail,
        }
    }

    fn fail(name: &str, detail: String) -> Self {
        Self {
            name: name.to_string(),
            passed: false,
            detail,
        }
    }
}

fn find_memory_id_by_content(manager: &MemoryManager, expected: &str) -> Option<String> {
    all_memories(manager)
        .into_iter()
        .find(|memory| memory.content.contains(expected))
        .map(|memory| memory.id)
}

fn all_memories(manager: &MemoryManager) -> Vec<crate::Memory> {
    manager.get_recent(RecentMemoryOptions {
        limit: Some(usize::MAX),
        ..RecentMemoryOptions::default()
    })
}

fn relationship_recall_case() -> MemoryEvalCase {
    MemoryEvalCase {
        name: "relationship recall and trace".into(),
        seed_memories: vec![
            baseline_memory(|memory| {
                memory.agent_id = "planner".into();
                memory.agent_name = "Planner".into();
                memory.content = "Leo prefers terse release summaries with rollback notes".into();
                memory.importance = 0.86;
                memory.tags = Some(vec!["preference".into(), "release".into()]);
                memory.room_id = Some("room-1".into());
                memory.world_id = Some("world-1".into());
                memory.session_id = Some("session-1".into());
            }),
            baseline_memory(|memory| {
                memory.agent_id = "critic".into();
                memory.agent_name = "Critic".into();
                memory.content = "Critic tracks unrelated deployment risk notes".into();
                memory.importance = 0.55;
            }),
        ],
        seed_relationships: vec![MemoryEvalRelationshipSeed {
            relationship: baseline_relationship(|relationship| {
                relationship.source_agent_id = "planner".into();
                relationship.source_agent_name = "Planner".into();
                relationship.target_kind = Some(RelationshipEndpointKind::User);
                relationship.target_agent_id = "user-1".into();
                relationship.target_agent_name = "Leo".into();
                relationship.relationship_type = "responds_to".into();
                relationship.evidence_memory_ids = Vec::new();
                relationship.world_id = Some("world-1".into());
            }),
            evidence_content_contains: vec!["terse release summaries".into()],
        }],
        checks: vec![
            MemoryEvalCheck::RecallTopN {
                name: "relationship-only recall finds user preference".into(),
                query: "how should planner brief user-1".into(),
                options: MemoryRecallOptions {
                    entity_id: Some("user-1".into()),
                    recent_limit: Some(0),
                    limit: Some(3),
                    relationship_limit: Some(5),
                    ..MemoryRecallOptions::default()
                },
                expected_content_contains: "terse release summaries".into(),
                top_n: 1,
                require_relationship_score: true,
            },
            MemoryEvalCheck::Trace {
                name: "trace explains user relationship evidence".into(),
                memory_content_contains: "terse release summaries".into(),
                min_relationships: 1,
                required_entity_id: Some("user-1".into()),
            },
        ],
    }
}

fn agent_handoff_case() -> MemoryEvalCase {
    MemoryEvalCase {
        name: "agent handoff memory".into(),
        seed_memories: vec![
            baseline_memory(|memory| {
                memory.agent_id = "planner".into();
                memory.agent_name = "Planner".into();
                memory.content =
                    "Planner delegated launch risk review to Critic with rollback checklist".into();
                memory.importance = 0.84;
                memory.tags = Some(vec!["handoff".into(), "agent-agent".into()]);
                memory.world_id = Some("world-1".into());
            }),
            baseline_memory(|memory| {
                memory.agent_id = "finance".into();
                memory.agent_name = "Finance".into();
                memory.content = "Finance tracks unrelated invoice export details".into();
                memory.importance = 0.7;
                memory.world_id = Some("world-1".into());
            }),
        ],
        seed_relationships: vec![MemoryEvalRelationshipSeed {
            relationship: baseline_relationship(|relationship| {
                relationship.source_agent_id = "planner".into();
                relationship.source_agent_name = "Planner".into();
                relationship.target_agent_id = "critic".into();
                relationship.target_agent_name = "Critic".into();
                relationship.relationship_type = "delegated_to".into();
                relationship.summary = Some("Planner handed launch risk review to Critic".into());
                relationship.evidence_memory_ids = Vec::new();
                relationship.world_id = Some("world-1".into());
            }),
            evidence_content_contains: vec!["launch risk review".into()],
        }],
        checks: vec![
            MemoryEvalCheck::RecallTopN {
                name: "agent-agent handoff recall finds delegated context".into(),
                query: "what context should critic carry forward".into(),
                options: MemoryRecallOptions {
                    entity_id: Some("critic".into()),
                    recent_limit: Some(0),
                    limit: Some(2),
                    relationship_limit: Some(5),
                    ..MemoryRecallOptions::default()
                },
                expected_content_contains: "launch risk review".into(),
                top_n: 1,
                require_relationship_score: true,
            },
            MemoryEvalCheck::Trace {
                name: "agent-agent handoff trace includes target agent".into(),
                memory_content_contains: "launch risk review".into(),
                min_relationships: 1,
                required_entity_id: Some("critic".into()),
            },
        ],
    }
}

fn room_world_isolation_case() -> MemoryEvalCase {
    MemoryEvalCase {
        name: "room and world isolation".into(),
        seed_memories: vec![
            baseline_memory(|memory| {
                memory.agent_id = "planner".into();
                memory.agent_name = "Planner".into();
                memory.content = "Leo wants launch briefs in room alpha".into();
                memory.importance = 0.8;
                memory.scope = Some(MemoryScope::Room);
                memory.room_id = Some("room-alpha".into());
                memory.world_id = Some("world-alpha".into());
            }),
            baseline_memory(|memory| {
                memory.agent_id = "planner".into();
                memory.agent_name = "Planner".into();
                memory.content = "Leo wants billing summaries in room beta".into();
                memory.importance = 0.95;
                memory.scope = Some(MemoryScope::Room);
                memory.room_id = Some("room-beta".into());
                memory.world_id = Some("world-beta".into());
            }),
        ],
        seed_relationships: vec![
            MemoryEvalRelationshipSeed {
                relationship: baseline_relationship(|relationship| {
                    relationship.source_agent_id = "planner".into();
                    relationship.source_agent_name = "Planner".into();
                    relationship.target_kind = Some(RelationshipEndpointKind::User);
                    relationship.target_agent_id = "user-1".into();
                    relationship.target_agent_name = "Leo".into();
                    relationship.relationship_type = "responds_to".into();
                    relationship.evidence_memory_ids = Vec::new();
                    relationship.room_id = Some("room-alpha".into());
                    relationship.world_id = Some("world-alpha".into());
                }),
                evidence_content_contains: vec!["launch briefs".into()],
            },
            MemoryEvalRelationshipSeed {
                relationship: baseline_relationship(|relationship| {
                    relationship.source_agent_id = "planner".into();
                    relationship.source_agent_name = "Planner".into();
                    relationship.target_kind = Some(RelationshipEndpointKind::User);
                    relationship.target_agent_id = "user-1".into();
                    relationship.target_agent_name = "Leo".into();
                    relationship.relationship_type = "responds_to".into();
                    relationship.evidence_memory_ids = Vec::new();
                    relationship.room_id = Some("room-beta".into());
                    relationship.world_id = Some("world-beta".into());
                }),
                evidence_content_contains: vec!["billing summaries".into()],
            },
        ],
        checks: vec![
            MemoryEvalCheck::RecallTopN {
                name: "world-scoped recall returns matching room memory".into(),
                query: "what does Leo want".into(),
                options: MemoryRecallOptions {
                    search: MemorySearchOptions {
                        scope: Some(MemoryScope::Room),
                        room_id: Some("room-alpha".into()),
                        world_id: Some("world-alpha".into()),
                        ..MemorySearchOptions::default()
                    },
                    entity_id: Some("user-1".into()),
                    recent_limit: Some(0),
                    limit: Some(2),
                    relationship_limit: Some(5),
                    ..MemoryRecallOptions::default()
                },
                expected_content_contains: "launch briefs".into(),
                top_n: 1,
                require_relationship_score: true,
            },
            MemoryEvalCheck::RecallExcludesTopN {
                name: "world-scoped recall excludes other rooms".into(),
                query: "what does Leo want".into(),
                options: MemoryRecallOptions {
                    search: MemorySearchOptions {
                        scope: Some(MemoryScope::Room),
                        room_id: Some("room-alpha".into()),
                        world_id: Some("world-alpha".into()),
                        ..MemorySearchOptions::default()
                    },
                    entity_id: Some("user-1".into()),
                    recent_limit: Some(0),
                    limit: Some(3),
                    relationship_limit: Some(5),
                    ..MemoryRecallOptions::default()
                },
                excluded_content_contains: "billing summaries".into(),
                top_n: 3,
            },
        ],
    }
}

fn evaluated_write_case() -> MemoryEvalCase {
    MemoryEvalCase {
        name: "evaluated writes".into(),
        seed_memories: vec![baseline_memory(|memory| {
            memory.content = "User stated preference: I prefer terse release summaries".into();
            memory.importance = 0.72;
            memory.tags = Some(vec!["user-stated".into()]);
        })],
        seed_relationships: Vec::new(),
        checks: vec![
            MemoryEvalCheck::EvaluateDecision {
                name: "explicit user preference stores".into(),
                candidate: baseline_memory(|memory| {
                    memory.content =
                        "User stated preference: I prefer changelogs with risk callouts".into();
                    memory.importance = 0.7;
                    memory.tags = Some(vec!["user-stated".into()]);
                }),
                options: MemoryEvaluationOptions::default(),
                expected_decision: MemoryEvaluationDecision::Store,
            },
            MemoryEvalCheck::EvaluateDecision {
                name: "duplicate preference merges".into(),
                candidate: baseline_memory(|memory| {
                    memory.content =
                        "User stated preference: I prefer terse release summaries".into();
                    memory.importance = 0.6;
                    memory.tags = Some(vec!["user-stated".into()]);
                }),
                options: MemoryEvaluationOptions::default(),
                expected_decision: MemoryEvaluationDecision::Merge,
            },
            MemoryEvalCheck::EvaluateDecision {
                name: "conflicting preference stores as distinct evidence".into(),
                candidate: baseline_memory(|memory| {
                    memory.content =
                        "User stated preference: I prefer detailed release summaries".into();
                    memory.importance = 0.68;
                    memory.tags = Some(vec!["user-stated".into(), "conflict".into()]);
                }),
                options: MemoryEvaluationOptions::default(),
                expected_decision: MemoryEvaluationDecision::Store,
            },
            MemoryEvalCheck::EvaluateDecision {
                name: "low-value junk ignored".into(),
                candidate: baseline_memory(|memory| {
                    memory.content = "ok".into();
                    memory.importance = 0.01;
                }),
                options: MemoryEvaluationOptions::default(),
                expected_decision: MemoryEvaluationDecision::Ignore,
            },
        ],
    }
}

fn vector_recall_case() -> MemoryEvalCase {
    MemoryEvalCase {
        name: "vector recall".into(),
        seed_memories: vec![
            baseline_memory(|memory| {
                memory.agent_id = "planner".into();
                memory.agent_name = "Planner".into();
                memory.content = "Operator wants concise ship notes".into();
                memory.importance = 0.78;
                memory.tags = Some(vec!["preference".into(), "semantic".into()]);
            }),
            baseline_memory(|memory| {
                memory.agent_id = "finance".into();
                memory.agent_name = "Finance".into();
                memory.content = "Billing ledger exports include invoice IDs".into();
                memory.importance = 0.62;
            }),
        ],
        seed_relationships: Vec::new(),
        checks: vec![
            MemoryEvalCheck::VectorRecallTopN {
                name: "semantic vector recall finds no-overlap preference".into(),
                query: "release briefing style".into(),
                options: MemoryRecallOptions {
                    recent_limit: Some(0),
                    lexical_limit: Some(2),
                    limit: Some(1),
                    ..MemoryRecallOptions::default()
                },
                vector_hits: vec![
                    MemoryEvalVectorHitSeed {
                        memory_content_contains: "concise ship notes".into(),
                        score: 0.94,
                    },
                    MemoryEvalVectorHitSeed {
                        memory_content_contains: "Billing ledger".into(),
                        score: 0.12,
                    },
                ],
                expected_content_contains: "concise ship notes".into(),
                top_n: 1,
            },
            MemoryEvalCheck::VectorRecallExcludesTopN {
                name: "semantic vector recall suppresses unrelated finance memory".into(),
                query: "release briefing style".into(),
                options: MemoryRecallOptions {
                    recent_limit: Some(0),
                    lexical_limit: Some(2),
                    limit: Some(1),
                    ..MemoryRecallOptions::default()
                },
                vector_hits: vec![
                    MemoryEvalVectorHitSeed {
                        memory_content_contains: "concise ship notes".into(),
                        score: 0.94,
                    },
                    MemoryEvalVectorHitSeed {
                        memory_content_contains: "Billing ledger".into(),
                        score: 0.12,
                    },
                ],
                excluded_content_contains: "Billing ledger".into(),
                top_n: 1,
            },
        ],
    }
}

fn retention_case() -> MemoryEvalCase {
    MemoryEvalCase {
        name: "retention policy".into(),
        seed_memories: vec![
            baseline_memory(|memory| {
                memory.content = "Critical launch rollback memory must survive retention".into();
                memory.importance = 0.92;
                memory.tags = Some(vec!["critical".into()]);
            }),
            baseline_memory(|memory| {
                memory.content = "Low signal hallway note should be pruned".into();
                memory.importance = 0.05;
            }),
        ],
        seed_relationships: Vec::new(),
        checks: vec![MemoryEvalCheck::Retention {
            name: "keeps important facts and removes weak notes".into(),
            policy: MemoryRetentionPolicy {
                min_importance: Some(0.2),
                ..MemoryRetentionPolicy::default()
            },
            removed_content_contains: vec!["hallway note".into()],
            retained_content_contains: vec!["Critical launch rollback".into()],
        }],
    }
}

fn decay_case() -> MemoryEvalCase {
    MemoryEvalCase {
        name: "stale memory decay".into(),
        seed_memories: vec![
            baseline_memory(|memory| {
                memory.content = "Old launch note should decay but remain available".into();
                memory.importance = 0.8;
                memory.tags = Some(vec!["decay".into()]);
            }),
            baseline_memory(|memory| {
                memory.content =
                    "High value identity preference should remain after decay pass".into();
                memory.importance = 0.95;
                memory.tags = Some(vec!["identity".into()]);
            }),
        ],
        seed_relationships: Vec::new(),
        checks: vec![MemoryEvalCheck::RetentionDecay {
            name: "decays stale memories without deleting retained facts".into(),
            policy: MemoryRetentionPolicy {
                decay_half_life_millis: Some(1_000),
                min_importance: Some(0.1),
                ..MemoryRetentionPolicy::default()
            },
            elapsed_millis: 1_000,
            decayed_content_contains: "Old launch note".into(),
            retained_content_contains: "identity preference".into(),
        }],
    }
}

fn baseline_memory(overrides: impl FnOnce(&mut NewMemory)) -> NewMemory {
    let mut memory = NewMemory {
        agent_id: "agent-1".into(),
        agent_name: "Researcher".into(),
        memory_type: MemoryType::Fact,
        content: "baseline memory".into(),
        importance: 0.5,
        tags: None,
        scope: Some(MemoryScope::Private),
        room_id: None,
        world_id: None,
        session_id: None,
    };
    overrides(&mut memory);
    memory
}

fn baseline_relationship(
    overrides: impl FnOnce(&mut NewAgentRelationship),
) -> NewAgentRelationship {
    let mut relationship = NewAgentRelationship {
        source_kind: Some(RelationshipEndpointKind::Agent),
        source_agent_id: "agent-1".into(),
        source_agent_name: "Researcher".into(),
        target_kind: Some(RelationshipEndpointKind::Agent),
        target_agent_id: "agent-2".into(),
        target_agent_name: "Reviewer".into(),
        relationship_type: "collaborates_with".into(),
        summary: Some("baseline relationship".into()),
        strength: 0.8,
        confidence: 0.75,
        evidence_memory_ids: Vec::new(),
        tags: Some(vec!["memory-eval".into()]),
        room_id: None,
        world_id: None,
        session_id: None,
    };
    overrides(&mut relationship);
    relationship
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_memory_eval_cases_pass() {
        let report = run_memory_eval_cases(&baseline_memory_eval_cases());

        assert!(report.passed(), "{:?}", report.failure_messages());
        assert_eq!(report.total_checks(), 14);
        assert_eq!(report.passed_checks(), 14);
    }

    #[test]
    fn memory_eval_report_exposes_failure_messages() {
        let case = MemoryEvalCase {
            name: "failing recall".into(),
            seed_memories: vec![baseline_memory(|memory| {
                memory.content = "durable launch memory".into();
            })],
            seed_relationships: Vec::new(),
            checks: vec![MemoryEvalCheck::RecallTopN {
                name: "missing expected content".into(),
                query: "launch".into(),
                options: MemoryRecallOptions::default(),
                expected_content_contains: "nonexistent memory".into(),
                top_n: 1,
                require_relationship_score: false,
            }],
        };

        let report = run_memory_eval_cases(&[case]);

        assert!(!report.passed());
        assert_eq!(report.total_checks(), 1);
        assert_eq!(report.failure_messages().len(), 1);
        assert!(report.failure_messages()[0].contains("nonexistent memory"));
    }
}
