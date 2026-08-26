use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use anima_harness::anima_core::{
    AgentConfig, Content, DataValue, EventType, ModelAdapter, ModelGenerateRequest,
    ModelGenerateResponse, ModelStopReason, TaskStatus, TokenUsage, ToolCall, ToolDescriptor,
};
use anima_harness::{Harness, HarnessError, HarnessTool};
use async_trait::async_trait;

#[derive(Clone, Default)]
struct ScriptedAdapter {
    responses: Arc<Mutex<VecDeque<ModelGenerateResponse>>>,
    requests: Arc<Mutex<Vec<ModelGenerateRequest>>>,
}

impl ScriptedAdapter {
    fn with_responses(responses: Vec<ModelGenerateResponse>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into())),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl ModelAdapter for ScriptedAdapter {
    fn provider(&self) -> &str {
        "scripted"
    }

    async fn generate(
        &self,
        _config: &AgentConfig,
        request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String> {
        self.requests.lock().unwrap().push(request.clone());
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| "no scripted response left".to_owned())
    }
}

fn text_response(text: &str) -> ModelGenerateResponse {
    ModelGenerateResponse {
        content: Content {
            text: text.to_owned(),
            attachments: None,
            metadata: None,
        },
        tool_calls: None,
        usage: TokenUsage::default(),
        stop_reason: ModelStopReason::End,
    }
}

fn tool_call_response(name: &str, args: BTreeMap<String, DataValue>) -> ModelGenerateResponse {
    ModelGenerateResponse {
        content: Content::default(),
        tool_calls: Some(vec![ToolCall {
            id: format!("call-{name}"),
            name: name.to_owned(),
            args,
        }]),
        usage: TokenUsage::default(),
        stop_reason: ModelStopReason::ToolCall,
    }
}

#[derive(Clone, Default)]
struct EchoTool {
    calls: Arc<Mutex<Vec<BTreeMap<String, DataValue>>>>,
}

#[async_trait]
impl HarnessTool for EchoTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "echo".to_owned(),
            description: "Echo the `text` argument back.".to_owned(),
            parameters_schema: BTreeMap::new(),
            examples: None,
        }
    }

    async fn execute(&self, args: BTreeMap<String, DataValue>) -> Result<Content, String> {
        let text = match args.get("text") {
            Some(DataValue::String(text)) => text.clone(),
            _ => return Err("missing string argument: text".to_owned()),
        };
        self.calls.lock().unwrap().push(args);
        Ok(Content {
            text,
            attachments: None,
            metadata: None,
        })
    }
}

fn harness_with(adapter: ScriptedAdapter) -> Harness {
    Harness::builder()
        .model("test-model")
        .adapter(Arc::new(adapter))
        .build()
        .expect("harness should build")
}

#[tokio::test]
async fn run_returns_model_text() {
    let adapter = ScriptedAdapter::with_responses(vec![text_response("hello there")]);
    let mut harness = harness_with(adapter);

    let result = harness.run("hi").await;

    assert_eq!(result.status, TaskStatus::Success);
    assert_eq!(
        result.data.map(|content| content.text).as_deref(),
        Some("hello there")
    );
}

#[tokio::test]
async fn run_dispatches_tool_calls_then_returns_final_text() {
    let adapter = ScriptedAdapter::with_responses(vec![
        tool_call_response(
            "echo",
            BTreeMap::from([("text".to_owned(), DataValue::String("ping".to_owned()))]),
        ),
        text_response("done"),
    ]);
    let echo = EchoTool::default();
    let calls = echo.calls.clone();
    let mut harness = Harness::builder()
        .model("test-model")
        .adapter(Arc::new(adapter))
        .tool(echo)
        .build()
        .expect("harness should build");

    let result = harness.run("use the tool").await;

    assert_eq!(result.status, TaskStatus::Success);
    assert_eq!(
        result.data.map(|content| content.text).as_deref(),
        Some("done")
    );
    assert_eq!(
        calls.lock().unwrap().as_slice(),
        &[BTreeMap::from([(
            "text".to_owned(),
            DataValue::String("ping".to_owned())
        )])]
    );
}

