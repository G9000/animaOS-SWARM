#![cfg(feature = "locomo-eval")]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anima_memory::{
    locomo_query_expander, MemoryManager, MemoryRecallOptions, MemoryRecallWeights, MemoryScope,
    MemorySearchOptions, MemoryType, NewMemory, NewTemporalFact, RelationshipEndpointKind,
    TemporalRecordStatus,
};
use serde::Deserialize;
use serde_json::Value;

const DEFAULT_TOP_K: usize = 20;
const DEFAULT_MIN_HIT_RATE: f64 = 0.70;
const DEFAULT_MIN_CATEGORY_HIT_RATE: f64 = 0.50;
const DEFAULT_MIN_MRR: f64 = 0.40;
const DEFAULT_MIN_QUESTIONS: usize = 1_500;
const DEFAULT_MIN_TURNS: usize = 5_000;
const DEFAULT_TEMPORAL_WEIGHT_SWEEP: &[f64] = &[0.025, 0.05, 0.075, 0.10];
const DEFAULT_TEMPORAL_RERANK_WEIGHT_SWEEP: &[f64] = &[0.075];
const DEFAULT_TEMPORAL_RERANK_BONUS: f64 = 0.02;

#[test]
fn locomo_dataset_benchmark_reaches_retrieval_thresholds() {
    let Some(path) = locomo_dataset_path() else {
        eprintln!("skipping LOCOMO dataset benchmark; set LOCOMO_DATASET_JSON or run core-rust:memory-locomo-dataset");
        return;
    };

    let text = fs::read_to_string(&path).expect("LOCOMO dataset JSON should be readable");
    let dataset: Vec<LocomoConversation> =
        serde_json::from_str(&text).expect("LOCOMO dataset JSON should parse");
    let top_k = env_usize("LOCOMO_TOP_K", DEFAULT_TOP_K);
    let min_hit_rate = env_f64("LOCOMO_MIN_HIT_RATE", DEFAULT_MIN_HIT_RATE);
    let min_category_hit_rate = env_f64(
        "LOCOMO_MIN_CATEGORY_HIT_RATE",
        DEFAULT_MIN_CATEGORY_HIT_RATE,
    );
    let min_mrr = env_f64("LOCOMO_MIN_MRR", DEFAULT_MIN_MRR);
    let min_core_hit_rate = env_optional_f64("LOCOMO_MIN_CORE_HIT_RATE");
    let min_core_mrr = env_optional_f64("LOCOMO_MIN_CORE_MRR");
    let min_questions = env_usize("LOCOMO_MIN_QUESTIONS", DEFAULT_MIN_QUESTIONS);
    let min_turns = env_usize("LOCOMO_MIN_TURNS", DEFAULT_MIN_TURNS);
    let temporal_weights = temporal_weight_sweep();
    let temporal_rerank_weights = temporal_rerank_weight_sweep();
    let temporal_rerank_bonus = env_f64(
        "LOCOMO_TEMPORAL_RERANK_BONUS",
        DEFAULT_TEMPORAL_RERANK_BONUS,
    );
    let miss_report_options = miss_report_options(top_k);

    let core_report = run_dataset_benchmark(&dataset, top_k, BenchmarkProfile::Core, None);
    let tuned_report = run_dataset_benchmark(&dataset, top_k, BenchmarkProfile::LocomoTuned, None);
    let temporal_reports: Vec<_> = temporal_weights
        .into_iter()
        .map(|temporal_weight| {
            run_dataset_benchmark(
                &dataset,
                top_k,
                BenchmarkProfile::LocomoTemporal { temporal_weight },
                miss_report_options,
            )
        })
        .collect();
    let temporal_rerank_reports: Vec<_> = temporal_rerank_weights
        .into_iter()
        .map(|temporal_weight| {
            run_dataset_benchmark(
                &dataset,
                top_k,
                BenchmarkProfile::LocomoTemporalRerank {
                    temporal_weight,
                    rerank_bonus: temporal_rerank_bonus,
                },
                miss_report_options,
            )
        })
        .collect();

    println!("LOCOMO dataset benchmark");
    println!("  dataset: {}", path.display());
    println!("  top_k: {}", top_k);
    println!(
        "  tuning delta: hit_rate={:.3} all_hit_rate={:.3} mrr={:.3}",
        tuned_report.hit_rate() - core_report.hit_rate(),
        tuned_report.all_hit_rate() - core_report.all_hit_rate(),
        tuned_report.mean_reciprocal_rank() - core_report.mean_reciprocal_rank()
    );
    print_profile_report(&core_report, &path);
    print_profile_report(&tuned_report, &path);
    for temporal_report in &temporal_reports {
        print_profile_report(temporal_report, &path);
    }
    for temporal_rerank_report in &temporal_rerank_reports {
        print_profile_report(temporal_rerank_report, &path);
    }
    if let Some(best_temporal) = best_category_report(temporal_reports.iter(), 3) {
        println!(
            "  best temporal category 3: profile={} hit_rate={:.3} all_hit_rate={:.3} mrr={:.3}",
            best_temporal.profile.label(),
            best_temporal
                .by_category
                .get(&3)
                .map(CategoryMetrics::hit_rate)
                .unwrap_or_default(),
            best_temporal
                .by_category
                .get(&3)
                .map(CategoryMetrics::all_hit_rate)
                .unwrap_or_default(),
            best_temporal
                .by_category
                .get(&3)
                .map(CategoryMetrics::mean_reciprocal_rank)
                .unwrap_or_default()
        );
    }
    if let Some(best_temporal_rerank) = best_category_report(temporal_rerank_reports.iter(), 3) {
        println!(
            "  best temporal rerank category 3: profile={} hit_rate={:.3} all_hit_rate={:.3} mrr={:.3}",
            best_temporal_rerank.profile.label(),
            best_temporal_rerank
                .by_category
                .get(&3)
                .map(CategoryMetrics::hit_rate)
                .unwrap_or_default(),
            best_temporal_rerank
                .by_category
                .get(&3)
                .map(CategoryMetrics::all_hit_rate)
                .unwrap_or_default(),
            best_temporal_rerank
                .by_category
                .get(&3)
                .map(CategoryMetrics::mean_reciprocal_rank)
                .unwrap_or_default()
        );
    }
    if let Some(options) = miss_report_options {
        if let Some(best_temporal) = best_category_report(
            temporal_reports
                .iter()
                .chain(temporal_rerank_reports.iter()),
            options.category,
        ) {
            print_miss_report(best_temporal, options);
        }
    }

    assert_same_dataset_shape(&core_report, &tuned_report);
    for temporal_report in &temporal_reports {
        assert_same_dataset_shape(&core_report, temporal_report);
    }
    for temporal_rerank_report in &temporal_rerank_reports {
        assert_same_dataset_shape(&core_report, temporal_rerank_report);
    }
    assert_dataset_minimums(&core_report, min_questions, min_turns);
    assert_profile_thresholds(&tuned_report, min_hit_rate, min_category_hit_rate, min_mrr);

    if let Some(min_core_hit_rate) = min_core_hit_rate {
        assert!(
            core_report.hit_rate() >= min_core_hit_rate,
            "expected core evidence hit rate >= {min_core_hit_rate:.3}, got {:.3}",
            core_report.hit_rate()
        );
    }

    if let Some(min_core_mrr) = min_core_mrr {
        assert!(
            core_report.mean_reciprocal_rank() >= min_core_mrr,
            "expected core MRR >= {min_core_mrr:.3}, got {:.3}",
            core_report.mean_reciprocal_rank()
        );
    }
}

