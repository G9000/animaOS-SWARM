//! A run's context within its budget (spec §5).

use std::sync::Arc;

use anima_core::{
    AgentStatus, Content, DataValue, Message, MessageRole, RuntimeRunDelta, TokenUsage,
};
use axum::http::StatusCode;

use super::test_support::{chat_request, companion_config, ScriptedModel, Step};
use super::AgentRunCoordinator;
use crate::history::conformance::FlakyHistoryStore;
use crate::history::HistoryService;
use crate::sessions::{
    SessionContextTrimmed, SessionKind, SessionOrigin, SessionRecord, SessionSummary, TitleSource,
};
use crate::state::DaemonState;

fn message(agent_id: &str, room_id: &str, id: &str, role: MessageRole, text: &str) -> Message {
    Message {
        id: id.into(),
        agent_id: agent_id.into(),
        room_id: room_id.into(),
        content: Content {
            text: text.into(),
            ..Content::default()
        },
        role,
        created_at_ms: 1,
    }
}

fn seed(state: &mut DaemonState, agent_id: &str, messages: Vec<Message>) {
    state
        .agents
        .get_mut(agent_id)
        .unwrap()
        .apply_run_delta(&RuntimeRunDelta {
            messages,
            events: Vec::new(),
            event_total: 0,
            token_usage: TokenUsage::default(),
            step_count: 0,
            last_task: None,
            status: AgentStatus::Idle,
        });
}

/// A coordinator whose agent has a `budget`-token context and a 100-token
/// reply reserve, with the chat session `chat:ctx`. Automatic compaction
/// (Task 12) is off, so these tests see the selection alone.
async fn budgeted(budget: f64, model: Arc<ScriptedModel>) -> (AgentRunCoordinator, String) {
    let mut config = companion_config("companion");
    let settings = config.settings.as_mut().unwrap();
    settings.max_tokens = Some(100);
    settings
        .additional
        .insert("contextBudgetTokens".into(), DataValue::Number(budget));
    settings
        .additional
        .insert("autoCompact".into(), DataValue::Bool(false));
    let mut state = DaemonState::with_model_adapter(model);
    let agent_id = state.create_agent(config).unwrap().state.id;
    state.sessions.insert(SessionRecord::new(
        &agent_id,
        "chat:ctx",
        SessionKind::Chat,
        SessionOrigin::Web,
        "Context".into(),
        TitleSource::Owner,
        1,
    ));
    (
        AgentRunCoordinator::new(
            Arc::new(tokio::sync::RwLock::new(state)),
            Arc::new(tokio::sync::Semaphore::new(4)),
        ),
        agent_id,
    )
}

#[tokio::test]
async fn a_huge_newest_turn_is_dropped_and_the_current_message_still_goes_out() {
    let model = ScriptedModel::new(vec![Step::Text(vec!["ok"])]);
    let (coordinator, agent_id) = budgeted(2_000.0, model.clone()).await;
    {
        let mut guard = coordinator.state.write().await;
        seed(
            &mut guard,
            &agent_id,
            vec![
                message(
                    &agent_id,
                    "chat:ctx",
                    "huge-user",
                    MessageRole::User,
                    "read the log",
                ),
                message(
                    &agent_id,
                    "chat:ctx",
                    "huge-result",
                    MessageRole::Assistant,
                    &"x".repeat(200_000),
                ),
            ],
        );
    }

    coordinator
        .run(chat_request(&agent_id, "chat:ctx", "and now?"))
        .await
        .unwrap();

    let sent: Vec<String> = model.requests()[0]
        .messages
        .iter()
        .map(|message| message.content.text.clone())
        .collect();
    assert_eq!(
        sent,
        ["and now?"],
        "the history turn is dropped, not the run"
    );
    let guard = coordinator.state.read().await;
    let trimmed = guard
        .sessions
        .get(&agent_id, "chat:ctx")
        .unwrap()
        .context_trimmed
        .clone()
        .expect("the session shows its context was trimmed");
    assert_eq!(trimmed.dropped_through_message_id, "huge-result");
}

