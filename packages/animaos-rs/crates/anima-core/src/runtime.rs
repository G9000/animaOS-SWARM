use std::collections::BTreeMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::agent::{AgentConfig, AgentState, AgentStatus, TokenUsage};
use crate::components::{Evaluator, Provider};
use crate::events::{EngineEvent, EventType};
use crate::model::{ModelAdapter, ModelGenerateRequest, ModelStopReason, ToolCall};
use crate::primitives::{Content, DataValue, Message, MessageRole, TaskResult, TaskStatus};

static NEXT_AGENT_ID: AtomicU64 = AtomicU64::new(0);
static NEXT_MESSAGE_ID: AtomicU64 = AtomicU64::new(0);
static NEXT_ROOM_ID: AtomicU64 = AtomicU64::new(0);
const MAX_TOOL_ITERATIONS: usize = 8;

#[derive(Clone, Debug, PartialEq)]
pub struct AgentRuntimeSnapshot {
    pub state: AgentState,
    pub message_count: usize,
    pub event_count: usize,
    pub last_task: Option<TaskResult<Content>>,
}

pub struct AgentRuntime {
    state: AgentState,
    messages: Vec<Message>,
    last_task: Option<TaskResult<Content>>,
    events: Vec<EngineEvent>,
    providers: Vec<Arc<dyn Provider>>,
    evaluators: Vec<Arc<dyn Evaluator>>,
    model_adapter: Arc<dyn ModelAdapter>,
}

impl AgentRuntime {
    pub fn new(config: AgentConfig, model_adapter: Arc<dyn ModelAdapter>) -> Self {
        let agent_id = next_id("agent", &NEXT_AGENT_ID);
        let name = config.name.clone();

        Self {
            state: AgentState {
                id: agent_id,
                name,
                status: AgentStatus::Idle,
                config,
                created_at: now_millis(),
                token_usage: TokenUsage::default(),
            },
            messages: Vec::new(),
            last_task: None,
            events: Vec::new(),
            providers: Vec::new(),
            evaluators: Vec::new(),
            model_adapter,
        }
    }

    pub fn init(&mut self) {
        self.record_event(
            EventType::AgentSpawned,
            DataValue::String(self.state.name.clone()),
        );
    }

    pub fn id(&self) -> &str {
        &self.state.id
    }

    pub fn config(&self) -> &AgentConfig {
        &self.state.config
    }

    pub fn state(&self) -> AgentState {
        self.state.clone()
    }