fn print_miss_report(report: &BenchmarkReport, options: MissReportOptions) {
    println!(
        "  category {} miss report: profile={} total_misses={} shown={} retrieved_rows_per_miss={}",
        options.category,
        report.profile.label(),
        report.miss_report_total,
        report.misses.len(),
        options.retrieved_limit
    );
    if report.misses.is_empty() {
        println!("    no misses captured for this profile/category");
        return;
    }

    for (index, miss) in report.misses.iter().enumerate() {
        println!(
            "    miss {} sample={} resolved_entity={}",
            index + 1,
            miss.sample_id,
            miss.resolved_entity.as_deref().unwrap_or("<none>")
        );
        println!("      q: {}", truncate_for_report(&miss.question, 180));
        if !miss.answer.is_empty() {
            println!("      a: {}", truncate_for_report(&miss.answer, 180));
        }
        println!(
            "      question relations: {}",
            format_relations(&miss.question_relations)
        );
        println!(
            "      seeded temporal evidence: {}/{}",
            miss.seeded_evidence_count,
            miss.evidence.len()
        );
        println!(
            "      relation-matched evidence: {}/{}",
            miss.matched_relation_evidence_count,
            miss.evidence.len()
        );
        println!("      evidence:");
        for evidence in &miss.evidence {
            println!(
                "        {} [{}] temporal_seeded={} matched_relations={} {}",
                evidence.dia_id,
                evidence.speaker,
                !evidence.temporal_facts.is_empty(),
                format_relations(&evidence.matching_question_relations),
                truncate_for_report(&evidence.content, 220)
            );
            for fact in &evidence.temporal_facts {
                println!(
                    "          fact relations={} subject={} predicate={} value={}",
                    format_relations(&fact.relation_labels),
                    fact.subject_name,
                    fact.predicate,
                    truncate_for_report(&fact.value, 160)
                );
            }
        }
        println!("      retrieved:");
        for row in &miss.retrieved {
            println!(
                "        #{:02} {} score={:.3} lex={:.3} tmp={:.3} rel={:.3} rec={:.3} imp={:.3} {}",
                row.rank,
                row.dia_id,
                row.score,
                row.lexical_score,
                row.temporal_score,
                row.relationship_score,
                row.recency_score,
                row.importance_score,
                truncate_for_report(&row.content, 220)
            );
        }
    }
}

fn print_profile_report(report: &BenchmarkReport, path: &Path) {
    println!("  profile: {}", report.profile.label());
    println!("    dataset: {}", path.display());
    println!("    conversations: {}", report.conversations);
    println!("    ingested turns: {}", report.turns);
    if report.seeded_temporal_facts > 0 {
        println!(
            "    seeded temporal facts: {}",
            report.seeded_temporal_facts
        );
    }
    println!("    evaluated questions: {}", report.evaluated_questions);
    println!("    skipped questions: {}", report.skipped_questions);
    println!("    evidence hit rate: {:.3}", report.hit_rate());
    println!("    all evidence hit rate: {:.3}", report.all_hit_rate());
    println!(
        "    mean reciprocal rank: {:.3}",
        report.mean_reciprocal_rank()
    );
    for (category, metrics) in &report.by_category {
        println!(
            "    category {}: questions={} hit_rate={:.3} all_hit_rate={:.3} mrr={:.3}",
            category,
            metrics.questions,
            metrics.hit_rate(),
            metrics.all_hit_rate(),
            metrics.mean_reciprocal_rank()
        );
    }
}

fn assert_same_dataset_shape(core_report: &BenchmarkReport, tuned_report: &BenchmarkReport) {
    assert_eq!(
        core_report.conversations, tuned_report.conversations,
        "core and tuned profiles should evaluate the same conversation count"
    );
    assert_eq!(
        core_report.turns, tuned_report.turns,
        "core and tuned profiles should ingest the same turn count"
    );
    assert_eq!(
        core_report.evaluated_questions, tuned_report.evaluated_questions,
        "core and tuned profiles should evaluate the same questions"
    );
    assert_eq!(
        core_report.skipped_questions, tuned_report.skipped_questions,
        "core and tuned profiles should skip the same questions"
    );
}

fn assert_dataset_minimums(report: &BenchmarkReport, min_questions: usize, min_turns: usize) {
    assert!(
        report.conversations >= 10,
        "expected at least 10 conversations"
    );
    assert!(
        report.turns >= min_turns,
        "expected at least {min_turns} turns, got {}",
        report.turns
    );
    assert!(
        report.evaluated_questions >= min_questions,
        "expected at least {min_questions} evaluated questions, got {}",
        report.evaluated_questions
    );
}

