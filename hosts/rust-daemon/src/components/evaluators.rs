use std::collections::BTreeMap;

use anima_core::{AgentRuntime, Content, DataValue, Evaluator, EvaluatorResult, Message};
use anima_memory::{MemoryType, NewMemory};
use async_trait::async_trait;

use crate::state::SharedMemoryStore;

pub(super) struct ReflectionMemoryEvaluator {
    pub(super) memory: SharedMemoryStore,
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
        let memory = {
            let mut memory_guard = self.memory.write().await;
            let memory = memory_guard
                .add(NewMemory {
                    agent_id: state.id,
                    agent_name: state.name,
                    memory_type: MemoryType::Reflection,
                    content: reflection.clone(),
                    importance: 0.6,
                    tags: Some(vec!["runtime".into(), "evaluator-reflection".into()]),
                    scope: None,
                    room_id: None,
                    world_id: None,
                    session_id: None,
                })
                .map_err(|error| error.message().to_string())?;
            memory_guard
                .save()
                .map_err(|error| format!("failed to persist reflection memory: {error}"))?;
            memory
        };

        let mut metadata = BTreeMap::new();
        metadata.insert("memoryId".into(), DataValue::String(memory.id));

        Ok(EvaluatorResult {
            feedback: Some("stored reflection memory".into()),
            metadata: Some(metadata),
            ..EvaluatorResult::default()
        })
    }
}
