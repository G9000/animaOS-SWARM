//! Live run frames and streamed model calls (spec §4.5).

use super::{AgentRuntime, RunFrame, RunObserver, MODEL_STREAM_WITHOUT_FINAL};
use crate::agent::{AgentConfig, TokenUsage, ToolDescriptor};
use crate::components::{Evaluator, EvaluatorResult};
use crate::model::{
    ModelAdapter, ModelGenerateRequest, ModelGenerateResponse, ModelStopReason, ModelStreamFrame,
    ModelStreamSink, ToolCall,
};
use crate::primitives::{
    Content, DataValue, LockRecover, Message, MessageRole, TaskResult, TaskStatus,
};
use async_trait::async_trait;
use futures::executor::block_on;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Frames(Mutex<Vec<RunFrame>>);

impl RunObserver for Frames {
    fn on_frame(&self, frame: RunFrame) {
        self.0.lock_recover().push(frame);
    }
}

impl Frames {
    fn take(&self) -> Vec<RunFrame> {
        std::mem::take(&mut *self.0.lock_recover())
    }
}

fn text(value: &str) -> Content {
    Content {
        text: value.into(),
        ..Content::default()
    }
}

fn usage(prompt: u64, completion: u64) -> TokenUsage {
    TokenUsage {
        prompt_tokens: prompt,
        completion_tokens: completion,
        total_tokens: prompt + completion,
        ..TokenUsage::default()
    }
}

fn config(tools: &[&str]) -> AgentConfig {
    AgentConfig {
        name: "streamer".into(),
        model: "stream-model".into(),
        bio: None,
        lore: None,
        knowledge: None,
        topics: None,
        adjectives: None,
        style: None,
        provider: None,
        system: None,
        tools: (!tools.is_empty()).then(|| {
            tools
                .iter()
                .map(|name| ToolDescriptor {
                    name: (*name).into(),
                    description: String::new(),
                    parameters_schema: BTreeMap::new(),
                    examples: None,
                })
                .collect()
        }),
        plugins: None,
        settings: None,
    }
}

/// Streams its reply in two deltas. With `tool` set, the first call asks for
/// that tool instead, streaming its own two-delta preamble.
struct StreamingModel {
    tool: Option<&'static str>,
}

#[async_trait]
impl ModelAdapter for StreamingModel {
    fn provider(&self) -> &str {
        "streaming"
    }

    async fn generate(
        &self,
        _config: &AgentConfig,
        _request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String> {
        Err("the runtime streams every model call".into())
    }

    async fn stream(
        &self,
        _config: &AgentConfig,
        request: &ModelGenerateRequest,
        sink: &dyn ModelStreamSink,
    ) -> Result<(), String> {
        let has_tool_result = request
            .messages
            .iter()
            .any(|message| message.role == MessageRole::Tool);
        if let (Some(tool), false) = (self.tool, has_tool_result) {
            sink.emit(ModelStreamFrame::TextDelta("Let me ".into()))
                .await?;
            sink.emit(ModelStreamFrame::TextDelta("check.".into()))
                .await?;
            return sink
                .emit(ModelStreamFrame::Final(ModelGenerateResponse {
                    content: text("Let me check."),
                    tool_calls: Some(vec![ToolCall {
                        id: "call-1".into(),
                        name: tool.into(),
                        args: BTreeMap::from([(
                            "query".into(),
                            DataValue::String("weather".into()),
                        )]),
                    }]),
                    usage: usage(3, 2),
                    stop_reason: ModelStopReason::ToolCall,
                }))
                .await;
        }
        sink.emit(ModelStreamFrame::TextDelta("Hello ".into()))
            .await?;
        sink.emit(ModelStreamFrame::TextDelta("there".into()))
            .await?;
        sink.emit(ModelStreamFrame::Final(ModelGenerateResponse {
            content: text("Hello there"),
            tool_calls: None,
            usage: usage(5, 2),
            stop_reason: ModelStopReason::End,
        }))
        .await
    }
}

/// Streams `partial`, then fails (`error`) or ends without a final frame.
struct BrokenStream {
    partial: &'static str,
    error: Option<&'static str>,
}

#[async_trait]
impl ModelAdapter for BrokenStream {
    fn provider(&self) -> &str {
        "broken"
    }