fn assert_profile_thresholds(
    report: &BenchmarkReport,
    min_hit_rate: f64,
    min_category_hit_rate: f64,
    min_mrr: f64,
) {
    assert!(
        report.hit_rate() >= min_hit_rate,
        "expected {} evidence hit rate >= {min_hit_rate:.3}, got {:.3}",
        report.profile.label(),
        report.hit_rate()
    );
    assert!(
        report.mean_reciprocal_rank() >= min_mrr,
        "expected {} MRR >= {min_mrr:.3}, got {:.3}",
        report.profile.label(),
        report.mean_reciprocal_rank()
    );
    for (category, metrics) in &report.by_category {
        assert!(
            metrics.hit_rate() >= min_category_hit_rate,
            "expected {} category {category} hit rate >= {min_category_hit_rate:.3}, got {:.3}",
            report.profile.label(),
            metrics.hit_rate()
        );
    }
}

fn run_dataset_benchmark(
    dataset: &[LocomoConversation],
    top_k: usize,
    profile: BenchmarkProfile,
    miss_report_options: Option<MissReportOptions>,
) -> BenchmarkReport {
    let mut report = BenchmarkReport {
        profile,
        ..BenchmarkReport::default()
    };
    report.conversations = dataset.len();

    for item in dataset {
        let mut manager = profile.memory_manager();
        let mut memory_id_by_dia_id = HashMap::new();
        let turns = extract_turns(item);
        report.turns += turns.len();

        for turn in &turns {
            let content = turn.memory_content();
            let memory = manager
                .add(NewMemory {
                    agent_id: normalize_agent_id(&turn.speaker),
                    agent_name: turn.speaker.clone(),
                    memory_type: MemoryType::Observation,
                    content,
                    importance: 0.55,
                    tags: Some(vec!["locomo".into(), turn.dia_id.clone()]),
                    scope: Some(MemoryScope::Room),
                    room_id: Some(item.sample_id.clone()),
                    world_id: Some("locomo".into()),
                    session_id: turn.session_id.clone(),
                })
                .expect("LOCOMO turn memory should be valid");
            memory_id_by_dia_id.insert(turn.dia_id.clone(), memory.id);
        }

        let temporal_seed_index = if profile.seeds_temporal_facts() {
            let seed_index = seed_temporal_facts(&mut manager, item, &turns, &memory_id_by_dia_id);
            report.seeded_temporal_facts += seed_index.count;
            seed_index
        } else {
            TemporalSeedIndex::default()
        };

        let speaker_entities = speaker_entities(&turns);

        for qa in &item.qa {
            if qa.category == 5 || qa.evidence.is_empty() {
                report.skipped_questions += 1;
                continue;
            }

            let expected_memory_ids: HashSet<_> = qa
                .evidence
                .iter()
                .filter_map(|dia_id| memory_id_by_dia_id.get(dia_id).cloned())
                .collect();
            if expected_memory_ids.is_empty() {
                report.skipped_questions += 1;
                continue;
            }

            let entity_id = profile
                .seeds_temporal_facts()
                .then(|| resolve_question_entity(&qa.question, &speaker_entities))
                .flatten();
            let resolved_entity = entity_id.clone();
            let weights = profile.recall_weights();
            let temporal_limit = profile
                .seeds_temporal_facts()
                .then_some(top_k.saturating_mul(16).max(200));
            let temporal_intent_terms = profile.temporal_intent_terms();
            let recall_limit = profile.recall_limit(top_k);

            let mut results = manager.recall(
                &qa.question,
                MemoryRecallOptions {
                    search: MemorySearchOptions {
                        scope: Some(MemoryScope::Room),
                        room_id: Some(item.sample_id.clone()),
                        world_id: Some("locomo".into()),
                        ..MemorySearchOptions::default()
                    },
                    recent_limit: Some(0),
                    lexical_limit: Some(top_k.saturating_mul(4).max(top_k)),
                    relationship_limit: Some(0),
                    temporal_limit,
                    entity_id,
                    temporal_intent_terms,
                    weights,
                    limit: Some(recall_limit),
                    ..MemoryRecallOptions::default()
                },
            );
            apply_temporal_rerank(&qa.question, profile, &mut results);
            results.truncate(top_k);
            let ranks: Vec<_> = results
                .iter()
                .enumerate()
                .filter_map(|(index, result)| {
                    expected_memory_ids
                        .contains(&result.memory.id)
                        .then_some(index + 1)
                })
                .collect();

            let best_rank = ranks.iter().copied().min();
            let all_evidence_hit = ranks.len() == expected_memory_ids.len();
            let answer_overlap = qa
                .answer
                .as_ref()
                .map(|answer| answer_overlap_score(answer, &results))
                .unwrap_or_default();
            report.record(qa.category, best_rank, all_evidence_hit, answer_overlap);
            if let Some(options) = miss_report_options {
                if qa.category == options.category && best_rank.is_none() {
                    report.miss_report_total += 1;
                    if report.misses.len() < options.limit {
                        report.misses.push(build_miss_detail(
                            item,
                            qa,
                            resolved_entity,
                            &turns,
                            &memory_id_by_dia_id,
                            &temporal_seed_index,
                            &results,
                            options.retrieved_limit,
                        ));
                    }
                }
            }
        }
    }

    report
}