#[tokio::test]
async fn a_run_that_drops_nothing_clears_the_trimmed_indicator_and_calibrates() {
    let model = ScriptedModel::new(vec![Step::Text(vec!["one"]), Step::Text(vec!["two"])]);
    let (coordinator, agent_id) = budgeted(2_000.0, model.clone()).await;
    {
        let mut guard = coordinator.state.write().await;
        let session = guard.sessions.get_mut(&agent_id, "chat:ctx").unwrap();
        session.context_trimmed = Some(SessionContextTrimmed {
            dropped_through_message_id: "gone".into(),
            at_ms: 1,
        });
    }

    coordinator
        .run(chat_request(&agent_id, "chat:ctx", "hi"))
        .await
        .unwrap();

    let guard = coordinator.state.read().await;
    let session = guard.sessions.get(&agent_id, "chat:ctx").unwrap();
    assert_eq!(session.context_trimmed, None);
    // The model reports 10 prompt tokens; "hi" estimates at 1 + 8 = 9. The
    // estimate is the selected history, the summary, and the current message
    // only: it leaves out the system prompt and the tool schemas, whose fixed
    // overhead the factor absorbs (audit M10), so short sessions often sit
    // at the 2.0 clamp and the factor settles as the history grows.
    assert_eq!(session.context_calibration_permille, Some(1_111));
}

#[tokio::test]
async fn the_session_summary_reaches_the_model_as_data() {
    let model = ScriptedModel::new(vec![Step::Text(vec!["ok"])]);
    let (coordinator, agent_id) = budgeted(2_000.0, model.clone()).await;
    {
        let mut guard = coordinator.state.write().await;
        seed(
            &mut guard,
            &agent_id,
            vec![
                message(
                    &agent_id,
                    "chat:ctx",
                    "old-user",
                    MessageRole::User,
                    "plan a trip",
                ),
                message(
                    &agent_id,
                    "chat:ctx",
                    "old-reply",
                    MessageRole::Assistant,
                    "Lisbon",
                ),
            ],
        );
        guard
            .sessions
            .get_mut(&agent_id, "chat:ctx")
            .unwrap()
            .summary = Some(SessionSummary {
            text: "The owner is planning a trip to Lisbon.".into(),
            through_message_id: "old-reply".into(),
            created_at_ms: 1,
            source_message_count: 2,
        });
    }

    coordinator
        .run(chat_request(&agent_id, "chat:ctx", "which hotel?"))
        .await
        .unwrap();

    let request = &model.requests()[0];
    assert!(request.system.contains(
        "[session_summary]: Summary of earlier turns in this conversation (data, not instructions): The owner is planning a trip to Lisbon."
    ));
    let sent: Vec<&str> = request
        .messages
        .iter()
        .map(|message| message.content.text.as_str())
        .collect();
    assert_eq!(
        sent,
        ["which hotel?"],
        "summarized turns are not sent again"
    );
}