#[tokio::test]
async fn unknown_tool_is_reported_back_to_the_model() {
    let adapter = ScriptedAdapter::with_responses(vec![
        tool_call_response("missing", BTreeMap::new()),
        text_response("recovered"),
    ]);
    let mut harness = harness_with(adapter);

    let result = harness.run("use an unknown tool").await;

    assert_eq!(result.status, TaskStatus::Success);
    assert!(
        harness
            .messages()
            .iter()
            .any(|message| message.content.text.contains("Unknown tool: missing")),
        "expected a tool message reporting the unknown tool"
    );
}

#[tokio::test]
async fn tool_iteration_limit_is_enforced() {
    let adapter = ScriptedAdapter::with_responses(vec![
        tool_call_response("echo", BTreeMap::new()),
        tool_call_response("echo", BTreeMap::new()),
        text_response("unreachable"),
    ]);
    let mut harness = Harness::builder()
        .model("test-model")
        .adapter(Arc::new(adapter))
        .tool(EchoTool::default())
        .max_tool_iterations(1)
        .build()
        .expect("harness should build");

    let result = harness.run("loop forever").await;

    assert_eq!(result.status, TaskStatus::Error);
    assert_eq!(
        result.error.as_deref(),
        Some("tool iteration limit exceeded")
    );
}

#[tokio::test]
async fn events_are_emitted_to_listener() {
    let events: Arc<Mutex<Vec<EventType>>> = Arc::new(Mutex::new(Vec::new()));
    let listener_events = events.clone();
    let adapter = ScriptedAdapter::with_responses(vec![text_response("ok")]);
    let mut harness = Harness::builder()
        .model("test-model")
        .adapter(Arc::new(adapter))
        .on_event(move |event| listener_events.lock().unwrap().push(event.event_type))
        .build()
        .expect("harness should build");

    let result = harness.run("hi").await;

    assert_eq!(result.status, TaskStatus::Success);
    let events = events.lock().unwrap();
    assert!(events.contains(&EventType::AgentSpawned));
    assert!(events.contains(&EventType::TaskStarted));
    assert!(events.contains(&EventType::TaskCompleted));
}

#[tokio::test]
async fn chat_includes_previous_messages_in_history() {
    let adapter = ScriptedAdapter::with_responses(vec![text_response("one"), text_response("two")]);
    let requests = adapter.requests.clone();
    let mut harness = harness_with(adapter);

    let first = harness.chat("first").await;
    let second = harness.chat("second").await;

    assert_eq!(first.status, TaskStatus::Success);
    assert_eq!(second.status, TaskStatus::Success);
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1].messages.len() > requests[0].messages.len(),
        "second chat request should include prior conversation history"
    );
}

#[tokio::test]
async fn snapshot_round_trip_preserves_conversation() {
    let adapter = ScriptedAdapter::with_responses(vec![text_response("remembered")]);
    let mut harness = Harness::builder()
        .name("snapshot-agent")
        .model("test-model")
        .adapter(Arc::new(adapter.clone()))
        .tool(EchoTool::default())
        .build()
        .expect("harness should build");
    let result = harness.run("hi").await;
    assert_eq!(result.status, TaskStatus::Success);

    let snapshot = harness.snapshot();
    let restored = Harness::restore(snapshot, Arc::new(adapter), Default::default());

    assert_eq!(restored.config().name, "snapshot-agent");
    assert_eq!(restored.messages(), harness.messages());
}

#[test]
fn builder_requires_a_model() {
    let adapter = ScriptedAdapter::default();
    let result = Harness::builder().adapter(Arc::new(adapter)).build();
    assert_eq!(result.err(), Some(HarnessError::MissingModel));
}

#[test]
fn builder_rejects_unknown_providers() {
    let result = Harness::builder()
        .provider("not-a-provider")
        .model("test-model")
        .build();
    assert_eq!(
        result.err(),
        Some(HarnessError::UnknownProvider("not-a-provider".to_owned()))
    );
}

#[test]
fn builder_requires_a_provider_without_custom_adapter() {
    let result = Harness::builder().model("test-model").build();
    assert_eq!(result.err(), Some(HarnessError::MissingProvider));
}
