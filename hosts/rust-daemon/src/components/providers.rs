use std::collections::BTreeMap;

use anima_core::{AgentRuntime, DataValue, Message, Provider, ProviderResult};
use anima_memory::RecentMemoryOptions;
use async_trait::async_trait;

use crate::state::SharedMemoryStore;

pub(super) struct RecentMemoriesProvider {
    pub(super) memory: SharedMemoryStore,
}

#[async_trait]
impl Provider for RecentMemoriesProvider {
    fn name(&self) -> &str {
        "recent_memories"
    }

    fn description(&self) -> &str {
        "Provides the agent's recent memories as run context"
    }

    async fn get(
        &self,
        runtime: &AgentRuntime,
        _message: &Message,
    ) -> Result<ProviderResult, String> {
        let memories = self.memory.read().await.get_recent(RecentMemoryOptions {
            agent_id: Some(runtime.id().to_string()),
            agent_name: None,
            scope: None,
            room_id: None,
            world_id: None,
            session_id: None,
            limit: Some(3),
        });

        let text = if memories.is_empty() {
            "no recent memories".to_string()
        } else {
            memories
                .into_iter()
                .map(|memory| memory.content)
                .collect::<Vec<_>>()
                .join(" | ")
        };

        let mut metadata = BTreeMap::new();
        metadata.insert("kind".into(), DataValue::String("recent_memories".into()));

        Ok(ProviderResult {
            text,
            metadata: Some(metadata),
        })
    }
}