    async fn generate(
        &self,
        _config: &AgentConfig,
        _request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String> {
        Err("the runtime streams every model call".into())
    }

    async fn stream(
        &self,
        _config: &AgentConfig,
        _request: &ModelGenerateRequest,
        sink: &dyn ModelStreamSink,
    ) -> Result<(), String> {
        if !self.partial.is_empty() {
            sink.emit(ModelStreamFrame::TextDelta(self.partial.into()))
                .await?;
        }
        match self.error {
            Some(error) => Err(error.into()),
            None => Ok(()),
        }
    }
}

/// Implements only `generate`, so the trait's default `stream` is used.
struct GenerateOnly;

#[async_trait]
impl ModelAdapter for GenerateOnly {
    fn provider(&self) -> &str {
        "generate-only"
    }

    async fn generate(
        &self,
        _config: &AgentConfig,
        _request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String> {
        Ok(ModelGenerateResponse {
            content: text("done"),
            tool_calls: None,
            usage: usage(1, 1),
            stop_reason: ModelStopReason::End,
        })
    }
}

/// Asks for one revision, then accepts.
#[derive(Default)]
struct ReviseOnce {
    calls: Mutex<usize>,
}

#[async_trait]
impl Evaluator for ReviseOnce {
    fn name(&self) -> &str {
        "revise-once"
    }

    fn description(&self) -> &str {
        "asks for one revision"
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
        let mut calls = self.calls.lock_recover();
        *calls += 1;
        Ok(if *calls == 1 {
            EvaluatorResult::retry("be warmer")
        } else {
            EvaluatorResult::accept()
        })
    }
}

fn runtime_with(adapter: Arc<dyn ModelAdapter>, tools: &[&str]) -> (AgentRuntime, Arc<Frames>) {
    let frames = Arc::new(Frames::default());
    let mut runtime = AgentRuntime::new(config(tools), adapter);
    runtime.init();
    runtime.set_run_observer(frames.clone());
    (runtime, frames)
}

fn metadata<'a>(message: &'a Message, key: &str) -> Option<&'a DataValue> {
    message.content.metadata.as_ref()?.get(key)
}

fn string_metadata<'a>(message: &'a Message, key: &str) -> Option<&'a str> {
    match metadata(message, key) {
        Some(DataValue::String(value)) => Some(value),
        _ => None,
    }
}

#[test]
fn a_streamed_reply_emits_step_frames_and_is_tagged_with_its_step() {
    let (mut runtime, frames) = runtime_with(Arc::new(StreamingModel { tool: None }), &[]);
    runtime.set_run_id("run_1");
    let events_before = runtime.snapshot().event_count;

    let result = block_on(runtime.run(text("hi")));

    assert_eq!(result.status, TaskStatus::Success);
    assert_eq!(
        result.data.as_ref().map(|content| content.text.as_str()),
        Some("Hello there")
    );
    let messages = runtime.messages();
    assert_eq!(messages.len(), 2);
    assert_eq!(string_metadata(&messages[0], "runId"), Some("run_1"));
    assert_eq!(
        string_metadata(&messages[0], "stepId"),
        None,
        "the owner's input is not a model call"
    );
    assert_eq!(string_metadata(&messages[1], "runId"), Some("run_1"));
    assert_eq!(string_metadata(&messages[1], "stepId"), Some("run_1:1"));
    assert_eq!(
        result
            .data
            .as_ref()
            .and_then(|content| content.metadata.as_ref()),
        None,
        "the task result is not tagged"
    );
    assert_eq!(
        frames.take(),
        vec![
            RunFrame::StepStarted {
                step_id: "run_1:1".into()
            },
            RunFrame::TextDelta {
                step_id: "run_1:1".into(),
                text: "Hello ".into()
            },
            RunFrame::TextDelta {
                step_id: "run_1:1".into(),
                text: "there".into()
            },
            RunFrame::StepUsage {
                step_id: "run_1:1".into(),
                usage: usage(5, 2)
            },
            RunFrame::StepFinished {
                step_id: "run_1:1".into(),
                message_id: Some(messages[1].id.clone())
            },
        ]
    );
    assert_eq!(runtime.state().token_usage.total_tokens, 7);
    // Frames are never recorded: the run adds exactly the events it did
    // before streaming existed (started ×2, two messages, completed ×2, tokens).
    assert_eq!(runtime.snapshot().event_count - events_before, 7);
    assert!(runtime
        .events()
        .iter()
        .all(|event| event.data != DataValue::String("Hello ".into())));
}

