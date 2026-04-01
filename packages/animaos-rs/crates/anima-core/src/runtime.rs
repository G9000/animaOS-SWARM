use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::agent::{AgentConfig, AgentState, AgentStatus, TokenUsage};
use crate::events::{EngineEvent, EventType};
use crate::primitives::{Content, DataValue, Message, MessageRole, TaskResult};

static NEXT_AGENT_ID: AtomicU64 = AtomicU64::new(0);
static NEXT_MESSAGE_ID: AtomicU64 = AtomicU64::new(0);
static NEXT_ROOM_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq)]
pub struct AgentRuntimeSnapshot {
    pub state: AgentState,
    pub message_count: usize,
    pub event_count: usize,
    pub last_task: Option<TaskResult<Content>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentRuntime {
    state: AgentState,
    messages: Vec<Message>,
    last_task: Option<TaskResult<Content>>,
    events: Vec<EngineEvent>,
}

impl AgentRuntime {
    pub fn new(config: AgentConfig) -> Self {
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

    pub fn run(&mut self, input: Content) -> TaskResult<Content> {
        let start = now_millis();
        let room_id = next_id("room", &NEXT_ROOM_ID);
        self.mark_running();
        self.record_message_in_room(room_id.clone(), MessageRole::User, input.clone());

        let output = Content {
            text: format!("{} handled task: {}", self.state.name, input.text),
            attachments: None,
            metadata: None,
        };

        let duration_ms = now_millis().saturating_sub(start);
        self.mark_completed_in_room(room_id, output.clone(), duration_ms);
        self.last_task
            .clone()
            .unwrap_or_else(|| TaskResult::success(output, duration_ms))
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
    use super::AgentRuntime;
    use crate::agent::{AgentConfig, AgentStatus};
    use crate::primitives::{Content, MessageRole, TaskStatus};

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

    #[test]
    fn runtime_tracks_lifecycle_state() {
        let mut runtime = AgentRuntime::new(config());
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
        let mut runtime = AgentRuntime::new(config());
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
        let mut runtime = AgentRuntime::new(config());
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
        let mut runtime = AgentRuntime::new(config());
        runtime.init();

        let result = runtime.run(Content {
            text: "Inspect memory state".into(),
            ..Content::default()
        });
        let snapshot = runtime.snapshot();

        assert_eq!(result.status, TaskStatus::Success);
        assert_eq!(
            result.data.as_ref().map(|content| content.text.as_str()),
            Some("researcher handled task: Inspect memory state")
        );
        assert_eq!(snapshot.state.status, AgentStatus::Completed);
        assert_eq!(snapshot.message_count, 2);
        assert_eq!(snapshot.event_count, 7);
    }

    #[test]
    fn runtime_run_reuses_one_room_for_conversation_messages() {
        let mut runtime = AgentRuntime::new(config());
        runtime.init();

        runtime.run(Content {
            text: "Keep one room".into(),
            ..Content::default()
        });

        assert_eq!(runtime.messages().len(), 2);
        assert_eq!(runtime.messages()[0].room_id, runtime.messages()[1].room_id);
    }
}
