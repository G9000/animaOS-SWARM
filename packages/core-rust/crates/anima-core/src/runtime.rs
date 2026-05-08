use std::collections::BTreeMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use futures::future::join_all;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::agent::{AgentConfig, AgentState, AgentStatus, TokenUsage};
use crate::components::{Evaluator, EvaluatorDecision, Provider};
use crate::events::{EngineEvent, EventType};
use crate::model::{ModelAdapter, ModelGenerateRequest, ModelStopReason, ToolCall};
use crate::persistence::{DatabaseAdapter, Step, StepStatus};
use crate::primitives::{
    now_millis, Content, DataValue, Message, MessageRole, TaskResult, TaskStatus,
};
use crate::runtime_serde::{
    data_value_json, persisted_task_result, task_result_data_value, tool_result_text_data_value,
    tool_step_input_json, tool_step_output_json,
};

static NEXT_AGENT_ID: AtomicU64 = AtomicU64::new(0);
static NEXT_EVENT_ID: AtomicU64 = AtomicU64::new(0);
static NEXT_MESSAGE_ID: AtomicU64 = AtomicU64::new(0);
static NEXT_ROOM_ID: AtomicU64 = AtomicU64::new(0);
pub const MAX_TOOL_ITERATIONS: usize = 8;
const MAX_EVALUATOR_RETRIES: usize = 2;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentRuntimeSnapshot {
    pub state: AgentState,
    pub message_count: usize,
    pub messages: Vec<Message>,
    pub event_count: usize,
    pub events: Vec<EngineEvent>,
    pub last_task: Option<TaskResult<Content>>,
    pub step_count: u64,
}

pub struct AgentRuntime {
    state: AgentState,
    messages: Vec<Message>,
    last_task: Option<TaskResult<Content>>,
    events: Vec<EngineEvent>,
    event_listener: Option<Arc<dyn Fn(EngineEvent) + Send + Sync>>,
    providers: Vec<Arc<dyn Provider>>,
    evaluators: Vec<Arc<dyn Evaluator>>,
    model_adapter: Arc<dyn ModelAdapter>,
    db: Option<Arc<dyn DatabaseAdapter>>,
    persistence_agent_id: Option<String>,
    step_counter: u64,
}

#[derive(Clone, Debug)]
struct PreparedToolStep {
    tool_call: ToolCall,
    step_index: i32,
    idempotency_key: String,
    recovered_result: Option<TaskResult<Content>>,
}

impl AgentRuntime {
    pub fn new(config: AgentConfig, model_adapter: Arc<dyn ModelAdapter>) -> Self {
        let agent_id = next_id("agent", &NEXT_AGENT_ID);
        Self::new_with_id(agent_id, config, model_adapter)
    }

    pub fn new_with_id(
        agent_id: impl Into<String>,
        config: AgentConfig,
        model_adapter: Arc<dyn ModelAdapter>,
    ) -> Self {
        let agent_id = agent_id.into();
        let name = config.name.clone();

        Self {
            state: AgentState {
                id: agent_id,
                name,
                status: AgentStatus::Idle,
                config,
                created_at_ms: now_millis(),
                token_usage: TokenUsage::default(),
            },
            messages: Vec::new(),
            last_task: None,
            events: Vec::new(),
            event_listener: None,
            providers: Vec::new(),
            evaluators: Vec::new(),
            model_adapter,
            db: None,
            persistence_agent_id: None,
            step_counter: 0,
        }
    }

