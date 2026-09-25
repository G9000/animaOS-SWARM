//! Isolated per-run copies, run deltas, and run-scoped step keys.

use super::{new_room_id, AgentRuntime, MAX_RETAINED_EVENTS};
use crate::agent::{AgentConfig, AgentStatus, TokenUsage};
use crate::events::EventType;
use crate::model::{ModelAdapter, ModelGenerateRequest, ModelGenerateResponse, ModelStopReason};
use crate::primitives::{Content, DataValue, MessageRole, TaskStatus};
use async_trait::async_trait;
use futures::executor::block_on;
use std::sync::Arc;

struct EchoModel;

#[async_trait]
impl ModelAdapter for EchoModel {
    fn provider(&self) -> &str {
        "echo"
    }

    async fn generate(
        &self,
        _config: &AgentConfig,
        request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String> {
        let input = request
            .messages
            .last()
            .map(|message| message.content.text.clone())
            .unwrap_or_default();
        Ok(ModelGenerateResponse {
            content: text(&format!("echo: {input}")),
            tool_calls: None,
            usage: usage(),
            stop_reason: ModelStopReason::End,
        })
    }
}

fn usage() -> TokenUsage {
    TokenUsage {
        prompt_tokens: 5,
        completion_tokens: 7,
        total_tokens: 12,
        cached_prompt_tokens: 2,
        reasoning_tokens: 3,
    }
}

fn config() -> AgentConfig {
    AgentConfig {
        name: "isolated".into(),
        model: "echo-model".into(),
        bio: None,
        lore: None,
        knowledge: None,
        topics: None,
        adjectives: None,
        style: None,
        provider: None,
        system: None,
        tools: None,
        plugins: None,
        settings: None,
    }
}

fn text(value: &str) -> Content {
    Content {
        text: value.into(),
        ..Content::default()
    }
}

fn canonical() -> AgentRuntime {
    let mut runtime = AgentRuntime::new(config(), Arc::new(EchoModel));
    runtime.init();
    runtime
}

fn room_history(runtime: &AgentRuntime, room_id: &str) -> Vec<crate::primitives::Message> {
    runtime
        .messages()
        .iter()
        .filter(|message| message.room_id == room_id)
        .cloned()
        .collect()
}

fn isolated_copy(canonical: &AgentRuntime, room_id: &str) -> AgentRuntime {
    AgentRuntime::from_snapshot(
        canonical.run_snapshot(room_history(canonical, room_id)),
        Arc::new(EchoModel),
    )
}

fn run_in(
    runtime: &mut AgentRuntime,
    room_id: &str,
    input: &str,
) -> crate::primitives::TaskResult<Content> {
    let history = room_history(runtime, room_id);
    block_on(runtime.run_in_room_with_context(room_id.into(), history, text(input)))
}

#[test]
fn isolated_copy_carries_only_its_room_and_no_events() {
    let mut canonical = canonical();
    run_in(&mut canonical, "room-a", "in a");
    run_in(&mut canonical, "room-b", "in b");

    let copy = canonical.run_snapshot(room_history(&canonical, "room-b"));
    let full = canonical.snapshot();

    assert_eq!(copy.messages.len(), 2);
    assert_eq!(copy.message_count, 2);
    assert!(copy
        .messages
        .iter()
        .all(|message| message.room_id == "room-b"));
    assert!(
        copy.events.is_empty(),
        "a run copy never carries the event log"
    );
    assert_eq!(copy.event_count, full.event_count);
    assert_eq!(copy.step_count, full.step_count);
    assert_eq!(copy.state, full.state);
    assert_eq!(copy.last_task, full.last_task);
}

#[test]
fn applying_a_delta_merges_exactly_one_run_and_reverting_restores_the_record() {
    let mut canonical = canonical();
    run_in(&mut canonical, "room-a", "first");
    let before = canonical.snapshot();

    let mut isolated = isolated_copy(&canonical, "room-b");
    let base = isolated.run_base();
    let result = run_in(&mut isolated, "room-b", "second");
    let delta = isolated.run_delta_since(&base);

    assert_eq!(result.status, TaskStatus::Success);
    assert_eq!(
        delta
            .messages
            .iter()
            .map(|message| (message.role, message.room_id.as_str()))
            .collect::<Vec<_>>(),
        [
            (MessageRole::User, "room-b"),
            (MessageRole::Assistant, "room-b")
        ]
    );
    assert_eq!(delta.token_usage, usage());
    assert_eq!(delta.status, AgentStatus::Completed);
    assert_eq!(delta.last_task, Some(result));
    assert!(delta.event_total > 0);
    assert_eq!(delta.event_total, delta.events.len());
    assert_eq!(
        canonical.snapshot(),
        before,
        "building and running a copy never touches the canonical record"
    );

    let undo = canonical.apply_run_delta(&delta);
    let applied = canonical.snapshot();
    assert_eq!(
        &applied.messages[..before.messages.len()],
        &before.messages[..]
    );
    assert_eq!(
        &applied.messages[before.messages.len()..],
        &delta.messages[..]
    );
    assert_eq!(
        applied.state.token_usage.total_tokens,
        before.state.token_usage.total_tokens + 12
    );
    assert_eq!(
        applied.state.token_usage.cached_prompt_tokens,
        before.state.token_usage.cached_prompt_tokens + 2
    );
    assert_eq!(
        applied.state.token_usage.reasoning_tokens,
        before.state.token_usage.reasoning_tokens + 3
    );
    assert_eq!(applied.event_count, before.event_count + delta.event_total);
    assert_eq!(applied.step_count, before.step_count + delta.step_count);
    assert_eq!(applied.last_task, delta.last_task);
    assert_eq!(applied.state.status, AgentStatus::Completed);

    canonical.revert_run_delta(&delta, undo);
    assert_eq!(canonical.snapshot(), before);
}

#[test]
fn copies_from_one_base_merge_independently_and_revert_by_id() {
    let mut canonical = canonical();
    let before = canonical.snapshot();
    let mut first = isolated_copy(&canonical, "room-a");
    let mut second = isolated_copy(&canonical, "room-b");
    let (first_base, second_base) = (first.run_base(), second.run_base());
    run_in(&mut first, "room-a", "from a");
    run_in(&mut second, "room-b", "from b");
    let first_delta = first.run_delta_since(&first_base);
    let second_delta = second.run_delta_since(&second_base);

    let first_undo = canonical.apply_run_delta(&first_delta);
    canonical.apply_run_delta(&second_delta);
    canonical.revert_run_delta(&first_delta, first_undo);

    let merged = canonical.snapshot();
    assert_eq!(merged.messages, second_delta.messages);
    assert_eq!(merged.state.token_usage, usage());
    assert_eq!(
        merged.event_count,
        before.event_count + second_delta.event_total
    );
    assert_eq!(
        merged.last_task, second_delta.last_task,
        "reverting an earlier run keeps a later run's result"
    );
    assert_eq!(merged.state.status, AgentStatus::Completed);
}

#[test]
fn applying_a_delta_keeps_the_retained_event_cap() {
    let mut canonical = canonical();
    for _ in 0..MAX_RETAINED_EVENTS {
        canonical.record_event(EventType::AgentTokens, DataValue::Null);
    }
    let before_total = canonical.snapshot().event_count;
    let mut isolated = isolated_copy(&canonical, "room-a");
    let base = isolated.run_base();
    for _ in 0..100 {
        isolated.record_event(EventType::AgentTokens, DataValue::Null);
    }
    let delta = isolated.run_delta_since(&base);

    assert_eq!(delta.event_total, 100);
    canonical.apply_run_delta(&delta);
    let applied = canonical.snapshot();
    assert_eq!(applied.events.len(), MAX_RETAINED_EVENTS);
    assert_eq!(applied.event_count, before_total + 100);
    assert_eq!(
        applied.events.last().map(|event| &event.id),
        delta.events.last().map(|event| &event.id)
    );
}

#[test]
fn new_room_ids_are_unique_generated_rooms() {
    let first = new_room_id();
    let second = new_room_id();

    assert_ne!(first, second);
    assert!(first.starts_with("room-") && second.starts_with("room-"));
}

use crate::agent::{AgentState, ToolDescriptor};
use crate::model::ToolCall;
use crate::persistence::{in_memory::InMemoryAdapter, StepStatus};
use crate::primitives::{Message, TaskResult};
use std::collections::BTreeMap;

/// Asks for `memory_search` once, then answers.
struct SearchOnceModel;

#[async_trait]
impl ModelAdapter for SearchOnceModel {
    fn provider(&self) -> &str {
        "search-once"
    }