    pub fn snapshot(&self) -> AgentRuntimeSnapshot {
        AgentRuntimeSnapshot {
            state: self.state(),
            message_count: self.messages.len(),
            event_count: self.events.len(),
            last_task: self.last_task.clone(),
        }
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn events(&self) -> &[EngineEvent] {
        &self.events
    }

    pub fn register_provider(&mut self, provider: Arc<dyn Provider>) {
        self.providers.push(provider);
    }

    pub fn set_providers(&mut self, providers: Vec<Arc<dyn Provider>>) {
        self.providers = providers;
    }

    pub fn register_evaluator(&mut self, evaluator: Arc<dyn Evaluator>) {
        self.evaluators.push(evaluator);
    }

    pub fn set_evaluators(&mut self, evaluators: Vec<Arc<dyn Evaluator>>) {
        self.evaluators = evaluators;
    }

    pub fn last_task(&self) -> Option<&TaskResult<Content>> {
        self.last_task.as_ref()
    }

    pub fn record_message(&mut self, role: MessageRole, content: Content) -> Message {
        self.record_message_in_room(next_id("room", &NEXT_ROOM_ID), role, content)
    }

    fn record_message_in_room(
        &mut self,
        room_id: String,
        role: MessageRole,
        content: Content,
    ) -> Message {
        let message = Message {
            id: next_id("msg", &NEXT_MESSAGE_ID),
            agent_id: self.state.id.clone(),
            room_id,
            content,
            role,
            created_at: now_millis(),
        };

        self.messages.push(message.clone());
        self.record_event(
            EventType::AgentMessage,
            DataValue::String(message.content.text.clone()),
        );
        message
    }

    pub async fn run(&mut self, input: Content) -> TaskResult<Content> {
        self.run_with_tools(input, |_, _, tool_call| {
            let tool_name = tool_call.name.clone();
            async move { TaskResult::error(format!("Unknown tool: {tool_name}"), 0) }
        })
        .await
    }

    pub async fn run_with_tools<F, Fut>(
        &mut self,
        input: Content,
        mut execute_tool: F,
    ) -> TaskResult<Content>
    where
        F: FnMut(AgentState, Message, ToolCall) -> Fut,
        Fut: Future<Output = TaskResult<Content>>,
    {
        let start = now_millis();
        let room_id = next_id("room", &NEXT_ROOM_ID);
        self.mark_running();
        let user_message = self.record_message_in_room(room_id.clone(), MessageRole::User, input);
        let context_parts = match self.build_provider_context(&user_message).await {
            Ok(context_parts) => context_parts,
            Err(error) => {
                let duration_ms = now_millis().saturating_sub(start);
                self.mark_failed(error, duration_ms);
                return self
                    .last_task
                    .clone()
                    .unwrap_or_else(|| TaskResult::error("provider context failed", duration_ms));
            }
        };
        let mut conversation = vec![user_message.clone()];
        let mut iterations = 0;

        loop {
            let request = ModelGenerateRequest {
                system: self.build_system_prompt(&context_parts),
                messages: conversation.clone(),
                temperature: self
                    .state
                    .config
                    .settings
                    .as_ref()
                    .and_then(|settings| settings.temperature),
                max_tokens: self
                    .state
                    .config
                    .settings
                    .as_ref()
                    .and_then(|settings| settings.max_tokens),
            };

            match self
                .model_adapter
                .generate(&self.state.config, &request)
                .await
            {
                Ok(response) => {
                    self.apply_token_usage(&response.usage);

                    match response.stop_reason {
                        ModelStopReason::End | ModelStopReason::MaxTokens => {
                            if let Err(error) =
                                self.run_evaluators(&user_message, &response.content).await
                            {
                                let duration_ms = now_millis().saturating_sub(start);
                                self.mark_failed(error, duration_ms);
                                return self.last_task.clone().unwrap_or_else(|| {
                                    TaskResult::error("evaluator execution failed", duration_ms)
                                });
                            }
                            let duration_ms = now_millis().saturating_sub(start);
                            self.mark_completed_in_room(
                                room_id.clone(),
                                response.content.clone(),
                                duration_ms,
                            );
                            self.record_token_event();
                            return self.last_task.clone().unwrap_or_else(|| {
                                TaskResult::success(response.content, duration_ms)
                            });
                        }
                        ModelStopReason::ToolCall => {
                            if iterations >= MAX_TOOL_ITERATIONS {
                                let duration_ms = now_millis().saturating_sub(start);
                                self.mark_failed("tool iteration limit exceeded", duration_ms);
                                return self.last_task.clone().unwrap_or_else(|| {
                                    TaskResult::error("tool iteration limit exceeded", duration_ms)
                                });
                            }

                            let Some(tool_calls) = response
                                .tool_calls
                                .clone()
                                .filter(|calls| !calls.is_empty())
                            else {
                                let duration_ms = now_millis().saturating_sub(start);
                                self.mark_failed(
                                    "model requested tools without tool calls",
                                    duration_ms,
                                );
                                return self.last_task.clone().unwrap_or_else(|| {
                                    TaskResult::error(
                                        "model requested tools without tool calls",
                                        duration_ms,
                                    )
                                });
                            };

                            iterations += 1;
                            let assistant_content =
                                content_with_tool_calls(response.content, &tool_calls);
                            let assistant_message = self.record_message_in_room(
                                room_id.clone(),
                                MessageRole::Assistant,
                                assistant_content,
                            );
                            conversation.push(assistant_message);

                            for tool_call in tool_calls {
                                self.record_event(
                                    EventType::ToolBefore,
                                    tool_event_data(&tool_call.name, "running", 0),
                                );
                                let tool_started = now_millis();
                                let tool_result = execute_tool(
                                    self.state.clone(),
                                    user_message.clone(),
                                    tool_call.clone(),
                                )
                                .await;
                                let tool_duration = now_millis().saturating_sub(tool_started);
                                self.record_event(
                                    EventType::ToolAfter,
                                    tool_event_data(
                                        &tool_call.name,
                                        tool_result.status.as_str(),
                                        tool_duration,
                                    ),
                                );
                                let tool_message = self.record_message_in_room(
                                    room_id.clone(),
                                    MessageRole::Tool,
                                    content_from_tool_result(&tool_call, tool_result),
                                );
                                conversation.push(tool_message);
                            }

                            self.record_token_event();
                        }
                    }
                }
                Err(error) => {
                    let duration_ms = now_millis().saturating_sub(start);
                    self.mark_failed(error, duration_ms);
                    return self.last_task.clone().unwrap_or_else(|| {
                        TaskResult::error("model generation failed", duration_ms)
                    });
                }
            }
        }
    }

    pub fn mark_running(&mut self) {
        self.state.status = AgentStatus::Running;
        self.record_event(EventType::AgentStarted, DataValue::Null);
        self.record_event(EventType::TaskStarted, DataValue::Null);
    }

    pub fn mark_completed(&mut self, content: Content, duration_ms: u128) {
        self.mark_completed_in_room(next_id("room", &NEXT_ROOM_ID), content, duration_ms);
    }

    fn mark_completed_in_room(&mut self, room_id: String, content: Content, duration_ms: u128) {
        self.state.status = AgentStatus::Completed;
        self.record_message_in_room(room_id, MessageRole::Assistant, content.clone());
        self.last_task = Some(TaskResult::success(content.clone(), duration_ms));
        self.record_event(EventType::AgentCompleted, DataValue::String(content.text));
        self.record_event(EventType::TaskCompleted, DataValue::Null);
    }

    pub fn mark_failed(&mut self, error: impl Into<String>, duration_ms: u128) {
        let error = error.into();
        self.state.status = AgentStatus::Failed;
        self.last_task = Some(TaskResult::error(error.clone(), duration_ms));
        self.record_event(EventType::AgentFailed, DataValue::String(error));
        self.record_event(EventType::TaskFailed, DataValue::Null);
    }

    pub fn stop(&mut self) {
        self.state.status = AgentStatus::Terminated;
        self.record_event(EventType::AgentTerminated, DataValue::Null);
    }

    fn record_event(&mut self, event_type: EventType, data: DataValue) {
        self.events.push(EngineEvent {
            event_type,
            agent_id: Some(self.state.id.clone()),
            timestamp: now_millis(),
            data,
        });
    }

    fn build_system_prompt(&self, context_parts: &[String]) -> String {
        let mut parts = Vec::new();

        if let Some(bio) = &self.state.config.bio {
            parts.push(format!("## Who You Are\n{bio}"));
        }
        if let Some(lore) = &self.state.config.lore {
            parts.push(format!("## Your Backstory\n{lore}"));
        }
        if let Some(adjectives) = self
            .state
            .config
            .adjectives
            .as_ref()
            .filter(|values| !values.is_empty())
        {
            parts.push(format!(
                "## Your Personality\nYou are {}.",
                adjectives.join(", ")
            ));
        }
        if let Some(topics) = self
            .state
            .config
            .topics
            .as_ref()
            .filter(|values| !values.is_empty())
        {
            parts.push(format!(
                "## Your Expertise\nYou specialize in: {}.",
                topics.join(", ")
            ));
        }
        if let Some(knowledge) = self
            .state
            .config
            .knowledge
            .as_ref()
            .filter(|values| !values.is_empty())
        {
            parts.push(format!(
                "## What You Know\n{}",
                knowledge
                    .iter()
                    .map(|entry| format!("- {entry}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }
        if let Some(style) = &self.state.config.style {
            parts.push(format!("## How You Communicate\n{style}"));
        }
        parts.push(
            self.state
                .config
                .system
                .clone()
                .unwrap_or_else(|| "You are a helpful task agent.".to_string()),
        );
        if !context_parts.is_empty() {
            parts.push(format!("## Context\n{}", context_parts.join("\n")));
        }

        parts.join("\n\n")
    }

    async fn build_provider_context(&self, message: &Message) -> Result<Vec<String>, String> {
        let mut context_parts = Vec::new();
        for provider in &self.providers {
            let result = provider.get(self, message).await?;
            context_parts.push(format!("[{}]: {}", provider.name(), result.text));
        }
        Ok(context_parts)
    }

    async fn run_evaluators(&self, message: &Message, response: &Content) -> Result<(), String> {
        for evaluator in &self.evaluators {
            if evaluator.validate(self, message).await? {
                let _ = evaluator.evaluate(self, message, response).await?;
            }
        }
        Ok(())
    }

    fn apply_token_usage(&mut self, usage: &TokenUsage) {
        self.state.token_usage.prompt_tokens += usage.prompt_tokens;
        self.state.token_usage.completion_tokens += usage.completion_tokens;
        self.state.token_usage.total_tokens += usage.total_tokens;
    }

    fn record_token_event(&mut self) {
        let mut usage = BTreeMap::new();
        usage.insert(
            "promptTokens".to_string(),
            DataValue::Number(self.state.token_usage.prompt_tokens as f64),
        );
        usage.insert(
            "completionTokens".to_string(),
            DataValue::Number(self.state.token_usage.completion_tokens as f64),
        );
        usage.insert(
            "totalTokens".to_string(),
            DataValue::Number(self.state.token_usage.total_tokens as f64),
        );
        self.record_event(EventType::AgentTokens, DataValue::Object(usage));
    }
}

fn content_with_tool_calls(mut content: Content, tool_calls: &[ToolCall]) -> Content {
    let mut metadata = content.metadata.take().unwrap_or_default();
    metadata.insert(
        "toolCalls".into(),
        DataValue::Array(
            tool_calls
                .iter()
                .map(|tool_call| {
                    let mut value = BTreeMap::new();
                    value.insert("id".into(), DataValue::String(tool_call.id.clone()));
                    value.insert("name".into(), DataValue::String(tool_call.name.clone()));
                    value.insert("args".into(), DataValue::Object(tool_call.args.clone()));
                    DataValue::Object(value)
                })
                .collect(),
        ),
    );
    content.metadata = Some(metadata);
    content
}

fn content_from_tool_result(tool_call: &ToolCall, result: TaskResult<Content>) -> Content {
    let mut metadata = BTreeMap::new();
    metadata.insert("toolCallId".into(), DataValue::String(tool_call.id.clone()));
    let task_result = task_result_data_value(&result);
    metadata.insert("taskResult".into(), task_result.clone());

    if result.status == TaskStatus::Success {
        if let Some(mut content) = result.data {
            let content_metadata = content.metadata.get_or_insert_with(BTreeMap::new);
            content_metadata.extend(metadata);
            return content;
        }
    }

    Content {
        text: data_value_json(&task_result),
        attachments: None,
        metadata: Some(metadata),
    }
}

fn tool_event_data(name: &str, status: &str, duration_ms: u128) -> DataValue {
    let mut value = BTreeMap::new();
    value.insert("name".into(), DataValue::String(name.to_string()));
    value.insert("status".into(), DataValue::String(status.to_string()));
    value.insert("durationMs".into(), DataValue::Number(duration_ms as f64));
    DataValue::Object(value)
}

fn task_result_data_value(result: &TaskResult<Content>) -> DataValue {
    let mut value = BTreeMap::new();
    value.insert(
        "status".into(),
        DataValue::String(result.status.as_str().to_string()),
    );
    value.insert("data".into(), content_data_value(result.data.as_ref()));
    value.insert(
        "error".into(),
        match &result.error {
            Some(error) => DataValue::String(error.clone()),
            None => DataValue::Null,
        },
    );
    value.insert(
        "durationMs".into(),
        DataValue::Number(result.duration_ms as f64),
    );
    DataValue::Object(value)
}

fn content_data_value(content: Option<&Content>) -> DataValue {
    let Some(content) = content else {
        return DataValue::Null;
    };

    let mut value = BTreeMap::new();
    value.insert("text".into(), DataValue::String(content.text.clone()));
    value.insert(
        "attachments".into(),
        match content.attachments.as_deref() {
            Some(attachments) => DataValue::Array(
                attachments
                    .iter()
                    .map(|attachment| {
                        let mut attachment_value = BTreeMap::new();
                        attachment_value.insert(
                            "type".into(),
                            DataValue::String(match attachment.attachment_type {
                                crate::primitives::AttachmentType::File => "file".into(),
                                crate::primitives::AttachmentType::Image => "image".into(),
                                crate::primitives::AttachmentType::Url => "url".into(),
                            }),
                        );
                        attachment_value
                            .insert("name".into(), DataValue::String(attachment.name.clone()));
                        attachment_value
                            .insert("data".into(), DataValue::String(attachment.data.clone()));
                        DataValue::Object(attachment_value)
                    })
                    .collect(),
            ),
            None => DataValue::Null,
        },
    );
    value.insert(
        "metadata".into(),
        match &content.metadata {
            Some(metadata) => DataValue::Object(metadata.clone()),
            None => DataValue::Null,
        },
    );
    DataValue::Object(value)
}

fn data_value_json(value: &DataValue) -> String {
    match value {
        DataValue::Null => "null".to_string(),
        DataValue::Bool(value) => value.to_string(),
        DataValue::Number(value) => value.to_string(),
        DataValue::String(value) => format!("\"{}\"", escape_json(value)),
        DataValue::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(data_value_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        DataValue::Object(values) => format!(
            "{{{}}}",
            values
                .iter()
                .map(|(key, value)| format!("\"{}\":{}", escape_json(key), data_value_json(value)))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

fn escape_json(value: &str) -> String {
    let mut escaped = String::new();
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\u{08}' => escaped.push_str("\\b"),
            '\u{0C}' => escaped.push_str("\\f"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character if character.is_control() => {
                escaped.push_str(&format!("\\u{:04x}", u32::from(character)))
            }
            character => escaped.push(character),
        }
    }
    escaped
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after unix epoch")
        .as_millis()
}

fn next_id(prefix: &str, counter: &AtomicU64) -> String {
    let next = counter.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{}-{next}", now_millis())
}

#[cfg(test)]
mod tests {
    use super::{data_value_json, AgentRuntime};
    use crate::agent::{AgentConfig, AgentState, AgentStatus, TokenUsage, ToolDescriptor};
    use crate::components::{Evaluator, EvaluatorResult, Provider, ProviderResult};
    use crate::model::{
        ModelAdapter, ModelGenerateRequest, ModelGenerateResponse, ModelStopReason, ToolCall,
    };
    use crate::primitives::{
        Attachment, AttachmentType, Content, DataValue, Message, MessageRole, TaskResult,
        TaskStatus,
    };
    use async_trait::async_trait;
    use futures::executor::block_on;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll};

    struct StaticModelAdapter;
    struct ToolCallingModelAdapter;
    struct ContextAwareModelAdapter;
    struct AsyncBoundaryModelAdapter;

    struct StaticProvider;
    struct AsyncProvider;
    struct RecordingEvaluator {
        calls: Arc<Mutex<Vec<String>>>,
    }
    struct AsyncRecordingEvaluator {
        calls: Arc<Mutex<Vec<String>>>,
    }
    struct PendingOnce<T> {
        value: Option<T>,
        pending: bool,
    }

    #[async_trait]
    impl ModelAdapter for StaticModelAdapter {
        fn provider(&self) -> &str {
            "static"
        }

        async fn generate(
            &self,
            config: &AgentConfig,
            request: &ModelGenerateRequest,
        ) -> Result<ModelGenerateResponse, String> {
            let input = request
                .messages
                .last()
                .map(|message| message.content.text.clone())
                .unwrap_or_default();
            Ok(ModelGenerateResponse {
                content: Content {
                    text: format!("{} handled task: {}", config.name, input),
                    attachments: None,
                    metadata: None,
                },
                tool_calls: None,
                usage: TokenUsage {
                    prompt_tokens: 5,
                    completion_tokens: 7,
                    total_tokens: 12,
                },
                stop_reason: ModelStopReason::End,
            })
        }
    }

    #[async_trait]
    impl ModelAdapter for ToolCallingModelAdapter {
        fn provider(&self) -> &str {
            "tool-calling"
        }

        async fn generate(
            &self,
            config: &AgentConfig,
            request: &ModelGenerateRequest,
        ) -> Result<ModelGenerateResponse, String> {
            let tool_result = trailing_tool_messages(&request.messages)
                .into_iter()
                .map(render_tool_result_for_model)
                .collect::<Vec<_>>();

            if !tool_result.is_empty() {
                return Ok(ModelGenerateResponse {
                    content: Content {
                        text: format!(
                            "{} used tool result: {}",
                            config.name,
                            tool_result.join("\n")
                        ),
                        attachments: None,
                        metadata: None,
                    },
                    usage: TokenUsage {
                        prompt_tokens: 2,
                        completion_tokens: 3,
                        total_tokens: 5,
                    },
                    stop_reason: ModelStopReason::End,
                    tool_calls: None,
                });
            }

            let mut args = BTreeMap::new();
            args.insert(
                "query".into(),
                crate::primitives::DataValue::String(
                    request
                        .messages
                        .last()
                        .map(|message| message.content.text.clone())
                        .unwrap_or_default(),
                ),
            );

            Ok(ModelGenerateResponse {
                content: Content {
                    text: "searching memories".into(),
                    attachments: None,
                    metadata: None,
                },
                usage: TokenUsage {
                    prompt_tokens: 1,
                    completion_tokens: 1,
                    total_tokens: 2,
                },
                stop_reason: ModelStopReason::ToolCall,
                tool_calls: Some(vec![ToolCall {
                    id: "tool-1".into(),
                    name: "memory_search".into(),
                    args,
                }]),
            })
        }
    }

    #[async_trait]
    impl ModelAdapter for ContextAwareModelAdapter {
        fn provider(&self) -> &str {
            "context-aware"
        }

        async fn generate(
            &self,
            _config: &AgentConfig,
            request: &ModelGenerateRequest,
        ) -> Result<ModelGenerateResponse, String> {
            let output = if request.system.contains("[clock]: noon UTC") {
                "provider context applied"
            } else {
                "provider context missing"
            };

            Ok(ModelGenerateResponse {
                content: Content {
                    text: output.into(),
                    ..Content::default()
                },
                tool_calls: None,
                usage: TokenUsage {
                    prompt_tokens: 3,
                    completion_tokens: 2,
                    total_tokens: 5,
                },
                stop_reason: ModelStopReason::End,
            })
        }
    }

    #[async_trait]
    impl ModelAdapter for AsyncBoundaryModelAdapter {
        fn provider(&self) -> &str {
            "async-boundary"
        }

        async fn generate(
            &self,
            _config: &AgentConfig,
            request: &ModelGenerateRequest,
        ) -> Result<ModelGenerateResponse, String> {
            let output = if request.system.contains("[clock]: noon UTC") {
                "async provider context applied"
            } else {
                "async provider context missing"
            };

            Ok(ModelGenerateResponse {
                content: Content {
                    text: output.into(),
                    ..Content::default()
                },
                tool_calls: None,
                usage: TokenUsage {
                    prompt_tokens: 11,
                    completion_tokens: 13,
                    total_tokens: 24,
                },
                stop_reason: ModelStopReason::End,
            })
        }
    }

    #[async_trait]
    impl Provider for StaticProvider {
        fn name(&self) -> &str {
            "clock"
        }

        fn description(&self) -> &str {
            "Provides the current clock context"
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
    impl Provider for AsyncProvider {
        fn name(&self) -> &str {
            "clock"
        }

        fn description(&self) -> &str {
            "Provides async clock context"
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
    impl Evaluator for RecordingEvaluator {
        fn name(&self) -> &str {
            "recorder"
        }

        fn description(&self) -> &str {
            "Records evaluated responses"
        }

        async fn validate(
            &self,
            _runtime: &AgentRuntime,
            _message: &Message,
        ) -> Result<bool, String> {
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
                .expect("recording evaluator mutex should not be poisoned")
                .push(response.text.clone());
            Ok(EvaluatorResult::default())
        }
    }

    #[async_trait]
    impl Evaluator for AsyncRecordingEvaluator {
        fn name(&self) -> &str {
            "async-recorder"
        }

        fn description(&self) -> &str {
            "Records evaluated responses through async hooks"
        }

        async fn validate(
            &self,
            _runtime: &AgentRuntime,
            _message: &Message,
        ) -> Result<bool, String> {
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
                .expect("recording evaluator mutex should not be poisoned")
                .push(response.text.clone());
            Ok(EvaluatorResult::default())
        }
    }

    impl<T: Unpin> std::future::Future for PendingOnce<T> {
        type Output = T;

        fn poll(
            mut self: std::pin::Pin<&mut Self>,
            context: &mut Context<'_>,
        ) -> Poll<Self::Output> {
            if self.pending {
                self.pending = false;
                context.waker().wake_by_ref();
                Poll::Pending
            } else {
                Poll::Ready(self.value.take().expect("pending-once value should exist"))
            }
        }
    }

    impl<T> PendingOnce<T> {
        fn new(value: T) -> Self {
            Self {
                value: Some(value),
                pending: true,
            }
        }
    }

    fn trailing_tool_messages(messages: &[Message]) -> Vec<&Message> {
        let mut trailing = messages
            .iter()
            .rev()
            .take_while(|message| matches!(message.role, MessageRole::Tool))
            .collect::<Vec<_>>();
        trailing.reverse();
        trailing
    }

    fn render_tool_result_for_model(message: &Message) -> String {
        let Some(task_result) = message
            .content
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("taskResult"))
        else {
            return message.content.text.clone();
        };

        match task_result {
            DataValue::Object(task_result) => {
                let status = task_result.get("status");
                let data_text = task_result.get("data").and_then(task_result_content_text);

                if matches!(status, Some(DataValue::String(value)) if value == "success") {
                    if let Some(text) = data_text {
                        return text.to_string();
                    }
                }

                data_value_json(&DataValue::Object(task_result.clone()))
            }
            _ => message.content.text.clone(),
        }
    }

    fn task_result_content_text(value: &DataValue) -> Option<&str> {
        match value {
            DataValue::Object(content) => match content.get("text") {
                Some(DataValue::String(text)) => Some(text.as_str()),
                _ => None,
            },
            _ => None,
        }
    }

    fn config() -> AgentConfig {
        AgentConfig {
            name: "researcher".into(),
            model: "gpt-5.4".into(),
            bio: Some("Finds answers".into()),
            lore: None,
            knowledge: None,
            topics: None,
            adjectives: None,
            style: None,
            provider: Some("openai".into()),
            system: None,
            tools: None,
            plugins: None,
            settings: None,
        }
    }

    fn tool_config() -> AgentConfig {
        AgentConfig {
            tools: Some(vec![ToolDescriptor {
                name: "memory_search".into(),
                description: "Search memories".into(),
                parameters: BTreeMap::new(),
                examples: None,
            }]),
            ..config()
        }
    }

    fn runtime() -> AgentRuntime {
        AgentRuntime::new(config(), Arc::new(StaticModelAdapter))
    }

    #[test]
    fn runtime_tracks_lifecycle_state() {
        let mut runtime = runtime();
        runtime.init();

        assert_eq!(runtime.state().status, AgentStatus::Idle);
        assert_eq!(runtime.snapshot().event_count, 1);

        runtime.mark_running();
        assert_eq!(runtime.state().status, AgentStatus::Running);

        runtime.mark_completed(
            Content {
                text: "done".into(),
                ..Content::default()
            },
            42,
        );

        let snapshot = runtime.snapshot();
        assert_eq!(snapshot.state.status, AgentStatus::Completed);
        assert_eq!(
            snapshot
                .last_task
                .as_ref()
                .expect("task should exist")
                .status,
            TaskStatus::Success
        );
    }

    #[test]
    fn runtime_records_messages_in_context() {
        let mut runtime = runtime();
        runtime.init();
        runtime.record_message(
            MessageRole::User,
            Content {
                text: "hello".into(),
                ..Content::default()
            },
        );

        let snapshot = runtime.snapshot();
        assert_eq!(snapshot.message_count, 1);
        assert_eq!(runtime.messages()[0].content.text, "hello");
    }

    #[test]
    fn runtime_stop_marks_terminated() {
        let mut runtime = runtime();
        runtime.init();
        runtime.stop();

        assert_eq!(runtime.state().status, AgentStatus::Terminated);
        assert_eq!(
            runtime
                .events()
                .last()
                .expect("event should exist")
                .event_type
                .as_str(),
            "agent:terminated"
        );
    }

    #[test]
    fn runtime_run_records_result_and_context() {
        let mut runtime = runtime();
        runtime.init();

        let result = block_on(runtime.run(Content {
            text: "Inspect memory state".into(),
            ..Content::default()
        }));
        let snapshot = runtime.snapshot();

        assert_eq!(result.status, TaskStatus::Success);
        assert_eq!(
            result.data.as_ref().map(|content| content.text.as_str()),
            Some("researcher handled task: Inspect memory state")
        );
        assert_eq!(snapshot.state.status, AgentStatus::Completed);
        assert_eq!(snapshot.message_count, 2);
        assert_eq!(snapshot.event_count, 8);
        assert_eq!(snapshot.state.token_usage.prompt_tokens, 5);
        assert_eq!(snapshot.state.token_usage.completion_tokens, 7);
        assert_eq!(snapshot.state.token_usage.total_tokens, 12);
        assert!(snapshot.state.token_usage.total_tokens > 0);
        assert_eq!(
            runtime
                .events()
                .last()
                .expect("token event should exist")
                .event_type
                .as_str(),
            "agent:tokens"
        );
    }

    #[test]
    fn runtime_run_reuses_one_room_for_conversation_messages() {
        let mut runtime = runtime();
        runtime.init();

        block_on(runtime.run(Content {
            text: "Keep one room".into(),
            ..Content::default()
        }));

        assert_eq!(runtime.messages().len(), 2);
        assert_eq!(runtime.messages()[0].room_id, runtime.messages()[1].room_id);
    }

    #[test]
    fn runtime_run_executes_tool_round_trip() {
        let mut runtime = AgentRuntime::new(tool_config(), Arc::new(ToolCallingModelAdapter));
        runtime.init();

        let result = block_on(runtime.run_with_tools(
            Content {
                text: "search memory".into(),
                ..Content::default()
            },
            |_, _, tool_call| {
                assert_eq!(tool_call.name, "memory_search");
                async move {
                    TaskResult::success(
                        Content {
                            text: "memory hit".into(),
                            ..Content::default()
                        },
                        1,
                    )
                }
            },
        ));

        let snapshot = runtime.snapshot();
        assert_eq!(result.status, TaskStatus::Success);
        assert_eq!(
            result.data.as_ref().map(|content| content.text.as_str()),
            Some("researcher used tool result: memory hit")
        );
        assert_eq!(snapshot.message_count, 4);
        assert_eq!(runtime.messages()[0].room_id, runtime.messages()[3].room_id);
        assert!(snapshot.state.token_usage.total_tokens >= 7);
    }

    #[test]
    fn runtime_run_does_not_reuse_previous_tool_result_between_runs() {
        let mut runtime = AgentRuntime::new(tool_config(), Arc::new(ToolCallingModelAdapter));
        runtime.init();

        let first = block_on(runtime.run_with_tools(
            Content {
                text: "alpha".into(),
                ..Content::default()
            },
            |_, user_message, _| {
                let output = format!("{} hit", user_message.content.text);
                async move {
                    TaskResult::success(
                        Content {
                            text: output,
                            ..Content::default()
                        },
                        1,
                    )
                }
            },
        ));
        let second = block_on(runtime.run_with_tools(
            Content {
                text: "beta".into(),
                ..Content::default()
            },
            |_, user_message, _| {
                let output = format!("{} hit", user_message.content.text);
                async move {
                    TaskResult::success(
                        Content {
                            text: output,
                            ..Content::default()
                        },
                        1,
                    )
                }
            },
        ));

        assert_eq!(
            first.data.as_ref().map(|content| content.text.as_str()),
            Some("researcher used tool result: alpha hit")
        );
        assert_eq!(
            second.data.as_ref().map(|content| content.text.as_str()),
            Some("researcher used tool result: beta hit")
        );
    }

    #[test]
    fn runtime_run_preserves_structured_tool_errors() {
        let mut runtime = AgentRuntime::new(tool_config(), Arc::new(ToolCallingModelAdapter));
        runtime.init();

        let result = block_on(runtime.run_with_tools(
            Content {
                text: "search failure".into(),
                ..Content::default()
            },
            |_, _, _| async move { TaskResult::error("boom", 3) },
        ));

        let output = result
            .data
            .as_ref()
            .map(|content| content.text.as_str())
            .expect("result should contain assistant output");
        assert!(
            output.contains("\"status\":\"error\""),
            "missing status in {output}"
        );
        assert!(
            output.contains("\"error\":\"boom\""),
            "missing error in {output}"
        );
        assert!(
            output.contains("\"durationMs\":3"),
            "missing duration in {output}"
        );
    }

    #[test]
    fn runtime_run_preserves_successful_tool_content_shape() {
        let mut runtime = AgentRuntime::new(tool_config(), Arc::new(ToolCallingModelAdapter));
        runtime.init();

        let result = block_on(runtime.run_with_tools(
            Content {
                text: "shape check".into(),
                ..Content::default()
            },
            |_, _, _| async move {
                let mut metadata = BTreeMap::new();
                metadata.insert("source".into(), DataValue::String("tool".into()));
                TaskResult::success(
                    Content {
                        text: "rich tool result".into(),
                        attachments: Some(vec![Attachment {
                            attachment_type: AttachmentType::File,
                            name: "notes.txt".into(),
                            data: "payload".into(),
                        }]),
                        metadata: Some(metadata),
                    },
                    2,
                )
            },
        ));

        let tool_message = runtime
            .messages()
            .iter()
            .find(|message| matches!(message.role, MessageRole::Tool))
            .expect("tool message should exist");

        assert_eq!(tool_message.content.text, "rich tool result");
        assert_eq!(
            tool_message
                .content
                .attachments
                .as_ref()
                .map(|attachments| attachments.len()),
            Some(1)
        );
        assert!(
            tool_message.content.text == "rich tool result",
            "tool message text should stay as the original content"
        );
        assert!(
            tool_message
                .content
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("source"))
                .is_some(),
            "tool message should preserve original metadata"
        );
        assert_eq!(
            result.data.as_ref().map(|content| content.text.as_str()),
            Some("researcher used tool result: rich tool result")
        );
    }

    #[test]
    fn runtime_run_includes_provider_context_in_system_prompt() {
        let mut runtime = AgentRuntime::new(config(), Arc::new(ContextAwareModelAdapter));
        runtime.register_provider(Arc::new(StaticProvider));
        runtime.init();

        let result = block_on(runtime.run(Content {
            text: "what time is it".into(),
            ..Content::default()
        }));

        assert_eq!(result.status, TaskStatus::Success);
        assert_eq!(
            result.data.as_ref().map(|content| content.text.as_str()),
            Some("provider context applied")
        );
    }

    #[test]
    fn runtime_run_executes_evaluators_after_final_response() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut runtime = runtime();
        runtime.register_evaluator(Arc::new(RecordingEvaluator {
            calls: Arc::clone(&calls),
        }));
        runtime.init();

        let result = block_on(runtime.run(Content {
            text: "evaluate this".into(),
            ..Content::default()
        }));

        assert_eq!(result.status, TaskStatus::Success);
        let recorded = calls
            .lock()
            .expect("recording evaluator mutex should not be poisoned");
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0], "researcher handled task: evaluate this");
    }

    #[test]
    fn runtime_run_awaits_async_boundaries() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut runtime = AgentRuntime::new(config(), Arc::new(AsyncBoundaryModelAdapter));
        runtime.register_provider(Arc::new(AsyncProvider));
        runtime.register_evaluator(Arc::new(AsyncRecordingEvaluator {
            calls: Arc::clone(&calls),
        }));
        runtime.init();

        let result = block_on(runtime.run(Content {
            text: "check async boundaries".into(),
            ..Content::default()
        }));

        assert_eq!(result.status, TaskStatus::Success);
        assert_eq!(
            result.data.as_ref().map(|content| content.text.as_str()),
            Some("async provider context applied")
        );
        assert_eq!(runtime.snapshot().state.token_usage.total_tokens, 24);
        let recorded = calls
            .lock()
            .expect("recording evaluator mutex should not be poisoned");
        assert_eq!(recorded.as_slice(), ["async provider context applied"]);
    }

    #[test]
    fn runtime_run_with_tools_awaits_async_tool_execution() {
        let mut runtime = AgentRuntime::new(tool_config(), Arc::new(ToolCallingModelAdapter));
        runtime.init();

        let result = block_on(runtime.run_with_tools(
            Content {
                text: "search memory".into(),
                ..Content::default()
            },
            |_, _, tool_call| {
                let tool_name = tool_call.name.clone();
                async move {
                    assert_eq!(tool_name, "memory_search");
                    TaskResult::success(
                        Content {
                            text: "async memory hit".into(),
                            ..Content::default()
                        },
                        1,
                    )
                }
            },
        ));

        assert_eq!(result.status, TaskStatus::Success);
        assert_eq!(
            result.data.as_ref().map(|content| content.text.as_str()),
            Some("researcher used tool result: async memory hit")
        );
    }

    #[test]
    fn runtime_run_with_tools_supports_owned_inputs_across_pending_await() {
        let mut runtime = AgentRuntime::new(tool_config(), Arc::new(ToolCallingModelAdapter));
        runtime.init();

        let result = block_on(runtime.run_with_tools(
            Content {
                text: "search memory".into(),
                ..Content::default()
            },
            |state: AgentState, message: Message, tool_call: ToolCall| async move {
                let rendered = PendingOnce::new(format!(
                    "{}:{}:{}",
                    state.name, message.content.text, tool_call.name
                ))
                .await;

                TaskResult::success(
                    Content {
                        text: rendered,
                        ..Content::default()
                    },
                    1,
                )
            },
        ));

        assert_eq!(result.status, TaskStatus::Success);
        assert_eq!(
            result.data.as_ref().map(|content| content.text.as_str()),
            Some("researcher used tool result: researcher:search memory:memory_search")
        );
    }
}