    /// Reconstructs a runtime from a previously captured snapshot.
    ///
    /// **Adapters and listeners are not part of the snapshot** and must be
    /// re-attached by the caller after construction: `set_event_listener`,
    /// `set_database`, `set_persistence_agent_id`, `set_providers`,
    /// `set_evaluators`. This is intentional — only `messages`, `events`,
    /// `state`, `last_task`, and `step_count` round-trip through serialization.
    ///
    /// Note: when a tool result is recovered from persistence on a subsequent
    /// `run()`, fresh `ToolBefore`/`ToolAfter` events are still emitted (with
    /// `recovered: true` on `ToolAfter`). Snapshots taken across replays will
    /// therefore contain duplicate event entries for the same logical step —
    /// idempotency is enforced at the persistence layer, not the event log.
    pub fn from_snapshot(
        snapshot: AgentRuntimeSnapshot,
        model_adapter: Arc<dyn ModelAdapter>,
    ) -> Self {
        Self {
            state: snapshot.state,
            messages: snapshot.messages,
            last_task: snapshot.last_task,
            events: snapshot.events,
            event_listener: None,
            providers: Vec::new(),
            evaluators: Vec::new(),
            model_adapter,
            db: None,
            persistence_agent_id: None,
            step_counter: snapshot.step_count,
        }
    }

    pub fn set_event_listener(&mut self, listener: Arc<dyn Fn(EngineEvent) + Send + Sync>) {
        self.event_listener = Some(listener);
    }

    pub fn set_database(&mut self, db: Arc<dyn DatabaseAdapter>) {
        self.db = Some(db);
    }