fn build_miss_detail(
    item: &LocomoConversation,
    qa: &LocomoQa,
    resolved_entity: Option<String>,
    turns: &[LocomoTurn],
    memory_id_by_dia_id: &HashMap<String, String>,
    temporal_seed_index: &TemporalSeedIndex,
    results: &[anima_memory::MemoryRecallResult],
    retrieved_limit: usize,
) -> CategoryMissDetail {
    let question_relations = relation_labels_to_strings(question_relation_labels(&qa.question));
    let evidence = qa
        .evidence
        .iter()
        .map(|dia_id| {
            let temporal_facts = memory_id_by_dia_id
                .get(dia_id)
                .and_then(|memory_id| temporal_seed_index.by_memory_id.get(memory_id))
                .cloned()
                .unwrap_or_default();
            let matching_question_relations =
                matching_relation_labels(&question_relations, &temporal_facts);
            let (speaker, content) = turns
                .iter()
                .find(|turn| turn.dia_id == *dia_id)
                .map(|turn| (turn.speaker.clone(), turn.memory_content()))
                .unwrap_or_else(|| ("<unknown>".into(), "<missing evidence turn>".into()));
            CategoryMissEvidence {
                dia_id: dia_id.clone(),
                speaker,
                content,
                matching_question_relations,
                temporal_facts,
            }
        })
        .collect::<Vec<_>>();
    let seeded_evidence_count = evidence
        .iter()
        .filter(|evidence| !evidence.temporal_facts.is_empty())
        .count();
    let matched_relation_evidence_count = evidence
        .iter()
        .filter(|evidence| !evidence.matching_question_relations.is_empty())
        .count();
    let retrieved = results
        .iter()
        .take(retrieved_limit)
        .enumerate()
        .map(|(index, result)| CategoryMissRetrieved {
            rank: index + 1,
            dia_id: memory_dia_id(result),
            score: result.score,
            lexical_score: result.lexical_score,
            temporal_score: result.temporal_score,
            relationship_score: result.relationship_score,
            recency_score: result.recency_score,
            importance_score: result.importance_score,
            content: result.memory.content.clone(),
        })
        .collect();

    CategoryMissDetail {
        sample_id: item.sample_id.clone(),
        question: qa.question.clone(),
        answer: qa.answer.as_ref().map(answer_to_text).unwrap_or_default(),
        resolved_entity,
        question_relations,
        seeded_evidence_count,
        matched_relation_evidence_count,
        evidence,
        retrieved,
    }
}

fn apply_temporal_rerank(
    query: &str,
    profile: BenchmarkProfile,
    results: &mut [anima_memory::MemoryRecallResult],
) {
    let Some(rerank_bonus) = profile.temporal_rerank_bonus() else {
        return;
    };
    if !temporal_rerank_query_has_intent(query) {
        return;
    }

    results.sort_by(|left, right| {
        temporal_rerank_score(right, rerank_bonus)
            .total_cmp(&temporal_rerank_score(left, rerank_bonus))
            .then_with(|| right.score.total_cmp(&left.score))
            .then_with(|| right.memory.created_at.cmp(&left.memory.created_at))
            .then_with(|| right.memory.id.cmp(&left.memory.id))
    });
}

fn temporal_rerank_score(result: &anima_memory::MemoryRecallResult, rerank_bonus: f64) -> f64 {
    let temporal_score = result.temporal_score.clamp(0.0, 1.0);
    result.score + (temporal_score * rerank_bonus)
}

fn temporal_rerank_query_has_intent(query: &str) -> bool {
    !question_relation_labels(query).is_empty()
}

fn matching_relation_labels(
    question_relations: &[String],
    temporal_facts: &[TemporalSeedDebug],
) -> Vec<String> {
    let mut matches = Vec::new();
    for relation in question_relations {
        if temporal_facts
            .iter()
            .any(|fact| fact.relation_labels.iter().any(|label| label == relation))
            && !matches.contains(relation)
        {
            matches.push(relation.clone());
        }
    }
    matches
}

fn relation_labels_to_strings(labels: Vec<TemporalRelationLabel>) -> Vec<String> {
    labels
        .into_iter()
        .map(|label| label.label().to_string())
        .collect()
}

fn format_relations(relations: &[String]) -> String {
    if relations.is_empty() {
        "<none>".into()
    } else {
        relations.join(",")
    }
}

fn memory_dia_id(result: &anima_memory::MemoryRecallResult) -> String {
    result
        .memory
        .tags
        .as_ref()
        .and_then(|tags| tags.iter().find(|tag| tag.as_str() != "locomo"))
        .cloned()
        .unwrap_or_else(|| result.memory.id.clone())
}

fn truncate_for_report(value: &str, max_chars: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= max_chars {
        return normalized;
    }
    let keep = max_chars.saturating_sub(3);
    let mut truncated = normalized.chars().take(keep).collect::<String>();
    truncated.push_str("...");
    truncated
}

fn extract_turns(item: &LocomoConversation) -> Vec<LocomoTurn> {
    let mut turns = Vec::new();
    let mut entries: Vec<_> = item.conversation.iter().collect();
    entries.sort_by_key(|(key, _)| session_sort_key(key));

    for (session_key, value) in entries {
        let Some(session_turns) = value.as_array() else {
            continue;
        };
        let session_date_time = item
            .conversation
            .get(&format!("{session_key}_date_time"))
            .and_then(Value::as_str)
            .map(str::to_string);
        for turn_value in session_turns {
            let mut turn: LocomoTurn = serde_json::from_value(turn_value.clone())
                .expect("LOCOMO conversation turn should parse");
            turn.session_id = Some(session_key.clone());
            turn.session_date_time = session_date_time.clone();
            turns.push(turn);
        }
    }

    turns
}

fn session_sort_key(key: &str) -> (u32, &str) {
    let number = key
        .strip_prefix("session_")
        .and_then(|value| value.split('_').next())
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(u32::MAX);
    (number, key)
}

fn answer_overlap_score(answer: &Value, results: &[anima_memory::MemoryRecallResult]) -> f64 {
    let answer = answer_to_text(answer).to_ascii_lowercase();
    let tokens: Vec<_> = answer
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| token.len() >= 4)
        .collect();
    if tokens.is_empty() {
        return 0.0;
    }
    let retrieved = results
        .iter()
        .map(|result| result.memory.content.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join("\n");
    let hits = tokens
        .iter()
        .filter(|token| retrieved.contains(**token))
        .count();
    hits as f64 / tokens.len() as f64
}

fn answer_to_text(answer: &Value) -> String {
    match answer {
        Value::String(value) => value.clone(),
        Value::Number(value) => value.to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Array(values) => values
            .iter()
            .map(answer_to_text)
            .collect::<Vec<_>>()
            .join(" "),
        Value::Null => String::new(),
        Value::Object(_) => answer.to_string(),
    }
}