#[test]
fn tool_steps_emit_tool_frames_and_tag_tool_results() {
    let (mut runtime, frames) = runtime_with(
        Arc::new(StreamingModel {
            tool: Some("search"),
        }),
        &["search"],
    );
    runtime.set_run_id("run_2");

    let result = block_on(
        runtime.run_with_tools(text("weather?"), |_, _, _| async move {
            TaskResult::success(text("sunny"), 4)
        }),
    );

    assert_eq!(result.status, TaskStatus::Success);
    let messages = runtime.messages();
    let roles: Vec<MessageRole> = messages.iter().map(|message| message.role).collect();
    assert_eq!(
        roles,
        [
            MessageRole::User,
            MessageRole::Assistant,
            MessageRole::Tool,
            MessageRole::Assistant
        ]
    );
    assert_eq!(string_metadata(&messages[1], "stepId"), Some("run_2:1"));
    assert_eq!(string_metadata(&messages[2], "stepId"), Some("run_2:1"));
    assert_eq!(string_metadata(&messages[2], "toolCallId"), Some("call-1"));
    assert_eq!(string_metadata(&messages[2], "toolStatus"), Some("success"));
    assert!(matches!(
        metadata(&messages[2], "toolDurationMs"),
        Some(DataValue::Number(_))
    ));
    assert_eq!(string_metadata(&messages[3], "stepId"), Some("run_2:2"));

    let frames = frames.take();
    let kinds: Vec<&str> = frames
        .iter()
        .map(|frame| match frame {
            RunFrame::StepStarted { .. } => "step",
            RunFrame::TextDelta { .. } => "delta",
            RunFrame::StepUsage { .. } => "usage",
            RunFrame::StepFinished { .. } => "finished",
            RunFrame::ToolStarted { .. } => "tool",
            RunFrame::ToolFinished { .. } => "tool-done",
            #[allow(unreachable_patterns)]
            _ => "other",
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "step",
            "delta",
            "delta",
            "usage",
            "finished",
            "tool",
            "tool-done",
            "step",
            "delta",
            "delta",
            "usage",
            "finished"
        ]
    );
    assert!(frames.contains(&RunFrame::StepFinished {
        step_id: "run_2:1".into(),
        message_id: Some(messages[1].id.clone()),
    }));
    let RunFrame::ToolStarted { step_id, tool_call } = &frames[5] else {
        panic!("expected the tool to start: {:?}", frames[5]);
    };
    assert_eq!(step_id, "run_2:1");
    assert_eq!(tool_call.id, "call-1");
    assert_eq!(tool_call.name, "search");
    let RunFrame::ToolFinished {
        step_id,
        tool_call_id,
        name,
        status,
        result,
        recovered,
        ..
    } = &frames[6]
    else {
        panic!("expected the tool to finish: {:?}", frames[6]);
    };
    assert_eq!(
        (
            step_id.as_str(),
            tool_call_id.as_str(),
            name.as_str(),
            *status,
            result.as_str(),
            *recovered
        ),
        (
            "run_2:1",
            "call-1",
            "search",
            TaskStatus::Success,
            "sunny",
            false
        )
    );
}

#[test]
fn a_runtime_without_a_run_id_records_no_run_metadata() {
    let (mut runtime, frames) = runtime_with(Arc::new(StreamingModel { tool: None }), &[]);

    let result = block_on(runtime.run(text("hi")));

    assert_eq!(result.status, TaskStatus::Success);
    assert!(runtime.messages().iter().all(
        |message| metadata(message, "runId").is_none() && metadata(message, "stepId").is_none()
    ));
    assert_eq!(
        frames.take()[0],
        RunFrame::StepStarted {
            step_id: "run:1".into()
        }
    );
}

