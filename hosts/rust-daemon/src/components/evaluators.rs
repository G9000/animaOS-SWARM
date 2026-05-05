use std::collections::BTreeMap;

use anima_core::{AgentRuntime, Content, DataValue, Evaluator, EvaluatorResult, Message};
use anima_memory::{
    MemoryEvaluationDecision, MemoryEvaluationOptions, MemoryType, NewAgentRelationship, NewMemory,
    NewTemporalFact, RelationshipEndpointKind,
};
use async_trait::async_trait;
use tracing::warn;

use crate::memory_embeddings::SharedMemoryEmbeddings;
use crate::memory_store::{save_memory_manager, MemoryStoreConfig};
use crate::state::SharedMemoryStore;

pub(super) struct ReflectionMemoryEvaluator {
    pub(super) memory: SharedMemoryStore,
    pub(super) memory_embeddings: SharedMemoryEmbeddings,
    pub(super) memory_store: Option<MemoryStoreConfig>,
}

#[derive(Clone, Debug, PartialEq)]
struct ExtractedUserMemory {
    memory_content: String,
    temporal_predicate: String,
    temporal_value: String,
    relation_labels: Vec<String>,
}

#[async_trait]
impl Evaluator for ReflectionMemoryEvaluator {
    fn name(&self) -> &str {
        "reflection_memory"
    }

    fn description(&self) -> &str {
        "Persists evaluated memory and relationship evidence for completed responses"
    }

    async fn validate(&self, _runtime: &AgentRuntime, _message: &Message) -> Result<bool, String> {
        Ok(true)
    }