    pub fn set_persistence_agent_id(&mut self, agent_id: impl Into<String>) {
        self.persistence_agent_id = Some(agent_id.into());
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

    fn persistence_agent_id(&self) -> &str {
        self.persistence_agent_id
            .as_deref()
            .unwrap_or(&self.state.id)
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
            messages: self.messages.clone(),
            event_count: self.events.len(),
            events: self.events.clone(),
            last_task: self.last_task.clone(),
            step_count: self.step_counter,
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
            created_at_ms: now_millis(),
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
        execute_tool: F,
    ) -> TaskResult<Content>
    where
        F: Fn(AgentState, Message, ToolCall) -> Fut,
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
        let mut evaluator_retries = 0;
        let max_tool_iterations = self
            .state
            .config
            .settings
            .as_ref()
            .and_then(|settings| settings.max_tool_iterations)
            .unwrap_or(MAX_TOOL_ITERATIONS);

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
                            let evaluation = match self
                                .run_evaluators(&user_message, &response.content)
                                .await
                            {
                                Ok(decision) => decision,
                                Err(error) => {
                                    let duration_ms = now_millis().saturating_sub(start);
                                    self.mark_failed(error, duration_ms);
                                    return self.last_task.clone().unwrap_or_else(|| {
                                        TaskResult::error("evaluator execution failed", duration_ms)
                                    });
                                }
                            };

                            match evaluation {
                                EvaluatorDecision::Accept => {
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
                                EvaluatorDecision::Retry { feedback } => {
                                    if evaluator_retries >= MAX_EVALUATOR_RETRIES {
                                        let duration_ms = now_millis().saturating_sub(start);
                                        self.mark_failed(
                                            "evaluator retry limit exceeded",
                                            duration_ms,
                                        );
                                        return self.last_task.clone().unwrap_or_else(|| {
                                            TaskResult::error(
                                                "evaluator retry limit exceeded",
                                                duration_ms,
                                            )
                                        });
                                    }

                                    evaluator_retries += 1;
                                    let assistant_message = self.record_message_in_room(
                                        room_id.clone(),
                                        MessageRole::Assistant,
                                        response.content.clone(),
                                    );
                                    conversation.push(assistant_message);
                                    conversation.push(self.record_message_in_room(
                                        room_id.clone(),
                                        MessageRole::System,
                                        Content {
                                            text: format!(
                                                "Evaluator requested a revision: {feedback}\nRevise your previous answer and try again."
                                            ),
                                            ..Content::default()
                                        },
                                    ));
                                    self.record_token_event();
                                    continue;
                                }
                                EvaluatorDecision::Abort { reason } => {
                                    let duration_ms = now_millis().saturating_sub(start);
                                    self.mark_failed(reason, duration_ms);
                                    return self.last_task.clone().unwrap_or_else(|| {
                                        TaskResult::error("evaluator aborted response", duration_ms)
                                    });
                                }
                            }
                        }
                        ModelStopReason::ToolCall => {
                            if iterations >= max_tool_iterations {
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

                            // Assign step indices by position (not by tool_call.id which may not be unique)
                            let step_indices: Vec<i32> = tool_calls
                                .iter()
                                .enumerate()
                                .map(|(i, _)| {
                                    let idx = (self.step_counter + i as u64) as i32;
                                    idx
                                })
                                .collect();
                            self.step_counter += tool_calls.len() as u64;

                            for tool_call in &tool_calls {
                                self.record_event(
                                    EventType::ToolBefore,
                                    tool_before_event_data(tool_call),
                                );
                            }

                            let prepared_steps = self
                                .prepare_tool_steps(
                                    &user_message,
                                    iterations,
                                    &tool_calls,
                                    &step_indices,
                                )
                                .await;

                            // Write pending steps to database
                            if let Some(db) = self.db.clone() {
                                let persistence_agent_id = self.persistence_agent_id().to_string();
                                for prepared_step in prepared_steps.iter().filter(|prepared_step| {
                                    prepared_step.recovered_result.is_none()
                                }) {
                                    let step = Step {
                                        id: Uuid::new_v4().to_string(),
                                        agent_id: persistence_agent_id.clone(),
                                        step_index: prepared_step.step_index,
                                        idempotency_key: prepared_step.idempotency_key.clone(),
                                        step_type: "tool".to_string(),
                                        status: StepStatus::Pending,
                                        input: Some(tool_step_input_json(&prepared_step.tool_call)),
                                        output: None,
                                    };
                                    if let Err(err) = db.write_step(&step).await {
                                        self.record_event(
                                            EventType::AgentMessage,
                                            DataValue::String(format!(
                                                "failed to persist pending step: step_index={}, error={}",
                                                prepared_step.step_index, err
                                            )),
                                        );
                                    }
                                }
                            }

                            let execute_tool = &execute_tool;
                            let tool_results =
                                join_all(prepared_steps.into_iter().map(|prepared_step| {
                                    let tool_started = now_millis();
                                    let state = self.state.clone();
                                    let user_message = user_message.clone();
                                    async move {
                                        let recovered = prepared_step.recovered_result.is_some();
                                        let tool_result = match prepared_step.recovered_result {
                                            Some(tool_result) => tool_result,
                                            None => {
                                                execute_tool(
                                                    state,
                                                    user_message,
                                                    prepared_step.tool_call.clone(),
                                                )
                                                .await
                                            }
                                        };
                                        let tool_duration = if recovered {
                                            0
                                        } else {
                                            now_millis().saturating_sub(tool_started)
                                        };
                                        (
                                            prepared_step.tool_call,
                                            prepared_step.step_index,
                                            prepared_step.idempotency_key,
                                            recovered,
                                            tool_result,
                                            tool_duration,
                                        )
                                    }
                                }))
                                .await;

                            // Write done/failed steps to database
                            if let Some(db) = self.db.clone() {
                                let persistence_agent_id = self.persistence_agent_id().to_string();
                                for (
                                    tool_call,
                                    step_index,
                                    idempotency_key,
                                    recovered,
                                    tool_result,
                                    _,
                                ) in tool_results.iter()
                                {
                                    if *recovered {
                                        continue;
                                    }
                                    let status = if tool_result.error.is_none() {
                                        StepStatus::Done
                                    } else {
                                        StepStatus::Failed
                                    };
                                    let step = Step {
                                        id: Uuid::new_v4().to_string(),
                                        agent_id: persistence_agent_id.clone(),
                                        step_index: *step_index,
                                        idempotency_key: idempotency_key.clone(),
                                        step_type: "tool".to_string(),
                                        status,
                                        input: Some(tool_step_input_json(tool_call)),
                                        output: Some(tool_step_output_json(tool_result)),
                                    };
                                    if let Err(err) = db.write_step(&step).await {
                                        self.record_event(
                                            EventType::AgentMessage,
                                            DataValue::String(format!(
                                                "failed to persist done/failed step: step_index={}, error={}",
                                                step_index, err
                                            )),
                                        );
                                    }
                                }
                            }

                            for (
                                tool_call,
                                _,
                                idempotency_key,
                                recovered,
                                tool_result,
                                tool_duration,
                            ) in tool_results
                            {
                                if recovered {
                                    self.record_event(
                                        EventType::AgentMessage,
                                        DataValue::String(format!(
                                            "reused persisted tool step: name={}, idempotency_key={}",
                                            tool_call.name, idempotency_key
                                        )),
                                    );
                                }
                                self.record_event(
                                    EventType::ToolAfter,
                                    tool_after_event_data(
                                        &tool_call.name,
                                        tool_result.status.as_str(),
                                        tool_duration,
                                        &tool_result,
                                        recovered,
                                    ),
                                );
                                let tool_message = self.record_message_in_room(
                                    room_id.clone(),
                                    MessageRole::Tool,
                                    content_from_tool_result(&tool_call, tool_result, recovered),
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

    pub fn mark_completed(&mut self, content: Content, duration_ms: u64) {
        self.mark_completed_in_room(next_id("room", &NEXT_ROOM_ID), content, duration_ms);
    }

    fn mark_completed_in_room(&mut self, room_id: String, content: Content, duration_ms: u64) {
        self.state.status = AgentStatus::Completed;
        self.record_message_in_room(room_id, MessageRole::Assistant, content.clone());
        self.last_task = Some(TaskResult::success(content.clone(), duration_ms));
        self.record_event(EventType::AgentCompleted, DataValue::String(content.text));
        self.record_event(EventType::TaskCompleted, DataValue::Null);
    }

    pub fn mark_failed(&mut self, error: impl Into<String>, duration_ms: u64) {
        let error = error.into();
        self.state.status = AgentStatus::Failed;
        self.last_task = Some(TaskResult::error(error.clone(), duration_ms));
        self.record_event(EventType::AgentFailed, DataValue::String(error.clone()));
        self.record_event(EventType::TaskFailed, DataValue::String(error));
    }

    pub fn stop(&mut self) {
        self.state.status = AgentStatus::Terminated;
        self.record_event(EventType::AgentTerminated, DataValue::Null);
    }

    fn record_event(&mut self, event_type: EventType, data: DataValue) {
        let event = EngineEvent {
            id: next_id("event", &NEXT_EVENT_ID),
            event_type,
            agent_id: Some(self.state.id.clone()),
            timestamp_ms: now_millis(),
            data,
        };
        self.events.push(event.clone());
        if let Some(listener) = &self.event_listener {
            listener(event);
        }
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
        let mut providers = self.providers.iter().collect::<Vec<_>>();
        providers.sort_by(|left, right| right.priority().cmp(&left.priority()));

        for provider in providers {
            let result = provider.get(self, message).await?;
            context_parts.push(format!("[{}]: {}", provider.name(), result.text));
        }
        Ok(context_parts)
    }

    async fn run_evaluators(
        &self,
        message: &Message,
        response: &Content,
    ) -> Result<EvaluatorDecision, String> {
        let mut evaluators = self.evaluators.iter().collect::<Vec<_>>();
        evaluators.sort_by(|left, right| right.priority().cmp(&left.priority()));

        for evaluator in evaluators {
            if evaluator.validate(self, message).await? {
                let result = evaluator.evaluate(self, message, response).await?;
                match result.decision {
                    EvaluatorDecision::Accept => {}
                    EvaluatorDecision::Retry { feedback } => {
                        return Ok(EvaluatorDecision::Retry { feedback })
                    }
                    EvaluatorDecision::Abort { reason } => {
                        return Ok(EvaluatorDecision::Abort { reason })
                    }
                }
            }
        }
        Ok(EvaluatorDecision::Accept)
    }

    async fn prepare_tool_steps(
        &mut self,
        user_message: &Message,
        iteration: usize,
        tool_calls: &[ToolCall],
        step_indices: &[i32],
    ) -> Vec<PreparedToolStep> {
        let mut prepared_steps = Vec::with_capacity(tool_calls.len());
        let db = self.db.clone();
        let persistence_agent_id = self.persistence_agent_id().to_string();

        for (i, tool_call) in tool_calls.iter().cloned().enumerate() {
            let idempotency_key = tool_step_idempotency_key(
                &persistence_agent_id,
                user_message,
                iteration,
                i,
                &tool_call,
            );
            let recovered_result = if let Some(db) = db.as_ref() {
                match db
                    .get_step_by_idempotency_key(&persistence_agent_id, &idempotency_key)
                    .await
                {
                    Ok(Some(step)) => match persisted_task_result(&step) {
                        Ok(result) => result,
                        Err(error) => {
                            self.record_event(
                                EventType::AgentMessage,
                                DataValue::String(format!(
                                    "failed to recover persisted step: step_index={}, error={}",
                                    step.step_index, error
                                )),
                            );
                            None
                        }
                    },
                    Ok(None) => None,
                    Err(error) => {
                        self.record_event(
                            EventType::AgentMessage,
                            DataValue::String(format!(
                                "failed to load persisted step: key={}, error={}",
                                idempotency_key, error
                            )),
                        );
                        None
                    }
                }
            } else {
                None
            };

            prepared_steps.push(PreparedToolStep {
                tool_call,
                step_index: step_indices[i],
                idempotency_key,
                recovered_result,
            });
        }

        prepared_steps
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

fn content_from_tool_result(
    tool_call: &ToolCall,
    result: TaskResult<Content>,
    recovered: bool,
) -> Content {
    let mut metadata = BTreeMap::new();
    metadata.insert("toolCallId".into(), DataValue::String(tool_call.id.clone()));
    if recovered {
        metadata.insert("recoveredFromPersistence".into(), DataValue::Bool(true));
    }
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

fn tool_before_event_data(tool_call: &ToolCall) -> DataValue {
    let mut value = BTreeMap::new();
    value.insert("name".into(), DataValue::String(tool_call.name.clone()));
    value.insert("args".into(), DataValue::Object(tool_call.args.clone()));
    value.insert("status".into(), DataValue::String("running".to_string()));
    value.insert("durationMs".into(), DataValue::Number(0.0));
    DataValue::Object(value)
}

fn tool_after_event_data(
    name: &str,
    status: &str,
    duration_ms: u64,
    result: &TaskResult<Content>,
    recovered: bool,
) -> DataValue {
    let mut value = BTreeMap::new();
    value.insert("name".into(), DataValue::String(name.to_string()));
    value.insert("status".into(), DataValue::String(status.to_string()));
    value.insert("durationMs".into(), DataValue::Number(duration_ms as f64));
    value.insert("recovered".into(), DataValue::Bool(recovered));
    value.insert("result".into(), tool_result_text_data_value(result));
    DataValue::Object(value)
}

fn tool_step_idempotency_key(
    agent_id: &str,
    message: &Message,
    iteration: usize,
    tool_position: usize,
    tool_call: &ToolCall,
) -> String {
    let step_seed = format!(
        "{}\n{}\n{}\n{}",
        iteration,
        tool_position,
        tool_call.name,
        data_value_json(&DataValue::Object(tool_call.args.clone())),
    );

    if let Some(retry_key) = message_retry_key(message) {
        let seed = format!("{}\n{}\n{}", agent_id, retry_key, step_seed);
        return Uuid::new_v5(&Uuid::NAMESPACE_OID, seed.as_bytes()).to_string();
    }

    let seed = format!(
        "{}\n{}\n{}\n{}",
        agent_id, message.id, message.room_id, step_seed,
    );
    Uuid::new_v5(&Uuid::NAMESPACE_OID, seed.as_bytes()).to_string()
}

fn message_retry_key(message: &Message) -> Option<&str> {
    let metadata = message.content.metadata.as_ref()?;
    ["retryKey", "retry_key", "idempotencyKey", "idempotency_key"]
        .iter()
        .find_map(|key| match metadata.get(*key) {
            Some(DataValue::String(value)) if !value.is_empty() => Some(value.as_str()),
            _ => None,
        })
}

fn next_id(prefix: &str, counter: &AtomicU64) -> String {
    let next = counter.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{}-{next}", now_millis())
}

#[cfg(test)]
#[path = "runtime/tests.rs"]
mod tests;