    async fn generate(
        &self,
        _config: &AgentConfig,
        request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String> {
        if request
            .messages
            .iter()
            .any(|message| message.role == MessageRole::Tool)
        {
            return Ok(ModelGenerateResponse {
                content: text("done"),
                tool_calls: None,
                usage: TokenUsage::default(),
                stop_reason: ModelStopReason::End,
            });
        }
        Ok(ModelGenerateResponse {
            content: Content::default(),
            tool_calls: Some(vec![ToolCall {
                id: "search-1".into(),
                name: "memory_search".into(),
                args: BTreeMap::new(),
            }]),
            usage: TokenUsage::default(),
            stop_reason: ModelStopReason::ToolCall,
        })
    }
}

fn search_config() -> AgentConfig {
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

#[test]
fn step_keys_include_the_run_id_unless_a_durable_retry_key_is_present() {
    let message = Message {
        id: "msg-1".into(),
        agent_id: "agent-1".into(),
        room_id: "room-a".into(),
        content: text("search"),
        role: MessageRole::User,
        created_at_ms: 1,
    };
    let call = ToolCall {
        id: "search-1".into(),
        name: "memory_search".into(),
        args: BTreeMap::new(),
    };
    let key = |run_id: Option<&str>, message: &Message| {
        super::tool_step_idempotency_key("agent-1", run_id, message, 1, 0, &call)
    };

    assert_ne!(key(Some("run_a"), &message), key(Some("run_b"), &message));
    assert_ne!(key(Some("run_a"), &message), key(None, &message));

    let mut keyed = message.clone();
    keyed.content.metadata = Some(BTreeMap::from([(
        "idempotencyKey".to_string(),
        DataValue::String("telegram-a:update:42".into()),
    )]));
    assert_eq!(
        key(Some("run_a"), &keyed),
        key(Some("run_b"), &keyed),
        "a durable retry key keeps tool steps replay-safe when a restart re-runs the work under a new run"
    );
    assert_eq!(key(Some("run_a"), &keyed), key(None, &keyed));
}

/// Proves that two isolated copies built from one canonical snapshot, run
/// concurrently, each persist their own completed tool step and neither
/// overwrites the other's row in the step log, even though both copies start
/// from the same step index. It does not pin the run-id key component: here
/// `message.id`/`message.room_id` already differ per iteration (each mints
/// fresh ids off the process-wide counters), so the keys would stay distinct
/// even without `run_id` in the seed. That component is covered by
/// `step_keys_include_the_run_id_unless_a_durable_retry_key_is_present`.
#[test]
fn copies_running_concurrently_persist_separate_tool_steps() {
    let db = Arc::new(InMemoryAdapter::new());
    let mut canonical = AgentRuntime::new(search_config(), Arc::new(SearchOnceModel));
    canonical.init();
    let tool =
        |_: AgentState, _: Message, _: ToolCall| async move { TaskResult::success(text("hit"), 1) };
    let mut steps_per_run = Vec::new();
    for run_id in ["run_a", "run_b"] {
        let mut copy = AgentRuntime::from_snapshot(
            canonical.run_snapshot(Vec::new()),
            Arc::new(SearchOnceModel),
        );
        copy.set_database(db.clone());
        copy.set_run_id(run_id);
        assert_eq!(copy.run_id(), Some(run_id));
        let base = copy.run_base();
        let result = block_on(copy.run_in_room_with_context_and_tools(
            new_room_id(),
            Vec::new(),
            text("search"),
            tool,
        ));
        assert_eq!(result.status, TaskStatus::Success);
        steps_per_run.push(copy.run_delta_since(&base).step_count);
    }

    let steps = db.recorded_steps();
    assert_eq!(steps_per_run, [1, 1]);
    assert_eq!(
        steps.len(),
        2,
        "copies share a starting step index but keep separate step rows"
    );
    assert_eq!(steps[0].step_index, steps[1].step_index);
    assert_ne!(steps[0].idempotency_key, steps[1].idempotency_key);
    assert!(steps.iter().all(|step| step.status == StepStatus::Done));
}

#[test]
fn content_retry_key_reads_the_supported_metadata_names() {
    for name in ["retryKey", "retry_key", "idempotencyKey", "idempotency_key"] {
        let content = Content {
            metadata: Some(BTreeMap::from([(
                name.to_string(),
                DataValue::String("key-1".into()),
            )])),
            ..Content::default()
        };
        assert_eq!(super::content_retry_key(&content), Some("key-1"), "{name}");
    }
    let blank = Content {
        metadata: Some(BTreeMap::from([(
            "idempotencyKey".to_string(),
            DataValue::String(String::new()),
        )])),
        ..Content::default()
    };
    assert_eq!(super::content_retry_key(&blank), None);
    assert_eq!(super::content_retry_key(&Content::default()), None);
}

#[test]
fn retain_messages_removes_only_rejected_messages_and_keeps_counters() {
    let mut canonical = canonical();
    run_in(&mut canonical, "room-a", "first");
    run_in(&mut canonical, "room-b", "second");
    let before = canonical.snapshot();

    let removed = canonical.retain_messages(|message| message.room_id != "room-a");

    assert_eq!(removed.len(), 2);
    assert!(removed.iter().all(|message| message.room_id == "room-a"));
    assert_eq!(removed[0].role, MessageRole::User);
    assert_eq!(removed[1].role, MessageRole::Assistant);
    let after = canonical.snapshot();
    assert_eq!(after.messages.len(), 2);
    assert!(after
        .messages
        .iter()
        .all(|message| message.room_id == "room-b"));
    assert_eq!(after.message_count, 2);
    assert_eq!(after.event_count, before.event_count);
    assert_eq!(after.events, before.events);
    assert_eq!(after.step_count, before.step_count);
    assert_eq!(after.state.token_usage, before.state.token_usage);
    assert_eq!(after.state.status, before.state.status);
    assert_eq!(after.last_task, before.last_task);
    assert!(
        canonical.retain_messages(|_| true).is_empty(),
        "keeping everything removes nothing"
    );
}

#[test]
fn retain_messages_leaves_no_room_for_the_removed_messages() {
    let mut canonical = canonical();
    let status = canonical.state().status;
    canonical.apply_run_delta(&super::RuntimeRunDelta {
        messages: (0..1_000)
            .map(|index| Message {
                id: format!("msg-{index}"),
                agent_id: "agent-1".into(),
                room_id: if index < 990 { "room-old" } else { "room-kept" }.into(),
                content: text("hello"),
                role: MessageRole::User,
                created_at_ms: index,
            })
            .collect(),
        events: Vec::new(),
        event_total: 0,
        token_usage: TokenUsage::default(),
        step_count: 0,
        last_task: None,
        status,
    });

    let removed = canonical.retain_messages(|message| message.room_id == "room-kept");

    assert_eq!(removed.len(), 990);
    assert_eq!(canonical.messages().len(), 10);
    assert!(
        canonical.messages.capacity() <= 2 * canonical.messages.len(),
        "the transcript keeps room for {} messages after keeping 10",
        canonical.messages.capacity()
    );
}