    async fn evaluate(
        &self,
        runtime: &AgentRuntime,
        message: &Message,
        response: &Content,
    ) -> Result<EvaluatorResult, String> {
        if response.text.trim().is_empty() {
            return Ok(EvaluatorResult::default());
        }

        let state = runtime.state();
        let user_id = metadata_string(message, &["userId", "user_id"]);
        let user_name = metadata_string(message, &["userName", "user_name"])
            .or_else(|| user_id.clone())
            .unwrap_or_else(|| "User".into());
        let world_id = metadata_string(message, &["worldId", "world_id"]);
        let session_id = metadata_string(message, &["sessionId", "session_id"]);
        let user_text = compact_text(&message.content.text, 500);
        let response_text = compact_text(&response.text, 900);
        let reflection = format!("evaluated response: {response_text}\nuser request: {user_text}");
        let extracted_user_memory = extract_explicit_user_memory(&user_text);
        let mut tags = vec!["runtime".into(), "memory-evaluator".into()];
        if user_id.is_some() {
            tags.push("agent-user".into());
        }

        let (outcome, extracted_outcome, extracted_temporal_fact, relationship, embedded_memories) = {
            let mut memory_guard = self.memory.write().await;
            let outcome = memory_guard
                .add_evaluated(
                    NewMemory {
                        agent_id: state.id.clone(),
                        agent_name: state.name.clone(),
                        memory_type: MemoryType::Reflection,
                        content: reflection,
                        importance: 0.65,
                        tags: Some(tags.clone()),
                        scope: None,
                        room_id: Some(message.room_id.clone()),
                        world_id: world_id.clone(),
                        session_id: session_id.clone(),
                    },
                    MemoryEvaluationOptions::default(),
                )
                .map_err(|error| error.message().to_string())?;
            let extracted_outcome = match extracted_user_memory.as_ref() {
                Some(extraction) => Some(
                    memory_guard
                        .add_evaluated(
                            NewMemory {
                                agent_id: state.id.clone(),
                                agent_name: state.name.clone(),
                                memory_type: MemoryType::Fact,
                                content: extraction.memory_content.clone(),
                                importance: 0.72,
                                tags: Some(extracted_memory_tags(extraction)),
                                scope: None,
                                room_id: Some(message.room_id.clone()),
                                world_id: world_id.clone(),
                                session_id: session_id.clone(),
                            },
                            MemoryEvaluationOptions {
                                min_content_chars: 8,
                                min_importance: 0.2,
                            },
                        )
                        .map_err(|error| error.message().to_string())?,
                ),
                None => None,
            };
            let extracted_temporal_fact = match (
                user_id.as_ref(),
                extracted_user_memory.as_ref(),
                extracted_outcome.as_ref().and_then(outcome_memory_id),
            ) {
                (Some(user_id), Some(extraction), Some(evidence_memory_id)) => Some(
                    memory_guard
                        .add_temporal_fact(NewTemporalFact {
                            subject_kind: RelationshipEndpointKind::User,
                            subject_id: user_id.clone(),
                            subject_name: user_name.clone(),
                            predicate: extraction.temporal_predicate.clone(),
                            object_kind: None,
                            object_id: None,
                            object_name: None,
                            value: Some(extraction.temporal_value.clone()),
                            valid_from: None,
                            valid_to: None,
                            observed_at: None,
                            confidence: 0.78,
                            evidence_memory_ids: vec![evidence_memory_id],
                            supersedes_fact_ids: Vec::new(),
                            status: None,
                            tags: Some(extracted_memory_tags(extraction)),
                            room_id: Some(message.room_id.clone()),
                            world_id: world_id.clone(),
                            session_id: session_id.clone(),
                        })
                        .map_err(|error| error.message().to_string())?,
                ),
                _ => None,
            };
            let mut evidence_memory_ids = Vec::new();
            push_outcome_memory_id(&mut evidence_memory_ids, &outcome);
            if let Some(extracted_outcome) = &extracted_outcome {
                push_outcome_memory_id(&mut evidence_memory_ids, extracted_outcome);
            }
            let relationship_tags = relationship_tags(&tags, extracted_user_memory.as_ref());
            let relationship = match (user_id.as_ref(), evidence_memory_ids.is_empty()) {
                (Some(user_id), false) => Some(
                    memory_guard
                        .upsert_agent_relationship(NewAgentRelationship {
                            source_kind: Some(RelationshipEndpointKind::Agent),
                            source_agent_id: state.id.clone(),
                            source_agent_name: state.name.clone(),
                            target_kind: Some(RelationshipEndpointKind::User),
                            target_agent_id: user_id.clone(),
                            target_agent_name: user_name.clone(),
                            relationship_type: "responds_to".into(),
                            summary: Some(format!(
                                "{} responded to {} in {}.",
                                runtime.config().name,
                                user_name,
                                message.room_id
                            )),
                            strength: 0.55,
                            confidence: 0.75,
                            evidence_memory_ids,
                            tags: Some(relationship_tags),
                            room_id: Some(message.room_id.clone()),
                            world_id,
                            session_id,
                        })
                        .map_err(|error| error.message().to_string())?,
                ),
                _ => None,
            };
            save_memory_manager(self.memory_store.as_ref(), &memory_guard)
                .map_err(|error| format!("failed to persist evaluated memory: {error}"))?;
            let embedded_memories = outcome
                .memory
                .iter()
                .chain(
                    extracted_outcome
                        .iter()
                        .filter_map(|outcome| outcome.memory.as_ref()),
                )
                .cloned()
                .collect::<Vec<_>>();
            (
                outcome,
                extracted_outcome,
                extracted_temporal_fact,
                relationship,
                embedded_memories,
            )
        };

        if !embedded_memories.is_empty() {
            let mut embeddings = self.memory_embeddings.write().await;
            for memory in &embedded_memories {
                if let Err(error) = embeddings.upsert_memory(memory) {
                    warn!(
                        memory_id = %memory.id,
                        error = %error,
                        "failed to index evaluated memory embedding"
                    );
                }
            }
        }

        let mut metadata = BTreeMap::new();
        metadata.insert(
            "memoryEvaluationDecision".into(),
            DataValue::String(evaluation_decision_label(outcome.evaluation.decision).into()),
        );
        if let Some(memory) = &outcome.memory {
            metadata.insert("memoryId".into(), DataValue::String(memory.id.clone()));
        }
        if let Some(duplicate_id) = &outcome.evaluation.duplicate_memory_id {
            metadata.insert(
                "duplicateMemoryId".into(),
                DataValue::String(duplicate_id.clone()),
            );
        }
        if let Some(extracted_outcome) = &extracted_outcome {
            if let Some(memory) = &extracted_outcome.memory {
                metadata.insert(
                    "extractedMemoryId".into(),
                    DataValue::String(memory.id.clone()),
                );
            }
            if let Some(duplicate_id) = &extracted_outcome.evaluation.duplicate_memory_id {
                metadata.insert(
                    "extractedDuplicateMemoryId".into(),
                    DataValue::String(duplicate_id.clone()),
                );
            }
        }
        if let Some(fact) = &extracted_temporal_fact {
            metadata.insert(
                "extractedTemporalFactId".into(),
                DataValue::String(fact.id.clone()),
            );
            metadata.insert(
                "extractedTemporalPredicate".into(),
                DataValue::String(fact.predicate.clone()),
            );
        }

        let feedback = if let Some(relationship) = relationship {
            metadata.insert("relationshipId".into(), DataValue::String(relationship.id));
            if let Some(user_id) = user_id {
                metadata.insert("targetUserId".into(), DataValue::String(user_id));
            }
            match outcome.evaluation.decision {
                MemoryEvaluationDecision::Store => "stored evaluated memory and user relationship",
                MemoryEvaluationDecision::Merge => {
                    "merged evaluated memory and updated user relationship"
                }
                MemoryEvaluationDecision::Ignore => "ignored evaluated memory",
            }
        } else {
            match outcome.evaluation.decision {
                MemoryEvaluationDecision::Store => "stored evaluated memory",
                MemoryEvaluationDecision::Merge => "merged evaluated memory",
                MemoryEvaluationDecision::Ignore => "ignored evaluated memory",
            }
        };

        metadata.insert(
            "evaluationSummary".into(),
            DataValue::String(feedback.into()),
        );

        Ok(EvaluatorResult {
            metadata: Some(metadata),
            ..EvaluatorResult::accept()
        })
    }
}

