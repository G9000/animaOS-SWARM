//! Fixtures for the M3 coordinator and route tests: a model that streams
//! scripted steps and can hold each call open.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use anima_core::{
    AgentConfig, AgentSettings, Content, DataValue, ModelAdapter, ModelGenerateRequest,
    ModelGenerateResponse, ModelStopReason, ModelStreamFrame, ModelStreamSink, TokenUsage,
    ToolCall,
};
use async_trait::async_trait;
use tokio::sync::{RwLock, Semaphore};

use super::{AgentRunCoordinator, AgentRunRequest, RunRoom};
use crate::live::{LiveDelivery, LiveSubscription};
use crate::runs::RunSource;
use crate::state::DaemonState;

/// One scripted model call.
#[derive(Clone, Debug)]
pub(crate) enum Step {
    /// Streams these chunks, then finishes with their concatenation.
    Text(Vec<&'static str>),
    /// Asks for these tool calls.
    Tools(Vec<ToolCall>),
    /// Fails the call with this error.
    Fail(&'static str),
    /// Streams these chunks, then never finishes: the call ends only when
    /// the run drops it (a stop).
    Hold(Vec<&'static str>),
}

/// Holds each model call open: a call adds an `entered` permit when it
/// starts, then waits for one `release` permit.
#[derive(Clone)]
pub(crate) struct Gate {
    pub(crate) entered: Arc<Semaphore>,
    pub(crate) release: Arc<Semaphore>,
}

impl Gate {
    pub(crate) fn new() -> Self {
        Self {
            entered: Arc::new(Semaphore::new(0)),
            release: Arc::new(Semaphore::new(0)),
        }
    }

    /// Waits until a model call is being held.
    pub(crate) async fn entered(&self) {
        tokio::time::timeout(Duration::from_secs(5), self.entered.acquire())
            .await
            .expect("a model call starts within five seconds")
            .expect("the gate stays open")
            .forget();
    }

    /// Lets one held model call continue.
    pub(crate) fn release(&self) {
        self.release.add_permits(1);
    }
}

/// A model adapter that answers each streamed call (every run's model call)
/// with the next scripted step (`Step::Text(vec!["done"])` once the script
/// runs out), and each `generate` call (the secondary calls: compaction and
/// titles) with the next secondary step (`Step::Fail` once those run out, so
/// no test gets a summary or a title it did not script).
pub(crate) struct ScriptedModel {
    steps: StdMutex<VecDeque<Step>>,
    secondary: StdMutex<VecDeque<Step>>,
    gate: Option<Gate>,
    requests: StdMutex<Vec<ModelGenerateRequest>>,
    secondary_requests: StdMutex<Vec<ModelGenerateRequest>>,
}

impl ScriptedModel {
    pub(crate) fn new(steps: Vec<Step>) -> Arc<Self> {
        Self::build(steps, Vec::new(), None)
    }

    /// Holds each streamed call at `gate` before answering it.
    pub(crate) fn gated(steps: Vec<Step>, gate: Gate) -> Arc<Self> {
        Self::build(steps, Vec::new(), Some(gate))
    }

    /// Also scripts the `generate` calls (a summary or a title).
    #[allow(dead_code)] // M3 Tasks 12 and 13 script summaries and titles.
    pub(crate) fn with_secondary(steps: Vec<Step>, secondary: Vec<Step>) -> Arc<Self> {
        Self::build(steps, secondary, None)
    }

    fn build(steps: Vec<Step>, secondary: Vec<Step>, gate: Option<Gate>) -> Arc<Self> {
        Arc::new(Self {
            steps: StdMutex::new(steps.into()),
            secondary: StdMutex::new(secondary.into()),
            gate,
            requests: StdMutex::new(Vec::new()),
            secondary_requests: StdMutex::new(Vec::new()),
        })
    }

    /// Every streamed request (the runs' model calls), oldest first.
    pub(crate) fn requests(&self) -> Vec<ModelGenerateRequest> {
        self.requests.lock().unwrap().clone()
    }

    /// Every `generate` request (compaction and titles), oldest first.
    #[allow(dead_code)] // M3 Tasks 12 and 13 read the summary and title requests.
    pub(crate) fn secondary_requests(&self) -> Vec<ModelGenerateRequest> {
        self.secondary_requests.lock().unwrap().clone()
    }

    async fn next(&self, request: &ModelGenerateRequest) -> Step {
        self.requests.lock().unwrap().push(request.clone());
        if let Some(gate) = &self.gate {
            gate.entered.add_permits(1);
            gate.release
                .acquire()
                .await
                .expect("the gate stays open")
                .forget();
        }
        self.steps
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Step::Text(vec!["done"]))
    }

    fn next_secondary(&self, request: &ModelGenerateRequest) -> Step {
        self.secondary_requests
            .lock()
            .unwrap()
            .push(request.clone());
        self.secondary
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Step::Fail("no secondary reply scripted"))
    }
}

fn response(text: String, tool_calls: Option<Vec<ToolCall>>) -> ModelGenerateResponse {
    let stop_reason = if tool_calls.is_some() {
        ModelStopReason::ToolCall
    } else {
        ModelStopReason::End
    };
    ModelGenerateResponse {
        content: Content {
            text,
            ..Content::default()
        },
        tool_calls,
        usage: TokenUsage {
            prompt_tokens: 10,
            completion_tokens: 2,
            total_tokens: 12,
            ..TokenUsage::default()
        },
        stop_reason,
    }
}

#[async_trait]
impl ModelAdapter for ScriptedModel {
    fn provider(&self) -> &str {
        "scripted"
    }

