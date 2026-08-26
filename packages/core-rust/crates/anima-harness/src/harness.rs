use std::sync::Arc;

use anima_core::{
    AgentConfig, AgentRuntime, AgentRuntimeSnapshot, AgentState, Content, EngineEvent, Message,
    ModelAdapter, TaskResult,
};

use crate::config::HarnessBuilder;
use crate::tool::ToolSet;

/// An embeddable agent harness: system prompt + tools + agentic loop +
/// provider credentials, composed on top of `anima-core`'s [`AgentRuntime`].
///
/// Build one with [`Harness::builder`].
pub struct Harness {
    runtime: AgentRuntime,
    adapter: Arc<dyn ModelAdapter>,
    tools: ToolSet,
}

impl Harness {
    pub fn builder() -> HarnessBuilder {
        HarnessBuilder::new()
    }

    pub(crate) fn new(
        config: AgentConfig,
        adapter: Arc<dyn ModelAdapter>,
        tools: ToolSet,
        event_listener: Option<Arc<dyn Fn(EngineEvent) + Send + Sync>>,
    ) -> Self {
        let mut runtime = AgentRuntime::new(config, adapter.clone());
        if let Some(listener) = event_listener {
            runtime.set_event_listener(listener);
        }
        runtime.init();
        Self {
            runtime,
            adapter,
            tools,
        }
    }

    /// Rebuild a harness from a previously captured [`Harness::snapshot`].
    ///
    /// The event listener is not part of the snapshot; re-attach it with
    /// [`Harness::set_event_listener`] if needed.
    pub fn restore(
        snapshot: AgentRuntimeSnapshot,
        adapter: Arc<dyn ModelAdapter>,
        tools: ToolSet,
    ) -> Self {
        Self {
            runtime: AgentRuntime::from_snapshot(snapshot, adapter.clone()),
            adapter,
            tools,
        }
    }

    pub fn set_event_listener(&mut self, listener: impl Fn(EngineEvent) + Send + Sync + 'static) {
        self.runtime.set_event_listener(Arc::new(listener));
    }

    pub fn id(&self) -> &str {
        self.runtime.id()
    }

    pub fn config(&self) -> &AgentConfig {
        self.runtime.config()
    }

    pub fn state(&self) -> AgentState {
        self.runtime.state()
    }

    pub fn messages(&self) -> &[Message] {
        self.runtime.messages()
    }

    pub fn tools(&self) -> &ToolSet {
        &self.tools
    }

    pub fn adapter(&self) -> &Arc<dyn ModelAdapter> {
        &self.adapter
    }

    /// Capture the full runtime state for later [`Harness::restore`].
    pub fn snapshot(&self) -> AgentRuntimeSnapshot {
        self.runtime.snapshot()
    }

    /// Run a single task. Tool calls requested by the model are dispatched to
    /// the registered [`ToolSet`] until the model finishes or the tool
    /// iteration limit is hit.
    pub async fn run(&mut self, input: impl Into<String>) -> TaskResult<Content> {
        let tools = self.tools.clone();
        self.runtime
            .run_with_tools(text_content(input), move |_, _, tool_call| {
                let tools = tools.clone();
                async move {
                    match tools.execute(&tool_call.name, tool_call.args).await {
                        Ok(content) => TaskResult::success(content, 0),
                        Err(error) => TaskResult::error(error, 0),
                    }
                }
            })
            .await
    }

    /// Like [`Harness::run`], but includes all previously recorded messages as
    /// conversation history, so the model sees the whole session.
    pub async fn chat(&mut self, input: impl Into<String>) -> TaskResult<Content> {
        let history = self.runtime.messages().to_vec();
        let tools = self.tools.clone();
        self.runtime
            .run_with_context_and_tools(history, text_content(input), move |_, _, tool_call| {
                let tools = tools.clone();
                async move {
                    match tools.execute(&tool_call.name, tool_call.args).await {
                        Ok(content) => TaskResult::success(content, 0),
                        Err(error) => TaskResult::error(error, 0),
                    }
                }
            })
            .await
    }
}

fn text_content(input: impl Into<String>) -> Content {
    Content {
        text: input.into(),
        attachments: None,
        metadata: None,
    }
}