fn evaluation_decision_label(decision: MemoryEvaluationDecision) -> &'static str {
    match decision {
        MemoryEvaluationDecision::Store => "store",
        MemoryEvaluationDecision::Merge => "merge",
        MemoryEvaluationDecision::Ignore => "ignore",
    }
}

fn push_outcome_memory_id(
    memory_ids: &mut Vec<String>,
    outcome: &anima_memory::MemoryEvaluationOutcome,
) {
    let id = outcome_memory_id(outcome);
    if let Some(id) = id {
        memory_ids.push(id);
    }
}

fn outcome_memory_id(outcome: &anima_memory::MemoryEvaluationOutcome) -> Option<String> {
    outcome
        .memory
        .as_ref()
        .map(|memory| memory.id.clone())
        .or_else(|| outcome.evaluation.duplicate_memory_id.clone())
}

fn extract_explicit_user_memory(text: &str) -> Option<ExtractedUserMemory> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let normalized = normalized_for_matching(text);
    let remember_request =
        normalized.starts_with("remember ") || normalized.starts_with("please remember ");
    let preference_statement = contains_any(
        &normalized,
        &[
            "i am a fan",
            "i enjoy",
            "i like",
            "i love",
            "i prefer",
            "i want",
            "my favorite",
            "my preference is",
        ],
    );
    let profile_statement = contains_any(
        &normalized,
        &[
            "i am",
            "i care about",
            "i identify",
            "i plan",
            "i work",
            "i study",
            "i m",
            "my",
        ],
    );
    if !remember_request && !preference_statement && !profile_statement {
        return None;
    }

    let mut relation_labels = classify_user_relation_labels(&normalized);
    if relation_labels.is_empty() {
        relation_labels.push(if preference_statement {
            "general_preference".to_string()
        } else {
            "general_profile".to_string()
        });
    }

    let memory_content = if remember_request {
        format!("user stated memory: {text}")
    } else if preference_statement {
        format!("user stated preference: {text}")
    } else {
        format!("user stated profile: {text}")
    };

    Some(ExtractedUserMemory {
        memory_content,
        temporal_predicate: relation_labels.join(" "),
        temporal_value: text.to_string(),
        relation_labels,
    })
}

fn extracted_memory_tags(extraction: &ExtractedUserMemory) -> Vec<String> {
    let mut tags = vec![
        "runtime".into(),
        "memory-evaluator".into(),
        "user-stated".into(),
    ];
    for relation_label in &extraction.relation_labels {
        let tag = format!("relation:{relation_label}");
        if !tags.contains(&tag) {
            tags.push(tag);
        }
    }
    tags
}

