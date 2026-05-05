#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use anima_core::{
    AgentConfig, AgentRuntime, Content, DataValue, DatabaseAdapter, Evaluator, EvaluatorResult,
    Message, ModelAdapter, ModelGenerateRequest, ModelGenerateResponse, ModelStopReason,
    PersistenceError, PersistenceResult, Provider, ProviderResult, Step, StepStatus, TokenUsage,
    ToolCall, ToolDescriptor,
};
use async_trait::async_trait;

pub struct ScriptedModelAdapter;
pub struct UnknownToolModelAdapter;
pub struct FinalAnswerModelAdapter;
pub struct FailingModelAdapter;

#[async_trait]
impl ModelAdapter for ScriptedModelAdapter {
    fn provider(&self) -> &str {
        "scripted"
    }

    async fn generate(
        &self,
        config: &AgentConfig,
        request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String> {
        if request
            .messages
            .iter()
            .any(|message| message.role == anima_core::MessageRole::Tool)
        {
            let tool_message = request
                .messages
                .iter()
                .rev()
                .find(|message| message.role == anima_core::MessageRole::Tool)
                .expect("tool message should exist once a tool has run");

            return Ok(ModelGenerateResponse {
                content: Content {
                    text: format!(
                        "{} finalized with {}",
                        config.name, tool_message.content.text
                    ),
                    ..Content::default()
                },
                tool_calls: None,
                usage: TokenUsage {
                    prompt_tokens: 5,
                    completion_tokens: 7,
                    total_tokens: 12,
                },
                stop_reason: ModelStopReason::End,
            });
        }

        assert!(
            request.system.contains("[clock]: noon UTC"),
            "provider context should be present in the system prompt"
        );

        Ok(ModelGenerateResponse {
            content: Content {
                text: "delegate to memory tool".into(),
                ..Content::default()
            },
            tool_calls: Some(vec![ToolCall {
                id: "tool-1".into(),
                name: "memory_search".into(),
                args: BTreeMap::from([(
                    "query".into(),
                    DataValue::String("release readiness".into()),
                )]),
            }]),
            usage: TokenUsage {
                prompt_tokens: 3,
                completion_tokens: 2,
                total_tokens: 5,
            },
            stop_reason: ModelStopReason::ToolCall,
        })
    }
}

#[async_trait]
impl ModelAdapter for UnknownToolModelAdapter {
    fn provider(&self) -> &str {
        "unknown-tool"
    }

    async fn generate(
        &self,
        config: &AgentConfig,
        request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String> {
        if request
            .messages
            .iter()
            .any(|message| message.role == anima_core::MessageRole::Tool)
        {
            let tool_message = request
                .messages
                .iter()
                .rev()
                .find(|message| message.role == anima_core::MessageRole::Tool)
                .expect("tool message should exist once a tool has run");

            return Ok(ModelGenerateResponse {
                content: Content {
                    text: format!("{} saw {}", config.name, tool_message.content.text),
                    ..Content::default()
                },
                tool_calls: None,
                usage: TokenUsage {
                    prompt_tokens: 4,
                    completion_tokens: 6,
                    total_tokens: 10,
                },
                stop_reason: ModelStopReason::End,
            });
        }

        Ok(ModelGenerateResponse {
            content: Content {
                text: "call an unknown tool".into(),
                ..Content::default()
            },
            tool_calls: Some(vec![ToolCall {
                id: "missing-1".into(),
                name: "missing_tool".into(),
                args: BTreeMap::new(),
            }]),
            usage: TokenUsage {
                prompt_tokens: 2,
                completion_tokens: 1,
                total_tokens: 3,
            },
            stop_reason: ModelStopReason::ToolCall,
        })
    }
}

#[async_trait]
impl ModelAdapter for FinalAnswerModelAdapter {
    fn provider(&self) -> &str {
        "final-answer"
    }

    async fn generate(
        &self,
        _config: &AgentConfig,
        _request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String> {
        Ok(ModelGenerateResponse {
            content: Content {
                text: "ready to ship".into(),
                ..Content::default()
            },
            tool_calls: None,
            usage: TokenUsage {
                prompt_tokens: 2,
                completion_tokens: 2,
                total_tokens: 4,
            },
            stop_reason: ModelStopReason::End,
        })
    }
}

#[async_trait]
impl ModelAdapter for FailingModelAdapter {
    fn provider(&self) -> &str {
        "failing-model"
    }

    async fn generate(
        &self,
        _config: &AgentConfig,
        _request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String> {
        Err("model backend offline".into())
    }
}

pub struct ClockProvider;
pub struct FailingProvider;

#[async_trait]
impl Provider for ClockProvider {
    fn name(&self) -> &str {
        "clock"
    }

