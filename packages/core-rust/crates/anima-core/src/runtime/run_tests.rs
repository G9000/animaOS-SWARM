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
    assert!(copy.messages.iter().all(|message| message.room_id == "room-b"));
    assert!(copy.events.is_empty(), "a run copy never carries the event log");
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
        [(MessageRole::User, "room-b"), (MessageRole::Assistant, "room-b")]
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
    assert_eq!(&applied.messages[..before.messages.len()], &before.messages[..]);
    assert_eq!(&applied.messages[before.messages.len()..], &delta.messages[..]);
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
    assert_eq!(merged.event_count, before.event_count + second_delta.event_total);
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