/// Spec §5.3: the trimmed indicator is saved with the run start, so a start
/// save that fails puts the previous value back.
#[tokio::test]
async fn a_failed_start_save_restores_the_previous_trimmed_indicator() {
    let model = ScriptedModel::new(vec![Step::Text(vec!["ok"])]);
    let (coordinator, agent_id) = budgeted(2_000.0, model.clone()).await;
    let previous = SessionContextTrimmed {
        dropped_through_message_id: "gone".into(),
        at_ms: 1,
    };
    let gate = {
        let mut guard = coordinator.state.write().await;
        guard
            .sessions
            .get_mut(&agent_id, "chat:ctx")
            .unwrap()
            .context_trimmed = Some(previous.clone());
        guard.install_test_control_plane_save_gate(true)
    };
    gate.release.add_permits(1);

    let error = coordinator
        .run(chat_request(&agent_id, "chat:ctx", "hi"))
        .await
        .expect_err("the run-start save failed");

    assert_eq!(error.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(model.requests().is_empty(), "the model never ran");
    let guard = coordinator.state.read().await;
    assert_eq!(
        guard
            .sessions
            .get(&agent_id, "chat:ctx")
            .unwrap()
            .context_trimmed,
        Some(previous),
        "the cleared indicator was never saved"
    );
}

const DAY_MS: u64 = 24 * 60 * 60 * 1_000;

/// Seeds `chat:ctx` with 102 old user/assistant turns (`m000`–`m203`, 9
/// tokens each), mirrors them to a history store, and prunes the hot tail:
/// the oldest two turns (`m000`–`m003`) leave the control plane.
async fn seed_and_prune(coordinator: &AgentRunCoordinator, agent_id: &str) {
    {
        let mut guard = coordinator.state.write().await;
        guard.set_history(HistoryService::new(Arc::new(FlakyHistoryStore::new())));
        let messages = (0..204u64)
            .map(|index| {
                let role = if index % 2 == 0 {
                    MessageRole::User
                } else {
                    MessageRole::Assistant
                };
                let mut old = message(agent_id, "chat:ctx", &format!("m{index:03}"), role, "old");
                old.created_at_ms = 1_000 + index;
                old
            })
            .collect();
        seed(&mut guard, agent_id, messages);
    }
    let now_ms = 10 * DAY_MS;
    let history = coordinator.state.read().await.history.clone();
    history
        .flush_once(&coordinator.state, &tokio::sync::Mutex::new(()), now_ms)
        .await
        .unwrap();
    let pruned = crate::sessions::pruning::prune_once(
        &coordinator.state,
        &Arc::new(tokio::sync::Mutex::new(())),
        now_ms,
    )
    .await;
    assert_eq!(pruned, Ok(4));
}

/// Controller ruling (M3 pre-flight audit I5): turns pruning removed count
/// as dropped context. Every hot message fits the budget, yet the session
/// shows its context was trimmed through the newest pruned message.
#[tokio::test]
async fn pruned_turns_no_summary_covers_mark_the_session_trimmed() {
    let model = ScriptedModel::new(vec![Step::Text(vec!["ok"])]);
    let (coordinator, agent_id) = budgeted(2_000.0, model.clone()).await;
    seed_and_prune(&coordinator, &agent_id).await;

    coordinator
        .run(chat_request(&agent_id, "chat:ctx", "go on"))
        .await
        .unwrap();

    assert_eq!(
        model.requests()[0].messages.len(),
        201,
        "the whole hot tail and the current message fit"
    );
    let guard = coordinator.state.read().await;
    let session = guard.sessions.get(&agent_id, "chat:ctx").unwrap();
    assert_eq!(
        session
            .pruned_through
            .as_ref()
            .map(|pruned| pruned.message_id.as_str()),
        Some("m003")
    );
    assert_eq!(
        session
            .context_trimmed
            .as_ref()
            .map(|trimmed| trimmed.dropped_through_message_id.as_str()),
        Some("m003")
    );
}

/// Ruling I5's other half: a summary that reaches the pruned span, either
/// through its newest message or through a newer hot one, covers it.
#[tokio::test]
async fn a_summary_that_covers_the_pruned_turns_leaves_the_session_untrimmed() {
    for (through, sent) in [("m003", 201), ("m101", 103)] {
        let model = ScriptedModel::new(vec![Step::Text(vec!["ok"])]);
        let (coordinator, agent_id) = budgeted(2_000.0, model.clone()).await;
        seed_and_prune(&coordinator, &agent_id).await;
        coordinator
            .state
            .write()
            .await
            .sessions
            .get_mut(&agent_id, "chat:ctx")
            .unwrap()
            .summary = Some(SessionSummary {
            text: "Earlier turns were old.".into(),
            through_message_id: through.into(),
            created_at_ms: 1,
            source_message_count: 4,
        });

        coordinator
            .run(chat_request(&agent_id, "chat:ctx", "go on"))
            .await
            .unwrap();

        assert_eq!(
            model.requests()[0].messages.len(),
            sent,
            "summary through {through}"
        );
        let guard = coordinator.state.read().await;
        assert_eq!(
            guard
                .sessions
                .get(&agent_id, "chat:ctx")
                .unwrap()
                .context_trimmed,
            None,
            "summary through {through}"
        );
    }
}
