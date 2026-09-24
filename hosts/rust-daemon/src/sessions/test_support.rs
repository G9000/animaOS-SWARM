//! Helpers shared by the session, route, and pruning tests.

use std::collections::BTreeMap;

use anima_core::{
    AgentConfig, AgentSettings, Content, DataValue, Message, MessageRole, RuntimeRunDelta,
    TokenUsage,
};

use crate::state::DaemonState;

pub(crate) fn agent_config(name: &str) -> AgentConfig {
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
        tools: None,
        plugins: None,
        settings: Some(AgentSettings::default()),
    }
}

pub(crate) fn message(
    agent_id: &str,
    id: &str,
    room_id: &str,
    role: MessageRole,
    text: &str,
    created_at_ms: u64,
) -> Message {
    Message {
        id: id.into(),
        agent_id: agent_id.into(),
        room_id: room_id.into(),
        content: Content {
            text: text.into(),
            ..Content::default()
        },
        role,
        created_at_ms,
    }
}

/// A scheduler check-in prompt for `schedule_id`.
pub(crate) fn checkin_prompt(
    agent_id: &str,
    id: &str,
    room_id: &str,
    schedule_id: &str,
    prompt: &str,
    created_at_ms: u64,
) -> Message {
    let mut checkin = message(
        agent_id,
        id,
        room_id,
        MessageRole::User,
        &crate::schedules::wrap_checkin_prompt(prompt),
        created_at_ms,
    );
    checkin.content.metadata = Some(BTreeMap::from([
        ("kind".to_string(), DataValue::String("checkin".into())),
        ("id".to_string(), DataValue::String(schedule_id.into())),
    ]));
    checkin
}

/// Awaits `future`, failing the test after five seconds instead of hanging
/// the test binary when a regression never reaches a gated call.
pub(crate) async fn within<T>(what: &str, future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(std::time::Duration::from_secs(5), future)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

/// Appends `messages` to the agent's canonical transcript, as a commit would.
pub(crate) fn seed_messages(state: &mut DaemonState, agent_id: &str, messages: Vec<Message>) {
    let runtime = state
        .agents
        .get_mut(agent_id)
        .expect("the seeded agent exists");
    let status = runtime.state().status;
    runtime.apply_run_delta(&RuntimeRunDelta {
        messages,
        events: Vec::new(),
        event_total: 0,
        token_usage: TokenUsage::default(),
        step_count: 0,
        last_task: None,
        status,
    });
}
