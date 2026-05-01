use std::collections::BTreeMap;

use anima_core::{AgentRuntime, Content, DataValue, Evaluator, EvaluatorResult, Message};
use anima_memory::{
    MemoryEvaluationDecision, MemoryEvaluationOptions, MemoryType, NewAgentRelationship, NewMemory,
    RelationshipEndpointKind,
};
use async_trait::async_trait;
use tracing::warn;

use crate::memory_embeddings::SharedMemoryEmbeddings;
use crate::state::SharedMemoryStore;

pub(super) struct ReflectionMemoryEvaluator {
    pub(super) memory: SharedMemoryStore,
    pub(super) memory_embeddings: SharedMemoryEmbeddings,
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

        let (outcome, extracted_outcome, relationship, embedded_memories) = {
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
            let extracted_outcome = match extracted_user_memory {
                Some(content) => Some(
                    memory_guard
                        .add_evaluated(
                            NewMemory {
                                agent_id: state.id.clone(),
                                agent_name: state.name.clone(),
                                memory_type: MemoryType::Fact,
                                content,
                                importance: 0.72,
                                tags: Some(vec![
                                    "runtime".into(),
                                    "memory-evaluator".into(),
                                    "user-stated".into(),
                                ]),
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
            let mut evidence_memory_ids = Vec::new();
            push_outcome_memory_id(&mut evidence_memory_ids, &outcome);
            if let Some(extracted_outcome) = &extracted_outcome {
                push_outcome_memory_id(&mut evidence_memory_ids, extracted_outcome);
            }
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
                            tags: Some(tags),
                            room_id: Some(message.room_id.clone()),
                            world_id,
                            session_id,
                        })
                        .map_err(|error| error.message().to_string())?,
                ),
                _ => None,
            };
            memory_guard
                .save()
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
            (outcome, extracted_outcome, relationship, embedded_memories)
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

        Ok(EvaluatorResult {
            feedback: Some(feedback.into()),
            metadata: Some(metadata),
            ..EvaluatorResult::default()
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
    let id = outcome
        .memory
        .as_ref()
        .map(|memory| memory.id.clone())
        .or_else(|| outcome.evaluation.duplicate_memory_id.clone());
    if let Some(id) = id {
        memory_ids.push(id);
    }
}

fn extract_explicit_user_memory(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let lower = text.to_ascii_lowercase();
    if lower.starts_with("remember ") || lower.starts_with("please remember ") {
        return Some(format!("user stated memory: {text}"));
    }
    if lower.contains("i prefer ")
        || lower.contains("i like ")
        || lower.contains("my preference is ")
    {
        return Some(format!("user stated preference: {text}"));
    }
    None
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
