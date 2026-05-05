use std::collections::BTreeMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use futures::future::join_all;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::agent::{AgentConfig, AgentState, AgentStatus, TokenUsage};
use crate::components::{Evaluator, EvaluatorDecision, Provider};
use crate::events::{EngineEvent, EventType};
use crate::model::{ModelAdapter, ModelGenerateRequest, ModelStopReason, ToolCall};
use crate::persistence::{DatabaseAdapter, Step, StepStatus};
use crate::primitives::{
    Attachment, AttachmentType, Content, DataValue, Message, MessageRole, TaskResult, TaskStatus,
};

static NEXT_AGENT_ID: AtomicU64 = AtomicU64::new(0);
static NEXT_EVENT_ID: AtomicU64 = AtomicU64::new(0);
static NEXT_MESSAGE_ID: AtomicU64 = AtomicU64::new(0);
static NEXT_ROOM_ID: AtomicU64 = AtomicU64::new(0);
const MAX_TOOL_ITERATIONS: usize = 8;
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
            timestamp_ms: now_millis_u64(),
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
    duration_ms: u128,
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

fn tool_step_input_json(tool_call: &ToolCall) -> serde_json::Value {
    serde_json::json!({
        "name": tool_call.name,
        "args": data_value_to_json(&DataValue::Object(tool_call.args.clone())),
    })
}

fn tool_step_output_json(result: &TaskResult<Content>) -> serde_json::Value {
    data_value_to_json(&task_result_data_value(result))
}

fn persisted_task_result(step: &Step) -> Result<Option<TaskResult<Content>>, String> {
    match step.status {
        StepStatus::Pending => Ok(None),
        StepStatus::Done | StepStatus::Failed => {
            let output = step.output.as_ref().ok_or_else(|| {
                format!(
                    "persisted step {} has terminal status without output",
                    step.id
                )
            })?;
            task_result_from_json(output)
                .ok_or_else(|| format!("persisted step {} has unreadable output", step.id))
                .map(Some)
        }
    }
}

fn task_result_from_json(value: &serde_json::Value) -> Option<TaskResult<Content>> {
    let object = value.as_object()?;
    let status = match object.get("status")?.as_str()? {
        "success" => TaskStatus::Success,
        "error" => TaskStatus::Error,
        _ => return None,
    };
    let data = match object.get("data") {
        Some(serde_json::Value::Null) | None => None,
        Some(content) => content_from_json(content),
    };
    let error = object
        .get("error")
        .and_then(|error| error.as_str().map(ToOwned::to_owned));
    let duration_ms = object.get("durationMs").and_then(json_u128).unwrap_or(0);

    Some(TaskResult {
        status,
        data,
        error,
        duration_ms,
    })
}

fn content_from_json(value: &serde_json::Value) -> Option<Content> {
    match value {
        serde_json::Value::Null => None,
        serde_json::Value::String(text) => Some(Content {
            text: text.clone(),
            ..Content::default()
        }),
        serde_json::Value::Object(object) => Some(Content {
            text: object
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            attachments: object.get("attachments").and_then(attachments_from_json),
            metadata: object.get("metadata").and_then(metadata_from_json),
        }),
        _ => None,
    }
}

fn attachments_from_json(value: &serde_json::Value) -> Option<Vec<Attachment>> {
    let values = value.as_array()?;
    let attachments = values
        .iter()
        .filter_map(attachment_from_json)
        .collect::<Vec<_>>();

    if attachments.is_empty() {
        None
    } else {
        Some(attachments)
    }
}

fn attachment_from_json(value: &serde_json::Value) -> Option<Attachment> {
    let object = value.as_object()?;
    let attachment_type = attachment_type_from_str(object.get("type")?.as_str()?)?;
    let name = object.get("name")?.as_str()?.to_string();
    let data = object.get("data")?.as_str()?.to_string();

    Some(Attachment {
        attachment_type,
        name,
        data,
    })
}

fn attachment_type_from_str(value: &str) -> Option<AttachmentType> {
    match value {
        "file" => Some(AttachmentType::File),
        "image" => Some(AttachmentType::Image),
        "url" => Some(AttachmentType::Url),
        _ => None,
    }
}

fn metadata_from_json(value: &serde_json::Value) -> Option<BTreeMap<String, DataValue>> {
    match json_to_data_value(value) {
        Some(DataValue::Object(metadata)) => Some(metadata),
        _ => None,
    }
}

fn json_to_data_value(value: &serde_json::Value) -> Option<DataValue> {
    match value {
        serde_json::Value::Null => Some(DataValue::Null),
        serde_json::Value::Bool(value) => Some(DataValue::Bool(*value)),
        serde_json::Value::Number(value) => value.as_f64().map(DataValue::Number),
        serde_json::Value::String(value) => Some(DataValue::String(value.clone())),
        serde_json::Value::Array(values) => values
            .iter()
            .map(json_to_data_value)
            .collect::<Option<Vec<_>>>()
            .map(DataValue::Array),
        serde_json::Value::Object(values) => values
            .iter()
            .map(|(key, value)| json_to_data_value(value).map(|value| (key.clone(), value)))
            .collect::<Option<BTreeMap<_, _>>>()
            .map(DataValue::Object),
    }
}

fn json_u128(value: &serde_json::Value) -> Option<u128> {
    value.as_u64().map(u128::from).or_else(|| {
        value
            .as_f64()
            .filter(|value| *value >= 0.0)
            .map(|value| value as u128)
    })
}

fn tool_result_text_data_value(result: &TaskResult<Content>) -> DataValue {
    match result.status {
        TaskStatus::Success => match result.data.as_ref() {
            Some(content) => DataValue::String(content.text.clone()),
            None => DataValue::Null,
        },
        TaskStatus::Error => match result.error.as_ref() {
            Some(error) => DataValue::String(error.clone()),
            None => DataValue::Null,
        },
    }
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

fn now_millis_u64() -> u64 {
    u64::try_from(now_millis()).unwrap_or(u64::MAX)
}

fn next_id(prefix: &str, counter: &AtomicU64) -> String {
    let next = counter.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{}-{next}", now_millis())
}

fn data_value_to_json(value: &DataValue) -> serde_json::Value {
    match value {
        DataValue::Null => serde_json::Value::Null,
        DataValue::Bool(v) => serde_json::json!(v),
        DataValue::Number(v) => serde_json::json!(v),
        DataValue::String(v) => serde_json::json!(v),
        DataValue::Array(vs) => {
            serde_json::Value::Array(vs.iter().map(data_value_to_json).collect())
        }
        DataValue::Object(map) => serde_json::Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), data_value_to_json(v)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{data_value_json, AgentRuntime};
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
}