    async fn generate(
        &self,
        _config: &AgentConfig,
        request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String> {
        match self.next_secondary(request) {
            Step::Text(chunks) => Ok(response(chunks.concat(), None)),
            Step::Tools(calls) => Ok(response(String::new(), Some(calls))),
            Step::Fail(error) => Err(error.to_string()),
            Step::Hold(_) => std::future::pending().await,
        }
    }

    async fn stream(
        &self,
        _config: &AgentConfig,
        request: &ModelGenerateRequest,
        sink: &dyn ModelStreamSink,
    ) -> Result<(), String> {
        match self.next(request).await {
            Step::Text(chunks) => {
                for chunk in &chunks {
                    sink.emit(ModelStreamFrame::TextDelta((*chunk).to_string()))
                        .await?;
                }
                sink.emit(ModelStreamFrame::Final(response(chunks.concat(), None)))
                    .await
            }
            Step::Tools(calls) => {
                sink.emit(ModelStreamFrame::Final(response(
                    String::new(),
                    Some(calls),
                )))
                .await
            }
            Step::Fail(error) => Err(error.to_string()),
            Step::Hold(chunks) => {
                for chunk in &chunks {
                    sink.emit(ModelStreamFrame::TextDelta((*chunk).to_string()))
                        .await?;
                }
                std::future::pending().await
            }
        }
    }
}

pub(crate) fn calculate_call(id: &str, expression: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: "calculate".into(),
        args: BTreeMap::from([(
            "expression".to_string(),
            DataValue::String(expression.into()),
        )]),
    }
}

/// An agent that may use `calculate`.
pub(crate) fn companion_config(name: &str) -> AgentConfig {
    AgentConfig {
        name: name.into(),
        model: "gpt-5.4".into(),
        bio: None,
        lore: None,
        knowledge: None,
        topics: None,
        adjectives: None,
        style: None,
        provider: Some("openai".into()),
        system: None,
        tools: Some(
            crate::tools::ToolRegistry::new()
                .resolve_descriptors(["calculate"])
                .unwrap(),
        ),
        plugins: None,
        settings: Some(AgentSettings::default()),
    }
}

/// `companion_config` as the workspace lead, which may spawn helpers.
pub(crate) fn lead_config(name: &str) -> AgentConfig {
    let mut config = companion_config(name);
    config
        .settings
        .as_mut()
        .unwrap()
        .additional
        .insert("workspaceRole".into(), DataValue::String("lead".into()));
    config
}

/// A coordinator over a fresh daemon whose one agent (`companion_config`)
/// uses `model`.
pub(crate) async fn coordinator_with(
    model: Arc<dyn ModelAdapter>,
) -> (AgentRunCoordinator, String) {
    let state = Arc::new(RwLock::new(DaemonState::with_model_adapter(model)));
    let agent_id = state
        .write()
        .await
        .create_agent(companion_config("companion"))
        .unwrap()
        .state
        .id;
    (
        AgentRunCoordinator::new(state, Arc::new(Semaphore::new(8))),
        agent_id,
    )
}

pub(crate) fn chat_request(agent_id: &str, room_id: &str, text: &str) -> AgentRunRequest {
    AgentRunRequest {
        agent_id: agent_id.into(),
        content: Content {
            text: text.into(),
            ..Content::default()
        },
        room: RunRoom::Stable(room_id.into()),
        idempotency_key: None,
        source: RunSource::Api,
        source_ref: None,
        parent: None,
    }
}

/// A subscription's events as JSON, up to and including the first of
/// `type_name`.
pub(crate) async fn events_until(
    subscription: &mut LiveSubscription,
    type_name: &str,
) -> Vec<serde_json::Value> {
    let mut events = Vec::new();
    loop {
        let delivery = tokio::time::timeout(Duration::from_secs(5), subscription.next())
            .await
            .expect("an event arrives within five seconds")
            .expect("the channel stays open");
        let LiveDelivery::Event(event) = delivery else {
            panic!("the subscription lagged");
        };
        let value = event.to_json(events.len() as u64 + 1);
        let done = value["type"] == type_name;
        events.push(value);
        if done {
            return events;
        }
    }
}
