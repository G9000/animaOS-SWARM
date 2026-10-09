//! AI titles for new chats (spec §12.3).

use std::time::Duration;

use anima_core::{DataValue, MessageRole};

use super::test_support::{chat_request, coordinator_with, events_until, ScriptedModel, Step};
use crate::sessions::test_support::{message, seed_messages, within};
use crate::sessions::{SessionKind, SessionOrigin, SessionRecord, TitleSource};
use crate::usage::{UsageRecord, UsageSource};

/// Waits (bounded by `within`, 5 seconds) until `model` has received a
/// secondary (title) call.
async fn wait_for_title_call(model: &ScriptedModel) {
    within("a title call", async {
        while model.secondary_requests().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
}

/// Waits (bounded by `within`) until the history service holds a queued
/// secondary-call usage row, and returns them all.
async fn wait_for_usage(coordinator: &super::AgentRunCoordinator) -> Vec<UsageRecord> {
    let history = coordinator.state.read().await.history.clone();
    within("a usage row", async {
        loop {
            let rows = history.pending_usage();
            if !rows.is_empty() {
                return rows;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
}

/// M8 Task 4: the title call's tokens are recorded as a `title` row for the
/// session, whether or not the title it wrote was usable.
#[tokio::test]
async fn a_title_call_records_usage_with_source_title() {
    for (reply, usable) in [("\"Lisbon Trip Plan\"", true), ("Lisbon", false)] {
        let model = ScriptedModel::with_secondary(
            vec![Step::Text(vec!["Lisbon in May is lovely."])],
            vec![Step::Text(vec![reply])],
        );
        let (coordinator, agent_id) = coordinator_with(model.clone()).await;
        coordinator.state.write().await.set_generated_titles(true);

        coordinator
            .run(chat_request(
                &agent_id,
                "chat:new",
                "Help me plan a trip to Lisbon",
            ))
            .await
            .unwrap();

        let rows = wait_for_usage(&coordinator).await;
        assert_eq!(rows.len(), 1, "reply {reply}");
        let row = &rows[0];
        assert_eq!(row.source, UsageSource::Title);
        assert_eq!(row.agent_id, agent_id);
        assert_eq!(row.session_id.as_deref(), Some("chat:new"));
        assert_eq!(row.run_id, None);
        assert_eq!(
            (row.provider.as_str(), row.model.as_str()),
            ("openai", "gpt-5.4")
        );
        assert_eq!((row.prompt_tokens, row.completion_tokens), (10, 2));
        if usable {
            within("the title", async {
                loop {
                    let titled = coordinator
                        .state
                        .read()
                        .await
                        .sessions
                        .get(&agent_id, "chat:new")
                        .is_some_and(|session| session.title_source == TitleSource::Generated);
                    if titled {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await;
        }
    }
}

#[tokio::test]
async fn a_new_chat_is_named_after_its_first_completed_reply() {
    let model = ScriptedModel::with_secondary(
        vec![
            Step::Text(vec!["Lisbon in May is lovely."]),
            Step::Text(vec!["Sure"]),
        ],
        vec![Step::Text(vec!["\"Lisbon Trip Plan\""])],
    );
    let (coordinator, agent_id) = coordinator_with(model.clone()).await;
    coordinator.state.write().await.set_generated_titles(true);
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();

    coordinator
        .run(chat_request(
            &agent_id,
            "chat:new",
            "Help me plan a trip to Lisbon",
        ))
        .await
        .unwrap();
    events_until(&mut subscription, "run.completed").await;
    let named = events_until(&mut subscription, "session.updated").await;
    assert_eq!(named.last().unwrap()["sessionId"], "chat:new");

    {
        let guard = coordinator.state.read().await;
        let session = guard.sessions.get(&agent_id, "chat:new").unwrap();
        assert_eq!(session.title, "Lisbon Trip Plan");
        assert_eq!(session.title_source, TitleSource::Generated);
    }
    let request = &model.secondary_requests()[0];
    assert!(request.messages[0]
        .content
        .text
        .contains("First message:\nHelp me plan a trip to Lisbon"));
    assert!(request.messages[0]
        .content
        .text
        .ends_with("Reply:\nLisbon in May is lovely."));

    coordinator
        .run(chat_request(&agent_id, "chat:new", "Thanks"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        model.secondary_requests().len(),
        1,
        "only the first reply names a chat"
    );
}

#[tokio::test]
async fn titles_stay_off_unless_enabled_and_follow_the_agent_setting() {
    let model = ScriptedModel::new(vec![]);
    let (coordinator, agent_id) = coordinator_with(model.clone()).await;
    coordinator
        .run(chat_request(&agent_id, "chat:off", "hello there"))
        .await
        .unwrap();

    let opted_out = ScriptedModel::new(vec![]);
    let (opted_coordinator, opted_agent) = coordinator_with(opted_out.clone()).await;
    {
        let mut guard = opted_coordinator.state.write().await;
        guard.set_generated_titles(true);
        let mut config = guard.agents[&opted_agent].config().clone();
        config
            .settings
            .as_mut()
            .unwrap()
            .additional
            .insert("autoTitle".into(), DataValue::Bool(false));
        guard.restore_agent_config(&opted_agent, config);
    }
    opted_coordinator
        .run(chat_request(&opted_agent, "chat:off", "hello there"))
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        model.secondary_requests().is_empty(),
        "titles are off by default"
    );
    assert!(
        opted_out.secondary_requests().is_empty(),
        "autoTitle: false opts out"
    );
}

/// Review Minor 1/2 (fix round 1): a non-chat session (here, a Telegram
/// room, by its `telegram:` room-id prefix — spec §3.1) is never titled, and
/// `title_after_first_reply`'s own cheap check must catch it before a
/// background task, let alone a model call, is ever started.
#[tokio::test]
async fn a_completed_reply_in_a_non_chat_session_is_never_titled() {
    let model = ScriptedModel::new(vec![Step::Text(vec!["Hello!"])]);
    let (coordinator, agent_id) = coordinator_with(model.clone()).await;
    coordinator.state.write().await.set_generated_titles(true);

    coordinator
        .run(chat_request(&agent_id, "telegram:12345", "hello there"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert!(
        model.secondary_requests().is_empty(),
        "a Telegram session is never AI-titled"
    );
    let guard = coordinator.state.read().await;
    assert_eq!(
        guard
            .sessions
            .get(&agent_id, "telegram:12345")
            .unwrap()
            .kind,
        SessionKind::Telegram
    );
}

#[tokio::test]
async fn an_unusable_or_failed_title_leaves_the_first_message_title() {
    for secondary in [Step::Text(vec!["Lisbon"]), Step::Fail("rate limited")] {
        let model = ScriptedModel::with_secondary(vec![], vec![secondary]);
        let (coordinator, agent_id) = coordinator_with(model.clone()).await;
        coordinator.state.write().await.set_generated_titles(true);

        coordinator
            .run(chat_request(&agent_id, "chat:keep", "Plan the offsite"))
            .await
            .unwrap();
        wait_for_title_call(&model).await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        let guard = coordinator.state.read().await;
        let session = guard.sessions.get(&agent_id, "chat:keep").unwrap();
        assert_eq!(session.title, "Plan the offsite");
        assert_eq!(session.title_source, TitleSource::FirstMessage);
    }
}

/// Ruling 2 (M3 pre-flight audit, M19): an assistant reply a run's own stop
/// or failure left behind is never held against a later run's title, because
/// that earlier run never reached `RunStatus::Completed` and so never spent
/// the session's one shot at a first-message title.
#[tokio::test]
async fn a_stopped_or_incomplete_earlier_reply_does_not_block_the_title() {
    for marker in [
        anima_core::STOPPED_METADATA_KEY,
        anima_core::INCOMPLETE_METADATA_KEY,
    ] {
        let model = ScriptedModel::with_secondary(
            vec![Step::Text(vec!["Sure, right away."])],
            vec![Step::Text(vec!["Offsite Planning"])],
        );
        let (coordinator, agent_id) = coordinator_with(model.clone()).await;
        {
            let mut guard = coordinator.state.write().await;
            guard.set_generated_titles(true);
            guard.sessions.insert(SessionRecord::new(
                &agent_id,
                "chat:early",
                SessionKind::Chat,
                SessionOrigin::Web,
                "Plan the offsite".into(),
                TitleSource::FirstMessage,
                1,
            ));
            let mut unfinished = message(
                &agent_id,
                "assistant-unfinished",
                "chat:early",
                MessageRole::Assistant,
                "Sure, I'm on",
                1,
            );
            unfinished.content.metadata = Some(std::collections::BTreeMap::from([(
                marker.to_string(),
                DataValue::Bool(true),
            )]));
            seed_messages(&mut guard, &agent_id, vec![unfinished]);
        }

        coordinator
            .run(chat_request(&agent_id, "chat:early", "Anything new?"))
            .await
            .unwrap();
        wait_for_title_call(&model).await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        let guard = coordinator.state.read().await;
        let session = guard.sessions.get(&agent_id, "chat:early").unwrap();
        assert_eq!(session.title, "Offsite Planning", "marker {marker}");
        assert_eq!(
            session.title_source,
            TitleSource::Generated,
            "marker {marker}"
        );
    }
}

/// Controller ruling (M3 pre-flight audit): the model adapter has no request
/// timeout (Task 12 finding), so a title call that never answers must not
/// hang the background task forever; it is handled like a failed call.
#[tokio::test]
async fn a_title_call_that_never_answers_times_out_and_keeps_the_first_message_title() {
    let model = ScriptedModel::with_secondary(vec![], vec![Step::Hold(Vec::new())]);
    let (coordinator, agent_id) = coordinator_with(model.clone()).await;
    let coordinator = coordinator.with_title_timeout(Duration::from_millis(50));
    coordinator.state.write().await.set_generated_titles(true);

    coordinator
        .run(chat_request(&agent_id, "chat:slow", "Plan the offsite"))
        .await
        .unwrap();
    wait_for_title_call(&model).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let guard = coordinator.state.read().await;
    let session = guard.sessions.get(&agent_id, "chat:slow").unwrap();
    assert_eq!(session.title, "Plan the offsite");
    assert_eq!(session.title_source, TitleSource::FirstMessage);
    assert!(
        guard.history.pending_usage().is_empty(),
        "a timed-out title call was dropped and records nothing"
    );
}