fn relationship_tags(
    base_tags: &[String],
    extraction: Option<&ExtractedUserMemory>,
) -> Vec<String> {
    let mut tags = base_tags.to_vec();
    if let Some(extraction) = extraction {
        for tag in extracted_memory_tags(extraction) {
            if !tags.contains(&tag) {
                tags.push(tag);
            }
        }
    }
    tags
}

fn classify_user_relation_labels(normalized: &str) -> Vec<String> {
    let mut labels = Vec::new();
    push_relation_if_matches(
        &mut labels,
        normalized,
        "communication_preference",
        &[
            "brief",
            "release summaries",
            "summary",
            "summaries",
            "terse",
            "verbose",
        ],
    );
    push_relation_if_matches(
        &mut labels,
        normalized,
        "music_preference",
        &[
            "bach",
            "classical",
            "jazz",
            "mozart",
            "music",
            "playlist",
            "song",
            "vivaldi",
        ],
    );
    push_relation_if_matches(
        &mut labels,
        normalized,
        "career_interest",
        &[
            "career",
            "counsel",
            "education",
            "field",
            "job",
            "mental health",
            "profession",
            "study",
            "work",
        ],
    );
    push_relation_if_matches(
        &mut labels,
        normalized,
        "hobby_preference",
        &[
            "book", "camp", "camping", "game", "gaming", "hobby", "movie", "movies", "painting",
            "travel",
        ],
    );
    push_relation_if_matches(
        &mut labels,
        normalized,
        "personality_trait",
        &[
            "caring",
            "curious",
            "empathetic",
            "organized",
            "patient",
            "supportive",
            "thoughtful",
            "understanding",
        ],
    );
    push_relation_if_matches(
        &mut labels,
        normalized,
        "belief_identity",
        &[
            "atheist",
            "christian",
            "conservative",
            "identity",
            "lgbtq",
            "muslim",
            "queer",
            "trans",
            "transgender",
        ],
    );
    push_relation_if_matches(
        &mut labels,
        normalized,
        "family_context",
        &[
            "brother", "daughter", "family", "father", "husband", "kid", "kids", "mom", "mother",
            "parent", "parents", "sister", "son", "wife",
        ],
    );
    push_relation_if_matches(
        &mut labels,
        normalized,
        "location_preference",
        &[
            "beach",
            "city",
            "country",
            "forest",
            "home",
            "lake",
            "live in",
            "living in",
            "mountain",
            "move to",
        ],
    );
    labels
}

