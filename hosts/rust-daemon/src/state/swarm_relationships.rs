use std::sync::Arc;

use anima_memory::{
    Memory, MemoryScope, MemoryType, NewAgentRelationship, NewMemory, RelationshipEndpointKind,
};
use anima_swarm::{AgentMessage, SwarmConfig};
use tracing::warn;

use crate::memory_embeddings::SharedMemoryEmbeddings;
use crate::memory_store::{save_memory_manager, MemoryStoreConfig};

use super::SharedMemoryStore;

pub(super) async fn persist_swarm_message_relationship(
    memory: SharedMemoryStore,
    memory_embeddings: SharedMemoryEmbeddings,
    memory_store: Option<MemoryStoreConfig>,
    agent_names: Arc<Vec<String>>,
    swarm_id: String,
    message: AgentMessage,
) {
    let source_name = swarm_agent_name(&message.from, &agent_names);
    let target = swarm_message_target(&message, &agent_names, &swarm_id);
    let evidence_content = format!(
        "swarm message: {} -> {}: {}",
        source_name, target.name, message.content.text
    );
    let relationship_summary = format!(
        "{} sent {} a swarm message in {}.",
        source_name, target.name, swarm_id
    );
    let relationship_type = if target.kind == RelationshipEndpointKind::Agent {
        "hands_off_to"
    } else {
        "broadcasts_to"
    };
    let tags = relationship_tags(target.kind);

    let evidence_memory = {
        let mut memory_guard = memory.write().await;
        let evidence_memory = match memory_guard.add(NewMemory {
            agent_id: message.from.clone(),
            agent_name: source_name.clone(),
            memory_type: MemoryType::Observation,
            content: evidence_content,
            importance: 0.58,
            tags: Some(tags.clone()),
            scope: Some(MemoryScope::Room),
            room_id: Some(swarm_id.clone()),
            world_id: Some(format!("swarm:{swarm_id}")),
            session_id: Some(message.id.clone()),
        }) {
            Ok(memory) => memory,
            Err(error) => {
                warn!(
                    swarm_id = %swarm_id,
                    message_id = %message.id,
                    error = %error.message(),
                    "failed to persist swarm message evidence memory"
                );
                return;
            }
        };

        if let Err(error) = memory_guard.upsert_agent_relationship(NewAgentRelationship {
            source_kind: Some(RelationshipEndpointKind::Agent),
            source_agent_id: message.from.clone(),
            source_agent_name: source_name,
            target_kind: Some(target.kind),
            target_agent_id: target.id,
            target_agent_name: target.name,
            relationship_type: relationship_type.into(),
            summary: Some(relationship_summary),
            strength: 0.58,
            confidence: 0.82,
            evidence_memory_ids: vec![evidence_memory.id.clone()],
            tags: Some(tags),
            room_id: Some(swarm_id.clone()),
            world_id: Some(format!("swarm:{swarm_id}")),
            session_id: Some(message.id.clone()),
        }) {
            warn!(
                swarm_id = %swarm_id,
                message_id = %message.id,
                error = %error.message(),
                "failed to persist swarm message relationship"
            );
            return;
        }

        if let Err(error) = save_memory_manager(memory_store.as_ref(), &memory_guard).await {
            warn!(
                swarm_id = %swarm_id,
                message_id = %message.id,
                error = %error,
                "failed to persist swarm message memory store"
            );
        }

        evidence_memory
    };

    index_swarm_message_memory(&memory_embeddings, &evidence_memory).await;
}

pub(super) fn swarm_agent_names(config: &SwarmConfig) -> Vec<String> {
    let mut names = Vec::with_capacity(config.workers.len() + 1);
    names.push(config.manager.name.clone());
    names.extend(config.workers.iter().map(|worker| worker.name.clone()));
    names
}

async fn index_swarm_message_memory(memory_embeddings: &SharedMemoryEmbeddings, memory: &Memory) {
    if let Err(error) = memory_embeddings.write().await.upsert_memory(memory) {
        warn!(
            memory_id = %memory.id,
            error = %error,
            "failed to index swarm message memory embedding"
        );
    }
}

#[derive(Clone)]
struct SwarmRelationshipTarget {
    kind: RelationshipEndpointKind,
    id: String,
    name: String,
}

fn swarm_message_target(
    message: &AgentMessage,
    agent_names: &[String],
    swarm_id: &str,
) -> SwarmRelationshipTarget {
    if message.to == "broadcast" {
        return SwarmRelationshipTarget {
            kind: RelationshipEndpointKind::System,
            id: swarm_id.to_string(),
            name: format!("swarm {swarm_id}"),
        };
    }

    SwarmRelationshipTarget {
        kind: RelationshipEndpointKind::Agent,
        id: message.to.clone(),
        name: swarm_agent_name(&message.to, agent_names),
    }
}

fn relationship_tags(target_kind: RelationshipEndpointKind) -> Vec<String> {
    if target_kind == RelationshipEndpointKind::Agent {
        vec![
            "runtime".into(),
            "swarm".into(),
            "swarm-message".into(),
            "agent-agent".into(),
            "relation:handoff".into(),
        ]
    } else {
        vec![
            "runtime".into(),
            "swarm".into(),
            "swarm-message".into(),
            "agent-swarm".into(),
            "relation:broadcast".into(),
        ]
    }
}

fn swarm_agent_name(agent_id: &str, configured_names: &[String]) -> String {
    configured_names
        .iter()
        .find(|name| is_generated_agent_id_for_name(agent_id, name))
        .cloned()
        .unwrap_or_else(|| fallback_agent_name(agent_id))
}

fn is_generated_agent_id_for_name(agent_id: &str, name: &str) -> bool {
    let Some(suffix) = agent_id.strip_prefix(name) else {
        return false;
    };
    // Generated IDs use either `{name}-{counter}` or `{name}-{millis}-{counter}`.
    // Accept any suffix made up of one or more `-` separated digit groups.
    let Some(suffix) = suffix.strip_prefix('-').filter(|value| !value.is_empty()) else {
        return false;
    };
    suffix
        .split('-')
        .all(|segment| !segment.is_empty() && segment.chars().all(|char| char.is_ascii_digit()))
}

fn fallback_agent_name(agent_id: &str) -> String {
    agent_id
        .rsplit_once('-')
        .map(|(prefix, _)| prefix.to_string())
        .unwrap_or_else(|| agent_id.to_string())
}
