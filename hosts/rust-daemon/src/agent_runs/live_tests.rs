//! Live events of coordinator runs (spec §4.5, §6).

use axum::http::StatusCode;
use serde_json::{json, Value};

use super::test_support::{
    calculate_call, chat_request, coordinator_with, events_until, lead_config, Gate, ScriptedModel,
    Step,
};
use super::InFlightRunGuard;
use crate::live::LiveDelivery;
use crate::routes::ApiError;
use crate::runs::{RunRecord, RunSource, RunStart, RunStatus};

fn types(events: &[Value]) -> Vec<&str> {
    events
        .iter()
        .map(|event| event["type"].as_str().unwrap())
        .collect()
}

fn delta_text(events: &[Value], step_id: &str) -> String {
    events
        .iter()
        .filter(|event| event["type"] == "step.delta" && event["stepId"] == step_id)
        .map(|event| event["text"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn a_run_announces_its_start_text_messages_and_result_in_order() {
    let model = ScriptedModel::new(vec![Step::Text(vec!["Hel", "lo ", "there"])]);
    let (coordinator, agent_id) = coordinator_with(model).await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();

    coordinator
        .run(chat_request(&agent_id, "chat:live", "hi"))
        .await
        .unwrap();

    let events = events_until(&mut subscription, "run.completed").await;
    assert_eq!(types(&events)[..2], ["session.created", "run.started"]);
    let run_id = events[1]["runId"].as_str().unwrap().to_string();
    let step_id = format!("{run_id}:1");
    assert_eq!(delta_text(&events, &step_id), "Hello there");
    let first_delta = events
        .iter()
        .find(|event| event["type"] == "step.delta")
        .unwrap();
    assert_eq!(first_delta["offset"], 0);
    let after_text: Vec<&str> = types(&events)
        .into_iter()
        .skip(2)
        .filter(|name| *name != "step.delta")
        .collect();
    assert_eq!(
        after_text,
        [
            "message.created",
            "message.created",
            "session.updated",
            "run.completed"
        ]
    );
    let messages: Vec<&Value> = events
        .iter()
        .filter(|event| event["type"] == "message.created")
        .collect();
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(messages[1]["stepId"], step_id.as_str());
    let completed = events.last().unwrap();
    assert_eq!(completed["run"]["status"], "completed");
    assert_eq!(completed["run"]["replyMessageId"], messages[1]["messageId"]);
    assert_eq!(completed["run"]["steps"][0]["stepId"], step_id.as_str());
    assert_eq!(completed["run"]["steps"][0]["usage"]["totalTokens"], 12);
    for event in &events {
        assert_eq!(event["agentId"], agent_id.as_str());
        assert_eq!(event["sessionId"], "chat:live");
    }

    assert!(
        hub.runs().view(&run_id).is_none(),
        "a finished run leaves the registry"
    );
    let record = coordinator
        .state
        .read()
        .await
        .runs
        .get(&run_id)
        .cloned()
        .unwrap();
    assert_eq!(
        record.reply_message_id.as_deref(),
        messages[1]["messageId"].as_str()
    );
    assert_eq!(record.steps.len(), 1);
}

#[tokio::test]
async fn tool_steps_publish_cards_with_argument_and_result_previews() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![calculate_call("call-1", "6*7")]),
        Step::Text(vec!["It is 42."]),
    ]);
    let (coordinator, agent_id) = coordinator_with(model).await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();

    coordinator
        .run(chat_request(&agent_id, "chat:tools", "what is 6*7?"))
        .await
        .unwrap();

    let events = events_until(&mut subscription, "run.completed").await;
    let run_id = events[1]["runId"].as_str().unwrap();
    let started = events
        .iter()
        .position(|event| event["type"] == "tool.started")
        .unwrap();
    let finished = events
        .iter()
        .position(|event| event["type"] == "tool.finished")
        .unwrap();
    assert!(started < finished);
    assert_eq!(events[started]["stepId"], format!("{run_id}:1"));
    assert_eq!(events[started]["name"], "calculate");
    assert_eq!(events[started]["toolCallId"], "call-1");
    assert!(events[started]["argumentsPreview"]
        .as_str()
        .unwrap()
        .contains("6*7"));
    assert_eq!(events[started]["argumentsTruncated"], false);
    assert_eq!(events[finished]["status"], "success");
    assert_eq!(events[finished]["resultPreview"], "42");
    assert_eq!(events[finished]["truncated"], false);
    assert_eq!(delta_text(&events, &format!("{run_id}:2")), "It is 42.");
    let roles: Vec<&str> = events
        .iter()
        .filter(|event| event["type"] == "message.created")
        .map(|event| event["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["user", "assistant", "tool", "assistant"]);
    let completed = events.last().unwrap();
    assert_eq!(completed["run"]["toolsStarted"], json!(["calculate"]));
    assert_eq!(completed["run"]["steps"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn a_helpers_run_reaches_its_companions_stream() {
    let model = ScriptedModel::new(vec![Step::Text(vec!["3"])]);
    let (coordinator, _) = coordinator_with(model).await;
    let companion = coordinator
        .state
        .write()
        .await
        .create_agent(lead_config("Companion"))
        .unwrap()
        .state
        .id;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&companion).unwrap();

    coordinator
        .spawn_helper(
            companion.clone(),
            "Adder".into(),
            "Add 1 and 2".into(),
            None,
        )
        .await
        .unwrap();

    let events = events_until(&mut subscription, "run.completed").await;
    assert_eq!(types(&events)[..2], ["session.created", "run.started"]);
    let helper = events[0]["agentId"].as_str().unwrap();
    assert_ne!(helper, companion.as_str());
    assert!(events.iter().all(|event| event["agentId"] == helper));
    assert_eq!(events.last().unwrap()["run"]["source"], "delegation");
}

#[tokio::test]
async fn a_rejected_commit_is_announced_as_a_failed_run_without_its_messages() {
    let model = ScriptedModel::new(vec![Step::Text(vec!["ok"])]);
    let (coordinator, agent_id) = coordinator_with(model).await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();

    let error = coordinator
        .run_with_commit(chat_request(&agent_id, "chat:refused", "hi"), |_, _| {
            Err(ApiError::conflict("hook refused"))
        })
        .await
        .unwrap_err();
    assert_eq!(error.status(), StatusCode::CONFLICT);

    let events = events_until(&mut subscription, "run.failed").await;
    assert!(
        !types(&events).contains(&"message.created"),
        "rolled-back messages are never announced"
    );
    let failed = events.last().unwrap();
    assert_eq!(failed["run"]["error"]["code"], "commit_rejected");
    assert_eq!(failed["run"]["replyMessageId"], Value::Null);
}

fn run_events<'a>(events: &'a [Value], prefix: &str) -> Vec<&'a str> {
    types(events)
        .into_iter()
        .filter(|name| name.starts_with(prefix))
        .collect()
}

#[tokio::test]
async fn a_second_run_in_a_session_announces_no_new_session() {
    let (coordinator, agent_id) = coordinator_with(ScriptedModel::new(Vec::new())).await;
    let hub = coordinator.state.read().await.live.clone();
    coordinator
        .run(chat_request(&agent_id, "chat:again", "first"))
        .await
        .unwrap();
    let mut subscription = hub.subscribe(&agent_id).unwrap();

    coordinator
        .run(chat_request(&agent_id, "chat:again", "second"))
        .await
        .unwrap();

    let events = events_until(&mut subscription, "run.completed").await;
    assert_eq!(types(&events)[0], "run.started");
    assert!(!types(&events).contains(&"session.created"));
    assert_eq!(
        run_events(&events, "run."),
        ["run.started", "run.completed"]
    );
}

#[tokio::test]
async fn the_observer_publishes_text_and_tool_cards_while_the_state_lock_is_held() {
    let gate = Gate::new();
    let model = ScriptedModel::gated(
        vec![
            Step::Tools(vec![calculate_call("call-1", "6*7")]),
            Step::Text(vec!["It is 42."]),
        ],
        gate.clone(),
    );
    let (coordinator, agent_id) = coordinator_with(model).await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();
    let running = {
        let coordinator = coordinator.clone();
        let request = chat_request(&agent_id, "chat:locked", "what is 6*7?");
        tokio::spawn(async move { coordinator.run(request).await })
    };

    // Held from inside the first model call until the second one streamed:
    // the tool, its cards, and the text never wait for the state lock.
    gate.entered().await;
    let guard = coordinator.state.write().await;
    gate.release();
    let tools = events_until(&mut subscription, "tool.finished").await;
    assert!(types(&tools).contains(&"tool.started"));
    gate.entered().await;
    gate.release();
    let text = events_until(&mut subscription, "step.delta").await;
    assert_eq!(text.last().unwrap()["text"], "It is 42.");
    drop(guard);

    running.await.unwrap().unwrap();
    events_until(&mut subscription, "run.completed").await;
}

#[tokio::test]
async fn a_stop_through_the_registered_control_ends_a_held_model_call() {
    let model = ScriptedModel::new(vec![Step::Hold(vec!["Half"])]);
    let (coordinator, agent_id) = coordinator_with(model.clone()).await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();
    let running = {
        let coordinator = coordinator.clone();
        let request = chat_request(&agent_id, "chat:held", "think");
        tokio::spawn(async move { coordinator.run(request).await })
    };
    let events = events_until(&mut subscription, "step.delta").await;
    let run_id = events[1]["runId"].as_str().unwrap().to_string();
    assert_eq!(events.last().unwrap()["text"], "Half");

    // The run uses the control its live registration holds.
    hub.runs().control(&run_id).unwrap().cancel.cancel();
    running.await.unwrap().unwrap();

    // The owner's turn and the stopped partial reply, then the result.
    let mut between = Vec::new();
    let terminal = loop {
        let Some(LiveDelivery::Event(event)) = subscription.next().await else {
            panic!("the run's events continue");
        };
        if event.body.type_name().starts_with("run.") {
            break event;
        }
        between.push(event.body.type_name());
    };
    assert_eq!(
        between,
        ["message.created", "message.created", "session.updated"]
    );
    let ledger = coordinator
        .state
        .read()
        .await
        .runs
        .get(&run_id)
        .cloned()
        .unwrap();
    assert!(ledger.status.is_terminal());
    assert_eq!(terminal.run_id.as_deref(), Some(run_id.as_str()));
    let announced = terminal.to_json(1);
    let expected = crate::live::run_status_event(&ledger).to_json(1);
    assert_eq!(announced["type"], expected["type"]);
    assert_eq!(
        announced["run"], expected["run"],
        "the announced result is the ledger's"
    );
    assert_eq!(model.requests().len(), 1);
    assert!(hub.runs().view(&run_id).is_none());
}

#[tokio::test]
async fn a_failed_final_save_is_announced_as_a_failed_run_without_its_messages() {
    let gate = Gate::new();
    let model = ScriptedModel::gated(vec![Step::Text(vec!["unsaved"])], gate.clone());
    let (coordinator, agent_id) = coordinator_with(model).await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();
    let running = {
        let coordinator = coordinator.clone();
        let request = chat_request(&agent_id, "chat:unsaved", "hi");
        tokio::spawn(async move { coordinator.run(request).await })
    };
    gate.entered().await;
    let save = coordinator
        .state
        .write()
        .await
        .install_test_control_plane_save_gate(true);
    save.release.add_permits(1);
    gate.release();

    let error = running.await.unwrap().unwrap_err();
    assert_eq!(error.status(), StatusCode::SERVICE_UNAVAILABLE);
    let events = events_until(&mut subscription, "run.failed").await;
    let run_id = events[1]["runId"].as_str().unwrap();
    assert_eq!(
        delta_text(&events, &format!("{run_id}:1")),
        "unsaved",
        "the text streamed before the save failed"
    );
    assert!(!types(&events).contains(&"message.created"));
    assert_eq!(run_events(&events, "run."), ["run.started", "run.failed"]);
    let failed = events.last().unwrap();
    assert_eq!(failed["run"]["error"]["code"], "commit_failed");
    assert_eq!(failed["run"]["replyMessageId"], Value::Null);
    let ledger = coordinator
        .state
        .read()
        .await
        .runs
        .get(run_id)
        .cloned()
        .unwrap();
    assert_eq!(failed["run"]["status"], ledger.status.as_str());
    assert_eq!(ledger.reply_message_id, None);
}

#[tokio::test]
async fn an_aborted_run_is_announced_as_failed() {
    let (coordinator, agent_id) = coordinator_with(ScriptedModel::new(Vec::new())).await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();
    let record = RunRecord::running(
        RunStart {
            agent_id: agent_id.clone(),
            session_id: "chat:aborted".into(),
            source: RunSource::Api,
            source_ref: None,
            idempotency_key: None,
            text: "hi".into(),
            model: "gpt-5.4".into(),
            provider: None,
            parent_run_id: None,
        },
        1,
    );
    let run_id = record.id.clone();
    coordinator.state.write().await.runs.insert(record);

    // What a panicking or aborted run task leaves behind.
    drop(InFlightRunGuard::new(
        std::sync::Arc::clone(&coordinator.state),
        run_id.clone(),
    ));

    let events = events_until(&mut subscription, "run.failed").await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["runId"], run_id.as_str());
    assert_eq!(events[0]["run"]["error"]["code"], "run_aborted");
    assert_eq!(
        coordinator
            .state
            .read()
            .await
            .runs
            .get(&run_id)
            .unwrap()
            .status,
        RunStatus::Failed
    );
}

#[tokio::test]
async fn the_snapshot_keeps_reply_ids_out_until_the_version_bump() {
    let (coordinator, agent_id) = coordinator_with(ScriptedModel::new(Vec::new())).await;
    coordinator
        .run(chat_request(&agent_id, "chat:saved", "hi"))
        .await
        .unwrap();

    let guard = coordinator.state.read().await;
    let ledger = guard.runs.for_agent(&agent_id)[0].clone();
    assert!(ledger.reply_message_id.is_some());
    assert_eq!(ledger.steps.len(), 1);
    // Controller ruling (M3 Task 6): Task 7's snapshot version 6 saves it.
    let saved = guard
        .control_plane_snapshot()
        .runs
        .into_iter()
        .find(|record| record.id == ledger.id)
        .unwrap();
    assert_eq!(saved.reply_message_id, None);
    assert_eq!(saved.steps, ledger.steps, "steps are a version-5 field");
}
