//! AI titles for new chats (spec §12.3).

use std::time::Duration;

use anima_core::{DataValue, MessageRole};

use super::test_support::{chat_request, coordinator_with, events_until, ScriptedModel, Step};
use crate::sessions::test_support::{message, seed_messages};
use crate::sessions::{SessionKind, SessionOrigin, SessionRecord, TitleSource};

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
        for _ in 0..500 {
            if !model.secondary_requests().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
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
        for _ in 0..500 {
            if !model.secondary_requests().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
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
    for _ in 0..500 {
        if !model.secondary_requests().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;

    let guard = coordinator.state.read().await;
    let session = guard.sessions.get(&agent_id, "chat:slow").unwrap();
    assert_eq!(session.title, "Plan the offsite");
    assert_eq!(session.title_source, TitleSource::FirstMessage);
}