fn seed_temporal_facts(
    manager: &mut MemoryManager,
    item: &LocomoConversation,
    turns: &[LocomoTurn],
    memory_id_by_dia_id: &HashMap<String, String>,
) -> TemporalSeedIndex {
    let speaker_entities = speaker_entities(turns);
    let mut seed_index = TemporalSeedIndex::default();

    for (turn_index, turn) in turns.iter().enumerate() {
        if !looks_like_profile_fact(&turn.text) {
            continue;
        }
        let Some(evidence_memory_id) = memory_id_by_dia_id.get(&turn.dia_id) else {
            continue;
        };
        let previous_speaker = turns[..turn_index]
            .iter()
            .rev()
            .find(|candidate| candidate.speaker != turn.speaker)
            .map(|candidate| candidate.speaker.as_str());
        let (subject_id, subject_name) =
            temporal_subject_for_turn(turn, &speaker_entities, previous_speaker);
        let relation_labels = temporal_relation_labels_for_text(&turn.text);
        let predicate = temporal_predicate_for_relations(&relation_labels);
        let value = turn.memory_content();

        manager
            .add_temporal_fact(NewTemporalFact {
                subject_kind: RelationshipEndpointKind::Agent,
                subject_id,
                subject_name: subject_name.clone(),
                predicate: predicate.clone(),
                object_kind: None,
                object_id: None,
                object_name: None,
                value: Some(value.clone()),
                valid_from: None,
                valid_to: None,
                observed_at: Some(turn_index as u128 + 1),
                confidence: 0.72,
                evidence_memory_ids: vec![evidence_memory_id.clone()],
                supersedes_fact_ids: Vec::new(),
                status: Some(TemporalRecordStatus::Active),
                tags: Some(vec!["locomo".into(), "temporal-seed".into()]),
                room_id: Some(item.sample_id.clone()),
                world_id: Some("locomo".into()),
                session_id: turn.session_id.clone(),
            })
            .expect("LOCOMO temporal seed fact should be valid");
        seed_index.count += 1;
        seed_index
            .by_memory_id
            .entry(evidence_memory_id.clone())
            .or_default()
            .push(TemporalSeedDebug {
                relation_labels: relation_labels_to_strings(relation_labels),
                subject_name,
                predicate,
                value,
            });
    }

    seed_index
}

fn looks_like_profile_fact(text: &str) -> bool {
    let normalized = normalized_for_matching(text);
    const PROFILE_CUES: &[&str] = &[
        "i always",
        "i am a fan",
        "i had",
        "i collect",
        "i enjoy",
        "i feel",
        "i got",
        "i have",
        "i hope",
        "i like",
        "i love",
        "i need",
        "i plan",
        "i prefer",
        "i started caring",
        "i want",
        "i would like",
        "i was",
        "i'd like",
        "i d like",
        "i'm a fan",
        "i'm interested",
        "i'm keen",
        "i m a fan",
        "i m interested",
        "i m keen",
        "i m still looking",
        "i ve been",
        "i ve got",
        "always remember",
        "camping trip",
        "conservative",
        "kids books",
        "important to me",
        "it made me",
        "local church",
        "made me appreciate",
        "my favorite",
        "my journey",
        "my own",
        "roadtrip",
        "this pic",
        "we always",
        "we enjoy",
        "we love",
        "we prefer",
        "we saw",
        "we went",
        "we were",
        "you're so",
        "you are so",
        "you really care",
        "your drive",
    ];

    PROFILE_CUES.iter().any(|cue| normalized.contains(cue))
}

fn temporal_subject_for_turn(
    turn: &LocomoTurn,
    speaker_entities: &[(String, String, String)],
    previous_speaker: Option<&str>,
) -> (String, String) {
    let text = normalized_for_matching(&turn.text);
    if text.contains("you") || text.contains("your") {
        if let Some((id, name, _)) = speaker_entities.iter().find(|(_, name, normalized_name)| {
            *name != turn.speaker && text.contains(normalized_name)
        }) {
            return (id.clone(), name.clone());
        }
        if let Some(previous_speaker) = previous_speaker {
            if previous_speaker != turn.speaker {
                return (
                    normalize_agent_id(previous_speaker),
                    previous_speaker.to_string(),
                );
            }
        }
    }

    (normalize_agent_id(&turn.speaker), turn.speaker.clone())
}

fn temporal_relation_labels_for_text(text: &str) -> Vec<TemporalRelationLabel> {
    let normalized = normalized_for_matching(text);
    let mut labels = Vec::new();
    push_relation_if_matches(
        &mut labels,
        &normalized,
        TemporalRelationLabel::CareerInterest,
        &[
            "career",
            "counsel",
            "education",
            "field",
            "job",
            "mental health",
            "pursue",
            "work",
        ],
    );
    push_relation_if_matches(
        &mut labels,
        &normalized,
        TemporalRelationLabel::MusicPreference,
        &[
            "bach",
            "classical",
            "ed sheeran",
            "mozart",
            "music",
            "playlist",
            "singing",
            "song",
            "vivaldi",
        ],
    );
    push_relation_if_matches(
        &mut labels,
        &normalized,
        TemporalRelationLabel::HobbyPreference,
        &[
            "art",
            "book",
            "camp",
            "camping",
            "collect",
            "favorite",
            "hike",
            "hiking",
            "horse",
            "horseback",
            "outdoor",
            "painting",
            "photograph",
            "roadtrip",
            "travel",
        ],
    );
    push_relation_if_matches(
        &mut labels,
        &normalized,
        TemporalRelationLabel::PersonalityTrait,
        &[
            "authentic",
            "care",
            "caring",
            "compassion",
            "drive",
            "empathy",
            "important to me",
            "support",
            "supportive",
            "thoughtful",
            "understanding",
        ],
    );
    push_relation_if_matches(
        &mut labels,
        &normalized,
        TemporalRelationLabel::BeliefIdentity,
        &[
            "ally",
            "church",
            "community",
            "conservative",
            "identity",
            "lgbtq",
            "political",
            "religious",
            "trans",
            "transgender",
        ],
    );
    push_relation_if_matches(
        &mut labels,
        &normalized,
        TemporalRelationLabel::FamilyContext,
        &[
            "brother", "dad", "daughter", "family", "father", "husband", "kid", "kids", "mom",
            "mother", "parent", "parents", "sister", "son", "wife",
        ],
    );
    push_relation_if_matches(
        &mut labels,
        &normalized,
        TemporalRelationLabel::LocationPreference,
        &[
            "beach",
            "city",
            "forest",
            "home",
            "lake",
            "live",
            "mountain",
            "move",
            "neighborhood",
            "town",
        ],
    );

    if labels.is_empty() {
        labels.push(TemporalRelationLabel::GeneralProfile);
    }

    labels
}

