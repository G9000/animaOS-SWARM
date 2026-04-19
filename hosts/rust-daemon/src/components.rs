use std::collections::BTreeMap;
use std::sync::Arc;

use anima_core::{
    AgentRuntime, Content, DataValue, Evaluator, EvaluatorResult, Message, Provider, ProviderResult,
};
use anima_memory::{MemoryType, NewMemory, RecentMemoryOptions};
use async_trait::async_trait;

use crate::state::SharedMemoryStore;

pub(crate) fn default_providers(memory: SharedMemoryStore) -> Vec<Arc<dyn Provider>> {
    vec![Arc::new(RecentMemoriesProvider { memory })]
}

pub(crate) fn default_evaluators(memory: SharedMemoryStore) -> Vec<Arc<dyn Evaluator>> {
    vec![Arc::new(ReflectionMemoryEvaluator { memory })]
}

struct RecentMemoriesProvider {
    memory: SharedMemoryStore,
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
        let memories = self
            .memory
            .read()
            .await
            .get_recent(RecentMemoryOptions {
                agent_id: Some(runtime.id().to_string()),
                agent_name: None,
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

struct ReflectionMemoryEvaluator {
    memory: SharedMemoryStore,
}

#[async_trait]
impl Evaluator for ReflectionMemoryEvaluator {
    fn name(&self) -> &str {
        "reflection_memory"
    }

    fn description(&self) -> &str {
        "Persists a reflection memory for each completed response"
    }

    async fn validate(&self, _runtime: &AgentRuntime, _message: &Message) -> Result<bool, String> {
        Ok(true)
    }

    async fn evaluate(
        &self,
        runtime: &AgentRuntime,
        _message: &Message,
        response: &Content,
    ) -> Result<EvaluatorResult, String> {
        if response.text.trim().is_empty() {
            return Ok(EvaluatorResult::default());
        }

        let state = runtime.state();
        let reflection = format!("evaluated response: {}", response.text);
        let memory = self
            .memory
            .write()
            .await
            .add(NewMemory {
                agent_id: state.id,
                agent_name: state.name,
                memory_type: MemoryType::Reflection,
                content: reflection.clone(),
                importance: 0.6,
                tags: Some(vec!["runtime".into(), "evaluator-reflection".into()]),
            })
            .map_err(|error| error.message().to_string())?;

        let mut metadata = BTreeMap::new();
        metadata.insert("memoryId".into(), DataValue::String(memory.id));

        Ok(EvaluatorResult {
            feedback: Some("stored reflection memory".into()),
            metadata: Some(metadata),
            ..EvaluatorResult::default()
        })
    }
}