fn push_relation_if_matches(
    labels: &mut Vec<String>,
    normalized: &str,
    relation_label: &str,
    cues: &[&str],
) {
    if contains_any(normalized, cues) && !labels.iter().any(|existing| existing == relation_label) {
        labels.push(relation_label.to_string());
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

fn metadata_string(message: &Message, keys: &[&str]) -> Option<String> {
    let metadata = message.content.metadata.as_ref()?;
    keys.iter().find_map(|key| match metadata.get(*key) {
        Some(DataValue::String(value)) if !value.trim().is_empty() => {
            Some(value.trim().to_string())
        }
        _ => None,
    })
}

fn compact_text(value: &str, max_chars: usize) -> String {
    let value = value.trim();
    if value.chars().count() <= max_chars {
        return value.to_string();
    }

    let mut compacted: String = value.chars().take(max_chars).collect();
    compacted.push_str("...");
    compacted
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use anima_core::{AgentConfig, MessageRole};
    use anima_memory::{MemoryManager, TemporalFactOptions};
    use tokio::sync::RwLock as AsyncRwLock;

    use crate::memory_embeddings::MemoryEmbeddingRuntime;
    use crate::model::DeterministicModelAdapter;

    #[test]
    fn extract_explicit_user_memory_keeps_existing_preference_text() {
        let extraction = extract_explicit_user_memory("I prefer terse release summaries")
            .expect("preference should extract");

        assert_eq!(
            extraction.memory_content,
            "user stated preference: I prefer terse release summaries"
        );
        assert!(extraction
            .relation_labels
            .contains(&"communication_preference".to_string()));
        assert!(extraction
            .temporal_predicate
            .contains("communication_preference"));
    }

    #[test]
    fn extract_explicit_user_memory_classifies_music_without_substring_noise() {
        let extraction =
            extract_explicit_user_memory("I'm a fan of classical music like Bach and Mozart.")
                .expect("music preference should extract");

        assert!(extraction
            .relation_labels
            .contains(&"music_preference".to_string()));
        assert!(!extraction
            .relation_labels
            .contains(&"hobby_preference".to_string()));
    }

    #[test]
    fn extract_explicit_user_memory_classifies_career_interest() {
        let extraction =
            extract_explicit_user_memory("I want to study counseling and work in mental health.")
                .expect("career interest should extract");

        assert!(extraction
            .relation_labels
            .contains(&"career_interest".to_string()));
    }

    #[test]
    fn extract_explicit_user_memory_classifies_family_context() {
        let extraction = extract_explicit_user_memory("My kids are the center of my family life.")
            .expect("family context should extract");

        assert!(extraction
            .relation_labels
            .contains(&"family_context".to_string()));
    }

    #[test]
    fn extract_explicit_user_memory_avoids_family_substring_false_positive() {
        let extraction = extract_explicit_user_memory(
            "I love listening to Vivaldi's Four Seasons while I work.",
        )
        .expect("music preference should extract");

        assert!(extraction
            .relation_labels
            .contains(&"music_preference".to_string()));
        assert!(!extraction
            .relation_labels
            .contains(&"family_context".to_string()));
    }

    #[tokio::test]
    async fn evaluate_persists_temporal_fact_for_extracted_user_preference() {
        let memory = Arc::new(AsyncRwLock::new(MemoryManager::new()));
        let evaluator = ReflectionMemoryEvaluator {
            memory: memory.clone(),
            memory_embeddings: Arc::new(AsyncRwLock::new(MemoryEmbeddingRuntime::disabled())),
            memory_store: None,
        };
        let runtime = AgentRuntime::new(
            AgentConfig {
                name: "operator".into(),
                model: "gpt-5.4".into(),
                bio: None,
                lore: None,
                knowledge: None,
                topics: None,
                adjectives: None,
                style: None,
                provider: None,
                system: None,
                tools: None,
                plugins: None,
                settings: None,
            },
            Arc::new(DeterministicModelAdapter),
        );
        let mut message_metadata = BTreeMap::new();
        message_metadata.insert("userId".into(), DataValue::String("user-42".into()));
        message_metadata.insert("userName".into(), DataValue::String("Leo".into()));
        let message = Message {
            id: "msg-1".into(),
            agent_id: runtime.id().to_string(),
            room_id: "room-1".into(),
            content: Content {
                text: "I prefer terse release summaries".into(),
                attachments: None,
                metadata: Some(message_metadata),
            },
            role: MessageRole::User,
            created_at_ms: 0,
        };
        let response = Content {
            text: "operator handled task: I prefer terse release summaries".into(),
            attachments: None,
            metadata: None,
        };

        let result = evaluator
            .evaluate(&runtime, &message, &response)
            .await
            .expect("evaluator should succeed");
        let metadata = result.metadata.expect("evaluator should return metadata");
        assert_eq!(
            metadata.get("extractedTemporalPredicate"),
            Some(&DataValue::String("communication_preference".into()))
        );
        assert!(matches!(
            metadata.get("extractedTemporalFactId"),
            Some(DataValue::String(value)) if !value.is_empty()
        ));

        let memory_guard = memory.read().await;
        let facts = memory_guard.list_temporal_facts(TemporalFactOptions {
            subject_kind: Some(RelationshipEndpointKind::User),
            subject_id: Some("user-42".into()),
            limit: Some(5),
            ..TemporalFactOptions::default()
        });

        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].predicate, "communication_preference");
        assert_eq!(
            facts[0].value.as_deref(),
            Some("I prefer terse release summaries")
        );
        assert_eq!(facts[0].room_id.as_deref(), Some("room-1"));
        assert_eq!(facts[0].evidence_memory_ids.len(), 1);
    }

    #[test]
    fn extract_explicit_user_memory_ignores_non_profile_requests() {
        assert!(extract_explicit_user_memory("What is the weather today?").is_none());
    }
}