    fn description(&self) -> &str {
        "Provides deterministic clock context"
    }

    async fn get(
        &self,
        _runtime: &AgentRuntime,
        _message: &Message,
    ) -> Result<ProviderResult, String> {
        Ok(ProviderResult {
            text: "noon UTC".into(),
            metadata: None,
        })
    }
}

#[async_trait]
impl Provider for FailingProvider {
    fn name(&self) -> &str {
        "failing-clock"
    }

    fn description(&self) -> &str {
        "Fails while building provider context"
    }

    async fn get(
        &self,
        _runtime: &AgentRuntime,
        _message: &Message,
    ) -> Result<ProviderResult, String> {
        Err("provider context unavailable".into())
    }
}

pub struct RecordingEvaluator {
    pub calls: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl Evaluator for RecordingEvaluator {
    fn name(&self) -> &str {
        "recorder"
    }

    fn description(&self) -> &str {
        "Records finalized responses"
    }

    async fn validate(&self, _runtime: &AgentRuntime, _message: &Message) -> Result<bool, String> {
        Ok(true)
    }

    async fn evaluate(
        &self,
        _runtime: &AgentRuntime,
        _message: &Message,
        response: &Content,
    ) -> Result<EvaluatorResult, String> {
        self.calls
            .lock()
            .expect("evaluator call recorder should not be poisoned")
            .push(response.text.clone());
        Ok(EvaluatorResult::default())
    }
}

pub struct FailingEvaluator;

#[async_trait]
impl Evaluator for FailingEvaluator {
    fn name(&self) -> &str {
        "failing-recorder"
    }

    fn description(&self) -> &str {
        "Fails after the model returns a final answer"
    }

    async fn validate(&self, _runtime: &AgentRuntime, _message: &Message) -> Result<bool, String> {
        Ok(true)
    }

    async fn evaluate(
        &self,
        _runtime: &AgentRuntime,
        _message: &Message,
        _response: &Content,
    ) -> Result<EvaluatorResult, String> {
        Err("evaluator exploded".into())
    }
}

#[derive(Default)]
pub struct RecordingDatabase {
    steps: Mutex<Vec<Step>>,
}

impl RecordingDatabase {
    pub fn statuses(&self) -> Vec<StepStatus> {
        self.steps
            .lock()
            .expect("step recorder should not be poisoned")
            .iter()
            .map(|step| step.status.clone())
            .collect()
    }
}

#[async_trait]
impl DatabaseAdapter for RecordingDatabase {
    async fn write_step(&self, step: &Step) -> PersistenceResult<()> {
        let mut steps = self
            .steps
            .lock()
            .map_err(|error| PersistenceError::Write(format!("Mutex poisoned: {error}")))?;

        if let Some(existing) = steps.iter_mut().find(|existing| {
            existing.agent_id == step.agent_id && existing.idempotency_key == step.idempotency_key
        }) {
            if !matches!(existing.status, StepStatus::Done | StepStatus::Failed) {
                existing.status = step.status.clone();
                existing.input = step.input.clone();
                existing.output = step.output.clone();
            }
            return Ok(());
        }

        steps.push(step.clone());
        Ok(())
    }

    async fn get_step_by_idempotency_key(
        &self,
        agent_id: &str,
        key: &str,
    ) -> PersistenceResult<Option<Step>> {
        let steps = self
            .steps
            .lock()
            .map_err(|error| PersistenceError::Query(format!("Mutex poisoned: {error}")))?;

        Ok(steps
            .iter()
            .find(|step| step.agent_id == agent_id && step.idempotency_key == key)
            .cloned())
    }

    async fn list_agent_steps(&self, agent_id: &str) -> PersistenceResult<Vec<Step>> {
        let steps = self
            .steps
            .lock()
            .map_err(|error| PersistenceError::Query(format!("Mutex poisoned: {error}")))?;

        Ok(steps
            .iter()
            .filter(|step| step.agent_id == agent_id)
            .cloned()
            .collect())
    }
}

pub fn config() -> AgentConfig {
    AgentConfig {
        name: "release-agent".into(),
        model: "gpt-5.4".into(),
        bio: Some("Checks first-release readiness.".into()),
        lore: None,
        knowledge: None,
        topics: None,
        adjectives: None,
        style: None,
        provider: Some("openai".into()),
        system: Some("Be concise and concrete.".into()),
        tools: Some(vec![ToolDescriptor {
            name: "memory_search".into(),
            description: "Search release notes".into(),
            parameters_schema: BTreeMap::new(),
            examples: None,
        }]),
        plugins: None,
        settings: None,
    }
}

pub fn retry_metadata(key: &str) -> BTreeMap<String, DataValue> {
    BTreeMap::from([(String::from("retryKey"), DataValue::String(key.into()))])
}