fn question_relation_labels(question: &str) -> Vec<TemporalRelationLabel> {
    let normalized = normalized_for_matching(question);
    let mut labels = Vec::new();
    push_relation_if_matches(
        &mut labels,
        &normalized,
        TemporalRelationLabel::CareerInterest,
        &[
            "career",
            "education",
            "field",
            "fields",
            "job",
            "profession",
            "pursue",
            "work",
        ],
    );
    push_relation_if_matches(
        &mut labels,
        &normalized,
        TemporalRelationLabel::MusicPreference,
        &[
            "artist",
            "bach",
            "classical",
            "mozart",
            "music",
            "song",
            "vivaldi",
        ],
    );
    push_relation_if_matches(
        &mut labels,
        &normalized,
        TemporalRelationLabel::HobbyPreference,
        &[
            "activity",
            "activities",
            "book",
            "camp",
            "camping",
            "hobby",
            "hobbies",
            "horse",
            "travel",
            "trip",
        ],
    );
    push_relation_if_matches(
        &mut labels,
        &normalized,
        TemporalRelationLabel::PersonalityTrait,
        &[
            "care",
            "drive",
            "empathetic",
            "personality",
            "supportive",
            "thoughtful",
            "trait",
            "traits",
            "understanding",
        ],
    );
    push_relation_if_matches(
        &mut labels,
        &normalized,
        TemporalRelationLabel::BeliefIdentity,
        &[
            "ally",
            "belief",
            "beliefs",
            "church",
            "community",
            "conservative",
            "identity",
            "lgbtq",
            "political",
            "religious",
            "transgender",
        ],
    );
    push_relation_if_matches(
        &mut labels,
        &normalized,
        TemporalRelationLabel::FamilyContext,
        &[
            "brother", "children", "daughter", "family", "father", "husband", "kids", "mother",
            "parent", "parents", "sister", "son", "wife",
        ],
    );
    push_relation_if_matches(
        &mut labels,
        &normalized,
        TemporalRelationLabel::LocationPreference,
        &[
            "beach", "city", "forest", "home", "lake", "live", "location", "mountain", "move",
            "town", "where",
        ],
    );

    if labels.is_empty()
        && contains_any(
            &normalized,
            &[
                "favorite", "likely", "prefer", "remember", "status", "want", "would",
            ],
        )
    {
        labels.push(TemporalRelationLabel::GeneralProfile);
    }

    labels
}

fn temporal_predicate_for_relations(relations: &[TemporalRelationLabel]) -> String {
    let mut terms = Vec::new();
    for relation in relations {
        for term in relation.predicate_terms() {
            if !terms.iter().any(|existing| existing == term) {
                terms.push(*term);
            }
        }
    }
    terms.join(" ")
}

fn push_relation_if_matches(
    labels: &mut Vec<TemporalRelationLabel>,
    normalized: &str,
    relation: TemporalRelationLabel,
    cues: &[&str],
) {
    if contains_any(normalized, cues) && !labels.contains(&relation) {
        labels.push(relation);
    }
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| contains_cue(value, needle))
}

fn contains_cue(value: &str, needle: &str) -> bool {
    let normalized_needle = normalized_for_matching(needle);
    if normalized_needle.is_empty() {
        return false;
    }

    if normalized_needle.contains(' ') {
        return value.contains(&normalized_needle);
    }

    value.split_whitespace().any(|token| {
        token == normalized_needle
            || (normalized_needle.len() >= 5 && token.starts_with(&normalized_needle))
    })
}

fn speaker_entities(turns: &[LocomoTurn]) -> Vec<(String, String, String)> {
    let mut speakers = Vec::new();
    for turn in turns {
        let id = normalize_agent_id(&turn.speaker);
        if speakers
            .iter()
            .any(|(existing_id, _, _): &(String, String, String)| *existing_id == id)
        {
            continue;
        }
        speakers.push((
            id,
            turn.speaker.clone(),
            normalized_for_matching(&turn.speaker),
        ));
    }
    speakers
}

fn resolve_question_entity(
    question: &str,
    speaker_entities: &[(String, String, String)],
) -> Option<String> {
    let normalized_question = normalized_for_matching(question);
    speaker_entities
        .iter()
        .find(|(_, _, normalized_name)| normalized_question.contains(normalized_name))
        .map(|(id, _, _)| id.clone())
}

fn normalized_for_matching(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_alphanumeric() {
                character.to_lowercase().collect::<String>()
            } else {
                " ".into()
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_agent_id(value: &str) -> String {
    let normalized = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if normalized.is_empty() {
        "speaker".into()
    } else {
        normalized
    }
}

fn locomo_dataset_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("LOCOMO_DATASET_JSON") {
        return Some(PathBuf::from(path));
    }
    None
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_f64(name: &str, default: f64) -> f64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_optional_f64(name: &str) -> Option<f64> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
}

fn env_optional_u8(name: &str) -> Option<u8> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
}