#[test]
fn an_evaluator_revision_keeps_the_earlier_step_marked_revised() {
    let (mut runtime, _frames) = runtime_with(Arc::new(StreamingModel { tool: None }), &[]);
    runtime.set_run_id("run_3");
    runtime.register_evaluator(Arc::new(ReviseOnce::default()));

    let result = block_on(runtime.run(text("hi")));

    assert_eq!(result.status, TaskStatus::Success);
    let assistants: Vec<&Message> = runtime
        .messages()
        .iter()
        .filter(|message| message.role == MessageRole::Assistant)
        .collect();
    assert_eq!(
        assistants.len(),
        2,
        "the revised draft stays in the transcript"
    );
    assert_eq!(
        metadata(assistants[0], "revised"),
        Some(&DataValue::Bool(true))
    );
    assert_eq!(string_metadata(assistants[0], "stepId"), Some("run_3:1"));
    assert_eq!(metadata(assistants[1], "revised"), None);
    assert_eq!(string_metadata(assistants[1], "stepId"), Some("run_3:2"));
}

#[test]
fn a_failed_stream_keeps_its_partial_text_as_an_incomplete_reply() {
    let (mut runtime, frames) = runtime_with(
        Arc::new(BrokenStream {
            partial: "Half an ans",
            error: Some("connection reset"),
        }),
        &[],
    );
    runtime.set_run_id("run_4");

    let result = block_on(runtime.run(text("hi")));

    assert_eq!(result.status, TaskStatus::Error);
    assert_eq!(result.error.as_deref(), Some("connection reset"));
    let messages = runtime.messages();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1].role, MessageRole::Assistant);
    assert_eq!(messages[1].content.text, "Half an ans");
    assert_eq!(
        metadata(&messages[1], "incomplete"),
        Some(&DataValue::Bool(true))
    );
    assert_eq!(string_metadata(&messages[1], "stepId"), Some("run_4:1"));
    assert_eq!(
        frames.take().last(),
        Some(&RunFrame::StepFinished {
            step_id: "run_4:1".into(),
            message_id: Some(messages[1].id.clone()),
        })
    );

    let (mut quiet, frames) = runtime_with(
        Arc::new(BrokenStream {
            partial: "",
            error: Some("connection refused"),
        }),
        &[],
    );
    let result = block_on(quiet.run(text("hi")));
    assert_eq!(result.error.as_deref(), Some("connection refused"));
    assert_eq!(
        quiet.messages().len(),
        1,
        "nothing streamed, nothing recorded"
    );
    assert_eq!(
        frames.take().last(),
        Some(&RunFrame::StepFinished {
            step_id: "run:1".into(),
            message_id: None,
        })
    );
}

#[test]
fn a_stream_without_a_final_response_fails_the_run() {
    let (mut runtime, _frames) = runtime_with(
        Arc::new(BrokenStream {
            partial: "never finished",
            error: None,
        }),
        &[],
    );

    let result = block_on(runtime.run(text("hi")));

    assert_eq!(result.status, TaskStatus::Error);
    assert_eq!(result.error.as_deref(), Some(MODEL_STREAM_WITHOUT_FINAL));
    assert_eq!(runtime.messages()[1].content.text, "never finished");
    assert_eq!(
        metadata(&runtime.messages()[1], "incomplete"),
        Some(&DataValue::Bool(true))
    );
}

#[test]
fn adapters_that_only_generate_still_run_through_the_default_stream() {
    let (mut runtime, frames) = runtime_with(Arc::new(GenerateOnly), &[]);
    runtime.set_run_id("run_5");

    let result = block_on(runtime.run(text("hi")));

    assert_eq!(result.status, TaskStatus::Success);
    let reply = runtime.messages()[1].id.clone();
    assert_eq!(
        frames.take(),
        vec![
            RunFrame::StepStarted {
                step_id: "run_5:1".into()
            },
            RunFrame::StepUsage {
                step_id: "run_5:1".into(),
                usage: usage(1, 1)
            },
            RunFrame::StepFinished {
                step_id: "run_5:1".into(),
                message_id: Some(reply)
            },
        ]
    );
}
