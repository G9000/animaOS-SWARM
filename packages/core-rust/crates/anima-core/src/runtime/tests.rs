//! Inline tests for `runtime`. Kept as a `#[path]`-attached submodule
//! so they can reach private items in `runtime.rs`.

use super::AgentRuntime;
use crate::runtime_serde::data_value_json;
use crate::agent::{AgentConfig, AgentState, AgentStatus, TokenUsage, ToolDescriptor};
use crate::components::{Evaluator, EvaluatorResult, Provider, ProviderResult};
use crate::model::{
    ModelAdapter, ModelGenerateRequest, ModelGenerateResponse, ModelStopReason, ToolCall,
};
use crate::persistence::{in_memory::InMemoryAdapter, DatabaseAdapter, StepStatus};
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
struct MultiToolCallingModelAdapter;
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
struct OrderedProvider {
    name: &'static str,
    priority: u8,
    calls: Arc<Mutex<Vec<String>>>,
}
struct OrderedEvaluator {
    name: &'static str,
    priority: u8,
    calls: Arc<Mutex<Vec<String>>>,
}
struct RetryAwareModelAdapter;
struct RetryOnceEvaluator;
struct AbortEvaluator;
struct AlwaysRetryEvaluator;
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
impl ModelAdapter for MultiToolCallingModelAdapter {
    fn provider(&self) -> &str {
        "multi-tool-calling"
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

        if tool_result.len() >= 2 {
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
                tool_calls: None,
                usage: TokenUsage {
                    prompt_tokens: 4,
                    completion_tokens: 3,
                    total_tokens: 7,
                },
                stop_reason: ModelStopReason::End,
            });
        }

        Ok(ModelGenerateResponse {
            content: Content {
                text: "delegate both tasks".into(),
                attachments: None,
                metadata: None,
            },
            tool_calls: Some(vec![
                ToolCall {
                    id: "tool-1".into(),
                    name: "memory_search".into(),
                    args: BTreeMap::from([("query".into(), DataValue::String("alpha".into()))]),
                },
                ToolCall {
                    id: "tool-2".into(),
                    name: "memory_search".into(),
                    args: BTreeMap::from([("query".into(), DataValue::String("beta".into()))]),
                },
            ]),
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
impl Provider for OrderedProvider {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        "Records provider execution order"
    }

    fn priority(&self) -> u8 {
        self.priority
    }

    async fn get(
        &self,
        _runtime: &AgentRuntime,
        _message: &Message,
    ) -> Result<ProviderResult, String> {
        self.calls
            .lock()
            .expect("ordered provider mutex should not be poisoned")
            .push(self.name.to_string());
        Ok(ProviderResult {
            text: self.name.into(),
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

#[async_trait]
impl Evaluator for OrderedEvaluator {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        "Records evaluator execution order"
    }

    fn priority(&self) -> u8 {
        self.priority
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
        _response: &Content,
    ) -> Result<EvaluatorResult, String> {
        self.calls
            .lock()
            .expect("ordered evaluator mutex should not be poisoned")
            .push(self.name.to_string());
        Ok(EvaluatorResult::default())
    }
}

#[async_trait]
impl ModelAdapter for RetryAwareModelAdapter {
    fn provider(&self) -> &str {
        "retry-aware"
    }

    async fn generate(
        &self,
        _config: &AgentConfig,
        request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String> {
        let revised = request.messages.iter().any(|message| {
            message.role == MessageRole::System && message.content.text.contains("be specific")
        });

        Ok(ModelGenerateResponse {
            content: Content {
                text: if revised {
                    "revised answer"
                } else {
                    "draft answer"
                }
                .into(),
                ..Content::default()
            },
            tool_calls: None,
            usage: TokenUsage {
                prompt_tokens: 2,
                completion_tokens: 3,
                total_tokens: 5,
            },
            stop_reason: ModelStopReason::End,
        })
    }
}

#[async_trait]
impl Evaluator for RetryOnceEvaluator {
    fn name(&self) -> &str {
        "retry-once"
    }

    fn description(&self) -> &str {
        "Requests one correction pass for draft answers"
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
        if response.text == "draft answer" {
            Ok(EvaluatorResult::retry("be specific"))
        } else {
            Ok(EvaluatorResult::accept())
        }
    }
}

#[async_trait]
impl Evaluator for AbortEvaluator {
    fn name(&self) -> &str {
        "abort"
    }

    fn description(&self) -> &str {
        "Rejects a response without retrying"
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
        _response: &Content,
    ) -> Result<EvaluatorResult, String> {
        Ok(EvaluatorResult::abort("unsafe response"))
    }
}

#[async_trait]
impl Evaluator for AlwaysRetryEvaluator {
    fn name(&self) -> &str {
        "always-retry"
    }

    fn description(&self) -> &str {
        "Always requests another attempt"
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
        _response: &Content,
    ) -> Result<EvaluatorResult, String> {
        Ok(EvaluatorResult::retry("try again"))
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
            parameters_schema: BTreeMap::new(),
            examples: None,
        }]),
        ..config()
    }
}

fn runtime() -> AgentRuntime {
    AgentRuntime::new(config(), Arc::new(StaticModelAdapter))
}

#[test]
fn runtime_notifies_event_listener_with_live_events() {
    let mut runtime = runtime();
    let events = Arc::new(Mutex::new(Vec::new()));
    runtime.set_event_listener(Arc::new({
        let events = Arc::clone(&events);
        move |event| {
            events
                .lock()
                .expect("listener mutex should not be poisoned")
                .push(event.event_type.as_str().to_string());
        }
    }));

    runtime.init();
    let result = block_on(runtime.run(Content {
        text: "ship it".into(),
        ..Content::default()
    }));

    assert_eq!(result.status, TaskStatus::Success);
    let events = events
        .lock()
        .expect("listener mutex should not be poisoned")
        .clone();
    assert!(events.iter().any(|event| event == "agent:spawned"));
    assert!(events.iter().any(|event| event == "task:started"));
    assert!(events.iter().any(|event| event == "agent:tokens"));
    assert!(events.iter().any(|event| event == "task:completed"));
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
fn runtime_reuses_persisted_tool_result_for_retried_task() {
    let db = Arc::new(InMemoryAdapter::new());
    let db_adapter: Arc<dyn DatabaseAdapter> = db.clone();
    let mut runtime = AgentRuntime::new(tool_config(), Arc::new(ToolCallingModelAdapter));
    runtime.init();
    runtime.set_database(db_adapter);

    let mut metadata = BTreeMap::new();
    metadata.insert(
        "retryKey".into(),
        DataValue::String("retry-search-memory".into()),
    );

    let first = block_on(runtime.run_with_tools(
        Content {
            text: "search memory".into(),
            metadata: Some(metadata.clone()),
            ..Content::default()
        },
        |_, _, _| async move {
            TaskResult::success(
                Content {
                    text: "persisted memory hit".into(),
                    ..Content::default()
                },
                1,
            )
        },
    ));
    let second = block_on(runtime.run_with_tools(
        Content {
            text: "search memory".into(),
            metadata: Some(metadata),
            ..Content::default()
        },
        |_, _, _| async move {
            panic!("retried tool execution should be recovered from persistence")
        },
    ));

    assert_eq!(first.status, TaskStatus::Success);
    assert_eq!(second.status, TaskStatus::Success);
    assert_eq!(
        second.data.as_ref().map(|content| content.text.as_str()),
        Some("researcher used tool result: persisted memory hit")
    );

    let steps = db.recorded_steps();
    assert_eq!(
        steps.len(),
        1,
        "retried logical step should not duplicate rows"
    );
    assert_eq!(steps[0].status, StepStatus::Done);
}

#[test]
fn runtime_does_not_reuse_persisted_tool_result_without_retry_key() {
    let db = Arc::new(InMemoryAdapter::new());
    let db_adapter: Arc<dyn DatabaseAdapter> = db.clone();
    let mut runtime = AgentRuntime::new(tool_config(), Arc::new(ToolCallingModelAdapter));
    runtime.init();
    runtime.set_database(db_adapter);

    let first = block_on(runtime.run_with_tools(
        Content {
            text: "search memory".into(),
            ..Content::default()
        },
        |_, _, _| async move {
            TaskResult::success(
                Content {
                    text: "first memory hit".into(),
                    ..Content::default()
                },
                1,
            )
        },
    ));
    let second = block_on(runtime.run_with_tools(
        Content {
            text: "search memory".into(),
            ..Content::default()
        },
        |_, _, _| async move {
            TaskResult::success(
                Content {
                    text: "second memory hit".into(),
                    ..Content::default()
                },
                1,
            )
        },
    ));

    assert_eq!(first.status, TaskStatus::Success);
    assert_eq!(second.status, TaskStatus::Success);
    assert_eq!(
        second.data.as_ref().map(|content| content.text.as_str()),
        Some("researcher used tool result: second memory hit")
    );

    let steps = db.recorded_steps();
    assert_eq!(
        steps.len(),
        2,
        "fresh runs without retry metadata should record distinct steps"
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
fn runtime_orders_provider_execution_by_priority() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut runtime = runtime();
    runtime.register_provider(Arc::new(OrderedProvider {
        name: "low",
        priority: 1,
        calls: Arc::clone(&calls),
    }));
    runtime.register_provider(Arc::new(OrderedProvider {
        name: "high-first",
        priority: 10,
        calls: Arc::clone(&calls),
    }));
    runtime.register_provider(Arc::new(OrderedProvider {
        name: "high-second",
        priority: 10,
        calls: Arc::clone(&calls),
    }));
    runtime.init();

    let result = block_on(runtime.run(Content {
        text: "order providers".into(),
        ..Content::default()
    }));

    assert_eq!(result.status, TaskStatus::Success);
    let recorded = calls
        .lock()
        .expect("ordered provider mutex should not be poisoned");
    assert_eq!(recorded.as_slice(), ["high-first", "high-second", "low"]);
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
fn runtime_orders_evaluator_execution_by_priority() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut runtime = runtime();
    runtime.register_evaluator(Arc::new(OrderedEvaluator {
        name: "low",
        priority: 1,
        calls: Arc::clone(&calls),
    }));
    runtime.register_evaluator(Arc::new(OrderedEvaluator {
        name: "high-first",
        priority: 10,
        calls: Arc::clone(&calls),
    }));
    runtime.register_evaluator(Arc::new(OrderedEvaluator {
        name: "high-second",
        priority: 10,
        calls: Arc::clone(&calls),
    }));
    runtime.init();

    let result = block_on(runtime.run(Content {
        text: "order evaluators".into(),
        ..Content::default()
    }));

    assert_eq!(result.status, TaskStatus::Success);
    let recorded = calls
        .lock()
        .expect("ordered evaluator mutex should not be poisoned");
    assert_eq!(recorded.as_slice(), ["high-first", "high-second", "low"]);
}

#[test]
fn runtime_retries_when_evaluator_requests_correction() {
    let mut runtime = AgentRuntime::new(config(), Arc::new(RetryAwareModelAdapter));
    runtime.register_evaluator(Arc::new(RetryOnceEvaluator));
    runtime.init();

    let result = block_on(runtime.run(Content {
        text: "answer carefully".into(),
        ..Content::default()
    }));

    assert_eq!(result.status, TaskStatus::Success);
    assert_eq!(
        result.data.as_ref().map(|content| content.text.as_str()),
        Some("revised answer")
    );
    assert!(runtime
        .messages()
        .iter()
        .any(|message| message.role == MessageRole::System
            && message
                .content
                .text
                .contains("Evaluator requested a revision: be specific")));
}

#[test]
fn runtime_marks_failed_when_evaluator_aborts_response() {
    let mut runtime = runtime();
    runtime.register_evaluator(Arc::new(AbortEvaluator));
    runtime.init();

    let result = block_on(runtime.run(Content {
        text: "say something unsafe".into(),
        ..Content::default()
    }));

    assert_eq!(result.status, TaskStatus::Error);
    assert_eq!(result.error.as_deref(), Some("unsafe response"));
    assert_eq!(runtime.snapshot().state.status, AgentStatus::Failed);
}

#[test]
fn runtime_marks_failed_when_evaluator_retry_limit_is_exceeded() {
    let mut runtime = AgentRuntime::new(config(), Arc::new(RetryAwareModelAdapter));
    runtime.register_evaluator(Arc::new(AlwaysRetryEvaluator));
    runtime.init();

    let result = block_on(runtime.run(Content {
        text: "never good enough".into(),
        ..Content::default()
    }));

    assert_eq!(result.status, TaskStatus::Error);
    assert_eq!(
        result.error.as_deref(),
        Some("evaluator retry limit exceeded")
    );
    assert_eq!(runtime.snapshot().state.status, AgentStatus::Failed);
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

#[test]
fn runtime_run_executes_multiple_tool_calls_concurrently() {
    let order = Arc::new(Mutex::new(Vec::new()));
    let mut runtime = AgentRuntime::new(tool_config(), Arc::new(MultiToolCallingModelAdapter));
    runtime.init();

    let result = block_on(runtime.run_with_tools(
        Content {
            text: "search memory twice".into(),
            ..Content::default()
        },
        {
            let order = Arc::clone(&order);
            move |_, _, tool_call| {
                let order = Arc::clone(&order);
                async move {
                    order
                        .lock()
                        .expect("tool order mutex should not be poisoned")
                        .push(format!("start:{}", tool_call.id));
                    PendingOnce::new(()).await;
                    order
                        .lock()
                        .expect("tool order mutex should not be poisoned")
                        .push(format!("end:{}", tool_call.id));

                    TaskResult::success(
                        Content {
                            text: format!("{} hit", tool_call.id),
                            ..Content::default()
                        },
                        1,
                    )
                }
            }
        },
    ));

    assert_eq!(result.status, TaskStatus::Success);
    assert_eq!(
        order
            .lock()
            .expect("tool order mutex should not be poisoned")
            .as_slice(),
        ["start:tool-1", "start:tool-2", "end:tool-1", "end:tool-2"]
    );
}

#[test]
fn runtime_writes_steps_to_database_adapter() {
    use crate::persistence::in_memory::InMemoryAdapter;
    use crate::persistence::StepStatus;

    let db = Arc::new(InMemoryAdapter::new());
    let mut runtime = AgentRuntime::new(tool_config(), Arc::new(ToolCallingModelAdapter));
    runtime.init();
    runtime.set_database(db.clone());

    let result = block_on(runtime.run_with_tools(
        Content {
            text: "search memory".into(),
            ..Content::default()
        },
        |_state, _msg, tool_call| async move {
            TaskResult::success(
                Content {
                    text: format!("result for {}", tool_call.name),
                    ..Content::default()
                },
                1,
            )
        },
    ));

    assert_eq!(result.status, TaskStatus::Success);

    let steps = db.recorded_steps();
    // InMemoryAdapter upserts by logical step idempotency, so pending is overwritten by done
    assert!(!steps.is_empty(), "at least one step should be recorded");
    let last = steps.last().unwrap();
    assert_eq!(last.status, StepStatus::Done);
    assert_eq!(last.step_type, "tool");
}