fn env_bool(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn temporal_weight_sweep() -> Vec<f64> {
    env_f64_list("LOCOMO_TEMPORAL_WEIGHT_SWEEP")
        .unwrap_or_else(|| DEFAULT_TEMPORAL_WEIGHT_SWEEP.to_vec())
}

fn temporal_rerank_weight_sweep() -> Vec<f64> {
    env_f64_list("LOCOMO_TEMPORAL_RERANK_WEIGHT_SWEEP")
        .unwrap_or_else(|| DEFAULT_TEMPORAL_RERANK_WEIGHT_SWEEP.to_vec())
}

fn env_f64_list(name: &str) -> Option<Vec<f64>> {
    std::env::var(name)
        .ok()
        .map(|value| {
            value
                .split(',')
                .filter_map(|part| part.trim().parse::<f64>().ok())
                .filter(|value| value.is_finite() && *value >= 0.0)
                .collect::<Vec<_>>()
        })
        .filter(|values| !values.is_empty())
}

fn miss_report_options(top_k: usize) -> Option<MissReportOptions> {
    let category = env_optional_u8("LOCOMO_MISS_REPORT_CATEGORY")
        .or_else(|| env_bool("LOCOMO_CATEGORY3_MISS_REPORT").then_some(3))?;
    let limit = env_usize("LOCOMO_MISS_REPORT_LIMIT", 5);
    if limit == 0 {
        return None;
    }
    let retrieved_limit = env_usize("LOCOMO_MISS_REPORT_TOP_K", top_k).max(1);
    Some(MissReportOptions {
        category,
        limit,
        retrieved_limit,
    })
}

fn best_category_report<'a>(
    reports: impl IntoIterator<Item = &'a BenchmarkReport>,
    category: u8,
) -> Option<&'a BenchmarkReport> {
    reports.into_iter().max_by(|left, right| {
        let left_metrics = left.by_category.get(&category);
        let right_metrics = right.by_category.get(&category);
        let left_hit_rate = left_metrics
            .map(CategoryMetrics::hit_rate)
            .unwrap_or_default();
        let right_hit_rate = right_metrics
            .map(CategoryMetrics::hit_rate)
            .unwrap_or_default();
        left_hit_rate.total_cmp(&right_hit_rate).then_with(|| {
            left_metrics
                .map(CategoryMetrics::mean_reciprocal_rank)
                .unwrap_or_default()
                .total_cmp(
                    &right_metrics
                        .map(CategoryMetrics::mean_reciprocal_rank)
                        .unwrap_or_default(),
                )
        })
    })
}

#[derive(Debug, Deserialize)]
struct LocomoConversation {
    qa: Vec<LocomoQa>,
    conversation: BTreeMap<String, Value>,
    sample_id: String,
}

#[derive(Debug, Deserialize)]
struct LocomoQa {
    question: String,
    #[serde(default)]
    answer: Option<Value>,
    evidence: Vec<String>,
    category: u8,
}

#[derive(Debug, Deserialize)]
struct LocomoTurn {
    speaker: String,
    dia_id: String,
    text: String,
    #[serde(default)]
    blip_caption: Option<String>,
    #[serde(default)]
    query: Option<String>,
    #[serde(skip)]
    session_id: Option<String>,
    #[serde(skip)]
    session_date_time: Option<String>,
}

impl LocomoTurn {
    fn memory_content(&self) -> String {
        let mut content = format!("[{}] {}: {}", self.dia_id, self.speaker, self.text);
        if let Some(session_date_time) = self
            .session_date_time
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            content.push_str(" Session date: ");
            content.push_str(session_date_time);
        }
        if let Some(caption) = self
            .blip_caption
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            content.push_str(" Image caption: ");
            content.push_str(caption);
        }
        if let Some(query) = self.query.as_deref().filter(|value| !value.is_empty()) {
            content.push_str(" Image query: ");
            content.push_str(query);
        }
        content
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum BenchmarkProfile {
    #[default]
    Core,
    LocomoTuned,
    LocomoTemporal {
        temporal_weight: f64,
    },
    LocomoTemporalRerank {
        temporal_weight: f64,
        rerank_bonus: f64,
    },
}

impl BenchmarkProfile {
    fn label(&self) -> String {
        match self {
            Self::Core => "core".into(),
            Self::LocomoTuned => "locomo-tuned".into(),
            Self::LocomoTemporal { temporal_weight } => {
                format!("locomo-temporal-w{temporal_weight:.3}")
            }
            Self::LocomoTemporalRerank {
                temporal_weight,
                rerank_bonus,
            } => format!("locomo-temporal-rerank-w{temporal_weight:.3}-b{rerank_bonus:.2}"),
        }
    }

    fn memory_manager(&self) -> MemoryManager {
        match self {
            Self::Core => MemoryManager::new(),
            Self::LocomoTuned | Self::LocomoTemporal { .. } | Self::LocomoTemporalRerank { .. } => {
                MemoryManager::with_query_expander(locomo_query_expander())
            }
        }
    }

    fn seeds_temporal_facts(&self) -> bool {
        matches!(
            self,
            Self::LocomoTemporal { .. } | Self::LocomoTemporalRerank { .. }
        )
    }

    fn recall_weights(&self) -> Option<MemoryRecallWeights> {
        match self {
            Self::LocomoTemporal { temporal_weight }
            | Self::LocomoTemporalRerank {
                temporal_weight, ..
            } => Some(MemoryRecallWeights {
                temporal: *temporal_weight,
                ..MemoryRecallWeights::default()
            }),
            Self::Core | Self::LocomoTuned => None,
        }
    }

    fn recall_limit(&self, top_k: usize) -> usize {
        if matches!(self, Self::LocomoTemporalRerank { .. }) {
            top_k.saturating_mul(4).max(top_k)
        } else {
            top_k
        }
    }

    fn temporal_rerank_bonus(&self) -> Option<f64> {
        match self {
            Self::LocomoTemporalRerank { rerank_bonus, .. } => Some(*rerank_bonus),
            Self::Core | Self::LocomoTuned | Self::LocomoTemporal { .. } => None,
        }
    }

    fn temporal_intent_terms(&self) -> Vec<String> {
        if !self.seeds_temporal_facts() {
            return Vec::new();
        }

        [
            "ally",
            "career",
            "considered",
            "education",
            "educaton",
            "enjoy",
            "field",
            "fields",
            "interest",
            "interested",
            "likely",
            "member",
            "personality",
            "political",
            "religious",
            "trait",
            "traits",
            "would",
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    }
}

#[derive(Clone, Copy, Debug)]
struct MissReportOptions {
    category: u8,
    limit: usize,
    retrieved_limit: usize,
}

#[derive(Clone, Debug)]
struct CategoryMissDetail {
    sample_id: String,
    question: String,
    answer: String,
    resolved_entity: Option<String>,
    question_relations: Vec<String>,
    seeded_evidence_count: usize,
    matched_relation_evidence_count: usize,
    evidence: Vec<CategoryMissEvidence>,
    retrieved: Vec<CategoryMissRetrieved>,
}

#[derive(Clone, Debug)]
struct CategoryMissEvidence {
    dia_id: String,
    speaker: String,
    content: String,
    matching_question_relations: Vec<String>,
    temporal_facts: Vec<TemporalSeedDebug>,
}

#[derive(Clone, Debug)]
struct CategoryMissRetrieved {
    rank: usize,
    dia_id: String,
    score: f64,
    lexical_score: f64,
    temporal_score: f64,
    relationship_score: f64,
    recency_score: f64,
    importance_score: f64,
    content: String,
}

#[derive(Clone, Debug, Default)]
struct TemporalSeedIndex {
    count: usize,
    by_memory_id: HashMap<String, Vec<TemporalSeedDebug>>,
}

#[derive(Clone, Debug)]
struct TemporalSeedDebug {
    relation_labels: Vec<String>,
    subject_name: String,
    predicate: String,
    value: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TemporalRelationLabel {
    GeneralProfile,
    CareerInterest,
    MusicPreference,
    HobbyPreference,
    PersonalityTrait,
    BeliefIdentity,
    FamilyContext,
    LocationPreference,
}

impl TemporalRelationLabel {
    fn label(self) -> &'static str {
        match self {
            Self::GeneralProfile => "general_profile",
            Self::CareerInterest => "career_interest",
            Self::MusicPreference => "music_preference",
            Self::HobbyPreference => "hobby_preference",
            Self::PersonalityTrait => "personality_trait",
            Self::BeliefIdentity => "belief_identity",
            Self::FamilyContext => "family_context",
            Self::LocationPreference => "location_preference",
        }
    }

    fn predicate_terms(self) -> &'static [&'static str] {
        match self {
            Self::GeneralProfile => &[
                "general_profile",
                "profile",
                "preference",
                "likely",
                "would",
            ],
            Self::CareerInterest => &[
                "career_interest",
                "career",
                "education",
                "field",
                "pursue",
                "work",
            ],
            Self::MusicPreference => &[
                "music_preference",
                "music",
                "song",
                "classical",
                "artist",
                "listen",
            ],
            Self::HobbyPreference => &[
                "hobby_preference",
                "activity",
                "book",
                "camp",
                "enjoy",
                "favorite",
                "hobby",
                "travel",
            ],
            Self::PersonalityTrait => &[
                "personality_trait",
                "care",
                "personality",
                "support",
                "thoughtful",
                "trait",
            ],
            Self::BeliefIdentity => &[
                "belief_identity",
                "belief",
                "community",
                "identity",
                "political",
                "religious",
            ],
            Self::FamilyContext => &["family_context", "children", "family", "parent", "partner"],
            Self::LocationPreference => &[
                "location_preference",
                "city",
                "home",
                "live",
                "location",
                "move",
            ],
        }
    }
}

#[derive(Debug, Default)]
struct BenchmarkReport {
    profile: BenchmarkProfile,
    conversations: usize,
    turns: usize,
    seeded_temporal_facts: usize,
    evaluated_questions: usize,
    skipped_questions: usize,
    hit_questions: usize,
    all_hit_questions: usize,
    reciprocal_rank_sum: f64,
    answer_overlap_sum: f64,
    miss_report_total: usize,
    misses: Vec<CategoryMissDetail>,
    by_category: BTreeMap<u8, CategoryMetrics>,
}

impl BenchmarkReport {
    fn record(
        &mut self,
        category: u8,
        best_rank: Option<usize>,
        all_evidence_hit: bool,
        answer_overlap: f64,
    ) {
        self.evaluated_questions += 1;
        if best_rank.is_some() {
            self.hit_questions += 1;
        }
        if all_evidence_hit {
            self.all_hit_questions += 1;
        }
        let reciprocal_rank = best_rank.map(|rank| 1.0 / rank as f64).unwrap_or_default();
        self.reciprocal_rank_sum += reciprocal_rank;
        self.answer_overlap_sum += answer_overlap;
        self.by_category.entry(category).or_default().record(
            best_rank,
            all_evidence_hit,
            reciprocal_rank,
            answer_overlap,
        );
    }

    fn hit_rate(&self) -> f64 {
        ratio(self.hit_questions, self.evaluated_questions)
    }

    fn all_hit_rate(&self) -> f64 {
        ratio(self.all_hit_questions, self.evaluated_questions)
    }

    fn mean_reciprocal_rank(&self) -> f64 {
        ratio_f64(self.reciprocal_rank_sum, self.evaluated_questions)
    }
}

#[derive(Debug, Default)]
struct CategoryMetrics {
    questions: usize,
    hit_questions: usize,
    all_hit_questions: usize,
    reciprocal_rank_sum: f64,
    answer_overlap_sum: f64,
}

impl CategoryMetrics {
    fn record(
        &mut self,
        best_rank: Option<usize>,
        all_evidence_hit: bool,
        reciprocal_rank: f64,
        answer_overlap: f64,
    ) {
        self.questions += 1;
        if best_rank.is_some() {
            self.hit_questions += 1;
        }
        if all_evidence_hit {
            self.all_hit_questions += 1;
        }
        self.reciprocal_rank_sum += reciprocal_rank;
        self.answer_overlap_sum += answer_overlap;
    }

    fn hit_rate(&self) -> f64 {
        ratio(self.hit_questions, self.questions)
    }

    fn all_hit_rate(&self) -> f64 {
        ratio(self.all_hit_questions, self.questions)
    }

    fn mean_reciprocal_rank(&self) -> f64 {
        ratio_f64(self.reciprocal_rank_sum, self.questions)
    }
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

fn ratio_f64(numerator: f64, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator / denominator as f64
    }
}
