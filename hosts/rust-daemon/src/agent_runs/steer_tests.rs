//! Steering messages into an active run (spec §4.7).

use std::collections::HashSet;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use anima_core::{DataValue, MessageRole};
use axum::http::StatusCode;

use super::test_support::{
    add_chat, calculate_call, chat_request, coordinator_with, events_until, quiet_for, Gate,
    ScriptedModel, Step,
};
use super::{AcceptRun, AcceptedRun, AgentRunCoordinator, SessionRunMode};
use crate::control_plane_store::{load_control_plane_snapshot, ControlPlaneStoreConfig};
use crate::runs::{RunLedger, RunRecord, RunSource, RunStatus, RunStopRequest};

fn message(agent_id: &str, key: &str, text: &str, mode: SessionRunMode) -> AcceptRun {
    AcceptRun {
        agent_id: agent_id.into(),
        session_id: "chat:s".into(),
        text: text.into(),
        idempotency_key: key.into(),
        mode,
        source: RunSource::Web,
        source_ref: None,
        skill: None,
    }
}

async fn accept(coordinator: &AgentRunCoordinator, request: AcceptRun) -> AcceptedRun {
    let start = coordinator.web_start(
        request.agent_id.clone(),
        request.session_id.clone(),
        request.text.clone(),
        request.idempotency_key.clone(),
    );
    coordinator.accept_run(request, start).await.unwrap()
}

async fn wait_for_key(
    coordinator: &AgentRunCoordinator,
    agent_id: &str,
    key: &str,
    status: RunStatus,
) -> RunRecord {
    for _ in 0..500 {
        if let Some(record) = coordinator
            .state
            .read()
            .await
            .runs
            .find_by_idempotency_key(agent_id, key, 0)
            .filter(|record| record.status == status)
        {
            return record.clone();
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no run with key {key} became {status:?}");
}

/// The ledger a restart would start from, given what was saved.
fn restarted(records: Vec<RunRecord>, agent_id: &str) -> RunLedger {
    RunLedger::restored(
        records,
        &HashSet::from([agent_id.to_string()]),
        anima_core::primitives::now_millis(),
    )
}

fn snapshot_path(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "anima-steer-{label}-{}-{}.json",
        std::process::id(),
        anima_core::primitives::now_millis()
    ))
}

#[tokio::test]
async fn a_steer_joins_the_active_run_before_its_next_model_call() {
    let gate = Gate::new();
    let model = ScriptedModel::gated(
        vec![
            Step::Tools(vec![calculate_call("call-1", "2*3")]),
            Step::Text(vec!["Six, and sunny"]),
        ],
        gate.clone(),
    );
    let (coordinator, agent_id) = coordinator_with(model.clone()).await;
    add_chat(&coordinator, &agent_id, "chat:s").await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();

    let AcceptedRun::Created(first) = accept(
        &coordinator,
        message(&agent_id, "key-1", "what is 2*3?", SessionRunMode::Queue),
    )
    .await
    else {
        panic!("the first message is queued");
    };
    gate.entered().await;
    let AcceptedRun::Steered(active) = accept(
        &coordinator,
        message(
            &agent_id,
            "key-2",
            "and the weather?",
            SessionRunMode::Steer,
        ),
    )
    .await
    else {
        panic!("the steer joins the active run");
    };
    assert_eq!(active.id, first.id);
    assert_eq!(active.status, RunStatus::Running);

    gate.release();
    gate.entered().await;
    gate.release();
    wait_for_key(&coordinator, &agent_id, "key-1", RunStatus::Completed).await;

    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    let last = requests[1].messages.last().unwrap();
    assert_eq!(last.role, MessageRole::User);
    assert_eq!(last.content.text, "and the weather?");
    let events = events_until(&mut subscription, "run.completed").await;
    let steered = events
        .iter()
        .find(|event| event["type"] == "run.steered")
        .expect("the stream announces the steer");
    assert_eq!(steered["text"], "and the weather?");
    assert_eq!(steered["runId"], first.id.as_str());

    let guard = coordinator.state.read().await;
    let recorded = guard.agents[&agent_id]
        .messages()
        .iter()
        .find(|message| Some(message.id.as_str()) == steered["messageId"].as_str())
        .expect("the steer is part of the run's transcript");
    let metadata = recorded.content.metadata.as_ref().unwrap();
    assert_eq!(metadata["steer"], DataValue::Bool(true));
    assert_eq!(
        metadata["clientRequestId"],
        DataValue::String("key-2".into())
    );
    assert_eq!(metadata["runId"], DataValue::String(first.id.clone()));
    assert_eq!(
        guard.runs.for_session(&agent_id, "chat:s").len(),
        1,
        "a steer is not a run of its own"
    );
    // Taken into the committed transcript, it leaves nothing for a restart
    // to offer again (audit I3).
    assert!(guard.runs.get(&first.id).unwrap().pending_steers.is_empty());
    let restored = restarted(guard.control_plane_snapshot().runs, &agent_id);
    assert_eq!(restored.for_session(&agent_id, "chat:s").len(), 1);
    assert!(restored
        .find_by_idempotency_key(&agent_id, "key-2", 0)
        .is_none());
}

#[tokio::test]
async fn a_steer_the_run_never_drained_becomes_the_next_queued_message() {
    let gate = Gate::new();
    let model = ScriptedModel::gated(
        vec![
            Step::Text(vec!["First answer"]),
            Step::Text(vec!["Second answer"]),
        ],
        gate.clone(),
    );
    let (coordinator, agent_id) = coordinator_with(model.clone()).await;
    add_chat(&coordinator, &agent_id, "chat:s").await;
    let AcceptedRun::Created(first) = accept(
        &coordinator,
        message(&agent_id, "key-1", "hello", SessionRunMode::Queue),
    )
    .await
    else {
        panic!("queued");
    };
    gate.entered().await;
    let AcceptedRun::Steered(_) = accept(
        &coordinator,
        message(&agent_id, "key-2", "one more thing", SessionRunMode::Steer),
    )
    .await
    else {
        panic!("steered");
    };

    // The first run ends without another model call.
    gate.release();
    wait_for_key(&coordinator, &agent_id, "key-1", RunStatus::Completed).await;
    gate.entered().await;
    gate.release();
    let second = wait_for_key(&coordinator, &agent_id, "key-2", RunStatus::Completed).await;

    assert_eq!(second.input.text, "one more thing");
    assert_eq!(second.source, RunSource::Web);
    assert_eq!(
        model.requests()[1].messages.last().unwrap().content.text,
        "one more thing"
    );
    // It left the joined run's record in the save that queued it.
    let guard = coordinator.state.read().await;
    assert!(guard.runs.get(&first.id).unwrap().pending_steers.is_empty());
    let restored = restarted(guard.control_plane_snapshot().runs, &agent_id);
    assert_eq!(restored.for_session(&agent_id, "chat:s").len(), 2);
}

#[tokio::test]
async fn without_an_active_run_a_steer_waits_in_the_queue() {
    let (coordinator, agent_id) = coordinator_with(ScriptedModel::new(vec![])).await;
    add_chat(&coordinator, &agent_id, "chat:s").await;

    let accepted = accept(
        &coordinator,
        message(&agent_id, "key-1", "hi", SessionRunMode::Steer),
    )
    .await;

    assert!(
        matches!(&accepted, AcceptedRun::Created(record) if record.status == RunStatus::Queued),
        "a steer with nothing to join is a queued message: {accepted:?}"
    );
    wait_for_key(&coordinator, &agent_id, "key-1", RunStatus::Completed).await;
}

#[tokio::test]
async fn a_retried_steer_is_answered_once_and_a_changed_text_conflicts() {
    let gate = Gate::new();
    let model = ScriptedModel::gated(
        vec![
            Step::Tools(vec![calculate_call("call-1", "1+1")]),
            Step::Text(vec!["ok"]),
        ],
        gate.clone(),
    );
    let (coordinator, agent_id) = coordinator_with(model).await;
    add_chat(&coordinator, &agent_id, "chat:s").await;
    let AcceptedRun::Created(first) = accept(
        &coordinator,
        message(&agent_id, "key-1", "compute", SessionRunMode::Queue),
    )
    .await
    else {
        panic!("queued");
    };
    gate.entered().await;
    let AcceptedRun::Steered(_) = accept(
        &coordinator,
        message(&agent_id, "key-2", "also this", SessionRunMode::Steer),
    )
    .await
    else {
        panic!("steered");
    };

    let retried = accept(
        &coordinator,
        message(&agent_id, "key-2", "also this", SessionRunMode::Steer),
    )
    .await;
    assert!(
        matches!(&retried, AcceptedRun::ReplayedSteer(record) if record.id == first.id),
        "still waiting in the run it joined: {retried:?}"
    );
    let start = coordinator.web_start(
        agent_id.clone(),
        "chat:s".into(),
        "something else".into(),
        "key-2".into(),
    );
    let conflict = coordinator
        .accept_run(
            message(&agent_id, "key-2", "something else", SessionRunMode::Steer),
            start,
        )
        .await
        .unwrap_err();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    assert_eq!(
        conflict.message(),
        "Idempotency-Key was already used for a different message"
    );

    gate.release();
    gate.entered().await;
    gate.release();
    wait_for_key(&coordinator, &agent_id, "key-1", RunStatus::Completed).await;

    // After the run, the recorded steer still answers its key.
    let after = accept(
        &coordinator,
        message(&agent_id, "key-2", "also this", SessionRunMode::Queue),
    )
    .await;
    assert!(matches!(&after, AcceptedRun::Replayed(record) if record.id == first.id));
    let start = coordinator.web_start(
        agent_id.clone(),
        "chat:s".into(),
        "something else".into(),
        "key-2".into(),
    );
    let conflict = coordinator
        .accept_run(
            message(&agent_id, "key-2", "something else", SessionRunMode::Queue),
            start,
        )
        .await
        .unwrap_err();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    assert_eq!(
        coordinator
            .state
            .read()
            .await
            .runs
            .for_session(&agent_id, "chat:s")
            .len(),
        1
    );
}

/// Controller ruling (M3 pre-flight audit I3): an accepted steer is saved
/// with the run it joined, in its acceptance save, so a restart before the
/// run takes it in offers it again as an `interrupted` message.
#[tokio::test]
async fn a_saved_steer_the_run_never_took_in_is_offered_again_after_a_restart() {
    let gate = Gate::new();
    let model = ScriptedModel::gated(vec![Step::Text(vec!["Working"])], gate.clone());
    let (coordinator, agent_id) = coordinator_with(model).await;
    add_chat(&coordinator, &agent_id, "chat:s").await;
    let path = snapshot_path("restart");
    let store = ControlPlaneStoreConfig::Json(path.clone());
    coordinator
        .state
        .write()
        .await
        .set_control_plane_store(Some(store.clone()));
    let AcceptedRun::Created(first) = accept(
        &coordinator,
        message(&agent_id, "key-1", "hello", SessionRunMode::Queue),
    )
    .await
    else {
        panic!("queued");
    };
    gate.entered().await;
    let AcceptedRun::Steered(_) = accept(
        &coordinator,
        message(&agent_id, "key-2", "one more thing", SessionRunMode::Steer),
    )
    .await
    else {
        panic!("steered");
    };

    let saved = load_control_plane_snapshot(&store).await.unwrap().unwrap();
    let joined = saved.runs.iter().find(|run| run.id == first.id).unwrap();
    assert_eq!(joined.pending_steers.len(), 1, "saved at acceptance");
    let steer = joined.pending_steers[0].clone();
    assert_eq!(steer.idempotency_key, "key-2");
    assert_eq!(steer.text, "one more thing");
    let restored = restarted(saved.runs, &agent_id);
    let offered = restored
        .find_by_idempotency_key(&agent_id, "key-2", 0)
        .expect("the steer is offered again");
    assert_eq!(offered.status, RunStatus::Interrupted);
    let error = offered.error.as_ref().unwrap();
    assert_eq!(error.code, "restart_before_start");
    assert_eq!(
        error.message,
        "The daemon restarted before this run started; it is safe to send it again."
    );
    assert_eq!(offered.session_id, "chat:s");
    assert_eq!(offered.source, RunSource::Web);
    assert_eq!(offered.input.text, "one more thing");
    assert_eq!(offered.created_at_ms, steer.accepted_at_ms);
    assert_eq!(offered.started_at_ms, None);
    let run = restored.get(&first.id).unwrap();
    assert_eq!(run.error.as_ref().unwrap().code, "restart_during_run");
    assert!(run.pending_steers.is_empty(), "offered once");
    assert_eq!(restored.for_session(&agent_id, "chat:s").len(), 2);

    // The live daemon still hands it on as the next message.
    gate.release.add_permits(2);
    wait_for_key(&coordinator, &agent_id, "key-2", RunStatus::Completed).await;
    let _ = std::fs::remove_file(path);
}

/// A steer is saved before its run can see it: when that save fails, the
/// steer is refused (503) and the run never reads it.
#[tokio::test]
async fn a_steer_whose_save_fails_is_refused_before_its_run_sees_it() {
    let gate = Gate::new();
    let model = ScriptedModel::gated(
        vec![
            Step::Tools(vec![calculate_call("call-1", "1+1")]),
            Step::Text(vec!["ok"]),
        ],
        gate.clone(),
    );
    let (coordinator, agent_id) = coordinator_with(model.clone()).await;
    add_chat(&coordinator, &agent_id, "chat:s").await;
    let AcceptedRun::Created(first) = accept(
        &coordinator,
        message(&agent_id, "key-1", "compute", SessionRunMode::Queue),
    )
    .await
    else {
        panic!("queued");
    };
    gate.entered().await;
    let save_gate = coordinator
        .state
        .write()
        .await
        .install_test_control_plane_save_gate(true);
    save_gate.release.add_permits(1);
    let start = coordinator.web_start(
        agent_id.clone(),
        "chat:s".into(),
        "also this".into(),
        "key-2".into(),
    );
    let refused = coordinator
        .accept_run(
            message(&agent_id, "key-2", "also this", SessionRunMode::Steer),
            start,
        )
        .await
        .unwrap_err();
    assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(coordinator
        .state
        .read()
        .await
        .runs
        .get(&first.id)
        .unwrap()
        .pending_steers
        .is_empty());

    gate.release();
    gate.entered().await;
    gate.release();
    wait_for_key(&coordinator, &agent_id, "key-1", RunStatus::Completed).await;
    assert!(model.requests()[1]
        .messages
        .iter()
        .all(|message| message.content.text != "also this"));
    assert!(coordinator
        .state
        .read()
        .await
        .runs
        .find_by_idempotency_key(&agent_id, "key-2", 0)
        .is_none());
}

/// Task 8's stop path with a saved steer (audit M8): the steer the stopped
/// run never read becomes exactly one `interrupted` run, and it leaves the
/// run's record in that same save, so a restart offers nothing twice.
#[tokio::test]
async fn a_stop_turns_a_saved_steer_into_one_run_to_send_again() {
    let model = ScriptedModel::new(vec![Step::Hold(vec!["Working"])]);
    let (coordinator, agent_id) = coordinator_with(model.clone()).await;
    add_chat(&coordinator, &agent_id, "chat:s").await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();
    let AcceptedRun::Created(first) = accept(
        &coordinator,
        message(&agent_id, "key-1", "hello", SessionRunMode::Queue),
    )
    .await
    else {
        panic!("queued");
    };
    let _ = events_until(&mut subscription, "step.delta").await;
    let AcceptedRun::Steered(_) = accept(
        &coordinator,
        message(
            &agent_id,
            "key-2",
            "and the weather?",
            SessionRunMode::Steer,
        ),
    )
    .await
    else {
        panic!("steered");
    };

    coordinator.stop_run(&agent_id, &first.id).await.unwrap();
    let mut events = events_until(&mut subscription, "run.cancelled").await;
    events.extend(quiet_for(&mut subscription).await);

    let interrupted = events
        .iter()
        .filter(|event| event["type"] == "run.interrupted")
        .collect::<Vec<_>>();
    assert_eq!(interrupted.len(), 1, "{events:?}");
    let guard = coordinator.state.read().await;
    let record = guard
        .runs
        .find_by_idempotency_key(&agent_id, "key-2", 0)
        .unwrap();
    assert_eq!(interrupted[0]["runId"], record.id.as_str());
    assert_eq!(record.status, RunStatus::Interrupted);
    assert_eq!(record.error.as_ref().unwrap().code, "stopped_before_start");
    assert_eq!(record.input.text, "and the weather?");
    assert!(guard.runs.get(&first.id).unwrap().pending_steers.is_empty());
    let restored = restarted(guard.control_plane_snapshot().runs, &agent_id);
    assert_eq!(restored.for_session(&agent_id, "chat:s").len(), 2);
    assert_eq!(model.requests().len(), 1, "the steer never ran");
}

/// A steer sent once the run's stop is saved does not join it: the run
/// is ending, so the steer waits its turn as a message.
#[tokio::test]
async fn a_steer_after_a_saved_stop_waits_as_the_next_message() {
    let model = ScriptedModel::new(vec![Step::Hold(vec!["Working"])]);
    let (coordinator, agent_id) = coordinator_with(model.clone()).await;
    add_chat(&coordinator, &agent_id, "chat:s").await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();
    let AcceptedRun::Created(first) = accept(
        &coordinator,
        message(&agent_id, "key-1", "hello", SessionRunMode::Queue),
    )
    .await
    else {
        panic!("queued");
    };
    let _ = events_until(&mut subscription, "step.delta").await;
    // The stop is saved; the run itself may still be ending.
    coordinator.stop_run(&agent_id, &first.id).await.unwrap();

    let accepted = accept(
        &coordinator,
        message(&agent_id, "key-2", "never mind", SessionRunMode::Steer),
    )
    .await;

    assert!(
        matches!(&accepted, AcceptedRun::Created(record) if record.status == RunStatus::Queued),
        "{accepted:?}"
    );
    let second = wait_for_key(&coordinator, &agent_id, "key-2", RunStatus::Completed).await;
    assert_eq!(second.input.text, "never mind");
    assert_eq!(model.requests().len(), 2);
}

/// Controller ruling (M3 pre-flight audit M9): steers a run leaves behind
/// become queued messages only within the agent's cap of 8; the rest become
/// `interrupted` runs to send again.
#[tokio::test]
async fn steers_left_beyond_the_queue_cap_are_offered_again_instead() {
    let gate = Gate::new();
    let model = ScriptedModel::gated(vec![Step::Text(vec!["First answer"])], gate.clone());
    let (coordinator, agent_id) = coordinator_with(model).await;
    add_chat(&coordinator, &agent_id, "chat:s").await;
    let AcceptedRun::Created(_) = accept(
        &coordinator,
        message(&agent_id, "key-0", "hello", SessionRunMode::Queue),
    )
    .await
    else {
        panic!("queued");
    };
    gate.entered().await;
    for n in 1..=7 {
        let key = format!("queued-{n}");
        let AcceptedRun::Created(_) = accept(
            &coordinator,
            message(&agent_id, &key, &key, SessionRunMode::Queue),
        )
        .await
        else {
            panic!("{key} is queued");
        };
    }
    for key in ["steer-1", "steer-2"] {
        let AcceptedRun::Steered(_) = accept(
            &coordinator,
            message(&agent_id, key, key, SessionRunMode::Steer),
        )
        .await
        else {
            panic!("{key} joins the run");
        };
    }

    gate.release.add_permits(20);
    wait_for_key(&coordinator, &agent_id, "key-0", RunStatus::Completed).await;

    let refused = wait_for_key(&coordinator, &agent_id, "steer-2", RunStatus::Interrupted).await;
    let error = refused.error.as_ref().unwrap();
    assert_eq!(error.code, "queue_full_before_start");
    assert_eq!(
        error.message,
        "Eight messages were already waiting when the run this message joined ended; it is safe to send it again."
    );
    assert_eq!(refused.input.text, "steer-2");
    let kept = wait_for_key(&coordinator, &agent_id, "steer-1", RunStatus::Completed).await;
    assert_eq!(kept.input.text, "steer-1");
    wait_for_key(&coordinator, &agent_id, "queued-7", RunStatus::Completed).await;
}

/// Task 7 review Minor 2: acceptance times come from a coordinator-wide
/// clock that never goes back, so a wall clock stepping backwards cannot
/// reorder a session's messages or its requeued steers.
#[tokio::test]
async fn a_backward_clock_step_does_not_reorder_a_sessions_messages() {
    let gate = Gate::new();
    let model = ScriptedModel::gated(vec![], gate.clone());
    let (coordinator, agent_id) = coordinator_with(model.clone()).await;
    add_chat(&coordinator, &agent_id, "chat:s").await;
    let AcceptedRun::Created(_) = accept(
        &coordinator,
        message(&agent_id, "key-0", "running", SessionRunMode::Queue),
    )
    .await
    else {
        panic!("queued");
    };
    gate.entered().await;
    let now = anima_core::primitives::now_millis();
    let readings = Arc::new(StdMutex::new(vec![
        now + 10_000, // first
        now,          // second: the clock stepped back
        now + 20_000, // the steer
        now,          // third: back again
    ]));
    coordinator.set_wall_clock_for_test({
        let readings = Arc::clone(&readings);
        move || {
            let mut readings = readings.lock().unwrap();
            if readings.is_empty() {
                anima_core::primitives::now_millis()
            } else {
                readings.remove(0)
            }
        }
    });
    for (key, mode) in [
        ("first", SessionRunMode::Queue),
        ("second", SessionRunMode::Queue),
        ("steer", SessionRunMode::Steer),
        ("third", SessionRunMode::Queue),
    ] {
        let accepted = accept(&coordinator, message(&agent_id, key, key, mode)).await;
        match mode {
            SessionRunMode::Queue => assert!(
                matches!(accepted, AcceptedRun::Created(_)),
                "{key}: {accepted:?}"
            ),
            SessionRunMode::Steer => assert!(
                matches!(accepted, AcceptedRun::Steered(_)),
                "{key}: {accepted:?}"
            ),
        }
    }
    assert!(readings.lock().unwrap().is_empty());

    gate.release.add_permits(10);
    wait_for_key(&coordinator, &agent_id, "third", RunStatus::Completed).await;

    let order = model
        .requests()
        .iter()
        .map(|request| request.messages.last().unwrap().content.text.clone())
        .collect::<Vec<_>>();
    assert_eq!(order, ["running", "first", "second", "steer", "third"]);
}

/// The runs a failed run's steers become (fix round 1): each is offered
/// again as `interrupted` (`failed_before_start`) in the change that fails
/// the run, saved with it and announced once saved, and a retried key
/// answers with it.
async fn assert_offered_again_as_failed_before_start(
    coordinator: &AgentRunCoordinator,
    agent_id: &str,
    run_id: &str,
    events: &[serde_json::Value],
) {
    let guard = coordinator.state.read().await;
    let offered = guard
        .runs
        .find_by_idempotency_key(agent_id, "key-2", 0)
        .expect("the steer is offered again")
        .clone();
    assert_eq!(offered.status, RunStatus::Interrupted);
    let error = offered.error.as_ref().unwrap();
    assert_eq!(error.code, "failed_before_start");
    assert_eq!(
        error.message,
        "The run this message joined failed before reading it; send it again."
    );
    assert_eq!(offered.input.text, "also this");
    assert_eq!(offered.session_id, "chat:s");
    assert_eq!(offered.started_at_ms, None);
    assert!(guard.runs.get(run_id).unwrap().pending_steers.is_empty());
    assert!(
        events.iter().any(
            |event| event["type"] == "run.interrupted" && event["runId"] == offered.id.as_str()
        ),
        "announced once saved: {events:?}"
    );
    let restored = restarted(guard.control_plane_snapshot().runs, agent_id);
    assert_eq!(
        restored.for_session(agent_id, "chat:s").len(),
        2,
        "a restart offers it only once"
    );
    drop(guard);
    let retried = accept(
        coordinator,
        message(agent_id, "key-2", "also this", SessionRunMode::Steer),
    )
    .await;
    assert!(
        matches!(&retried, AcceptedRun::Replayed(record) if record.id == offered.id),
        "{retried:?}"
    );
}

/// A steer the run took in is part of the run's result: when that result
/// cannot be saved, the steer is offered again instead of being lost.
#[tokio::test]
async fn a_steer_of_a_run_whose_result_cannot_be_saved_is_offered_again() {
    let gate = Gate::new();
    let model = ScriptedModel::gated(
        vec![
            Step::Tools(vec![calculate_call("call-1", "1+1")]),
            Step::Text(vec!["ok"]),
        ],
        gate.clone(),
    );
    let (coordinator, agent_id) = coordinator_with(model.clone()).await;
    add_chat(&coordinator, &agent_id, "chat:s").await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();
    let AcceptedRun::Created(first) = accept(
        &coordinator,
        message(&agent_id, "key-1", "compute", SessionRunMode::Queue),
    )
    .await
    else {
        panic!("queued");
    };
    gate.entered().await;
    let AcceptedRun::Steered(_) = accept(
        &coordinator,
        message(&agent_id, "key-2", "also this", SessionRunMode::Steer),
    )
    .await
    else {
        panic!("steered");
    };
    gate.release();
    gate.entered().await;
    assert_eq!(
        model.requests()[1].messages.last().unwrap().content.text,
        "also this",
        "taken in before the second call"
    );
    // The run's result save is the next save: it fails.
    let save_gate = coordinator
        .state
        .write()
        .await
        .install_test_control_plane_save_gate(true);
    save_gate.release.add_permits(1);
    gate.release();
    let mut events = events_until(&mut subscription, "run.failed").await;
    events.extend(quiet_for(&mut subscription).await);
    let failed = wait_for_key(&coordinator, &agent_id, "key-1", RunStatus::Failed).await;
    assert_eq!(failed.error.as_ref().unwrap().code, "commit_failed");

    assert_offered_again_as_failed_before_start(&coordinator, &agent_id, &first.id, &events).await;
}

/// The same for a result its source refused (`commit_rejected`).
#[tokio::test]
async fn a_steer_of_a_run_whose_result_is_refused_is_offered_again() {
    let gate = Gate::new();
    let model = ScriptedModel::gated(
        vec![
            Step::Tools(vec![calculate_call("call-1", "1+1")]),
            Step::Text(vec!["ok"]),
        ],
        gate.clone(),
    );
    let (coordinator, agent_id) = coordinator_with(model).await;
    add_chat(&coordinator, &agent_id, "chat:s").await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();
    let running = {
        let coordinator = coordinator.clone();
        let request = chat_request(&agent_id, "chat:s", "compute");
        tokio::spawn(async move {
            coordinator
                .run_with_commit(request, |_, _| {
                    Err(crate::routes::ApiError::conflict("the source refused it"))
                })
                .await
        })
    };
    gate.entered().await;
    let AcceptedRun::Steered(joined) = accept(
        &coordinator,
        message(&agent_id, "key-2", "also this", SessionRunMode::Steer),
    )
    .await
    else {
        panic!("steered");
    };
    gate.release();
    // Taken in before the second call, whose result the source refuses.
    gate.entered().await;
    gate.release();
    running.await.unwrap().unwrap_err();
    let mut events = events_until(&mut subscription, "run.failed").await;
    events.extend(quiet_for(&mut subscription).await);
    let failed = coordinator
        .state
        .read()
        .await
        .runs
        .get(&joined.id)
        .cloned()
        .unwrap();
    assert_eq!(failed.status, RunStatus::Failed);
    assert_eq!(failed.error.as_ref().unwrap().code, "commit_rejected");

    assert_offered_again_as_failed_before_start(&coordinator, &agent_id, &joined.id, &events).await;
}

/// The same for a run whose task panicked while it held the steer.
#[tokio::test]
async fn a_steer_of_a_run_that_crashed_is_offered_again() {
    let gate = Gate::new();
    let model = ScriptedModel::gated(vec![Step::Panic("the model crashed")], gate.clone());
    let (coordinator, agent_id) = coordinator_with(model).await;
    add_chat(&coordinator, &agent_id, "chat:s").await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();
    let AcceptedRun::Created(first) = accept(
        &coordinator,
        message(&agent_id, "key-1", "compute", SessionRunMode::Queue),
    )
    .await
    else {
        panic!("queued");
    };
    gate.entered().await;
    let AcceptedRun::Steered(_) = accept(
        &coordinator,
        message(&agent_id, "key-2", "also this", SessionRunMode::Steer),
    )
    .await
    else {
        panic!("steered");
    };
    gate.release();
    let events = events_until(&mut subscription, "run.interrupted").await;
    let failed = wait_for_key(&coordinator, &agent_id, "key-1", RunStatus::Failed).await;
    assert_eq!(failed.error.as_ref().unwrap().code, "run_aborted");

    assert_offered_again_as_failed_before_start(&coordinator, &agent_id, &first.id, &events).await;
}

/// A steer whose save succeeds after the run's inbox closed is refused by
/// the inbox; the run's end, waiting for that save's transaction, finds it
/// on the run's record alone and makes it the next queued message.
#[tokio::test]
async fn a_steer_saved_as_its_run_finished_becomes_the_next_message() {
    let gate = Gate::new();
    let model = ScriptedModel::gated(vec![Step::Text(vec!["First answer"])], gate.clone());
    let (coordinator, agent_id) = coordinator_with(model.clone()).await;
    add_chat(&coordinator, &agent_id, "chat:s").await;
    let hub = coordinator.state.read().await.live.clone();
    let AcceptedRun::Created(first) = accept(
        &coordinator,
        message(&agent_id, "key-1", "hello", SessionRunMode::Queue),
    )
    .await
    else {
        panic!("queued");
    };
    gate.entered().await;
    let control = hub.runs().control(&first.id).unwrap();
    // The steer's acceptance save is held.
    let save_gate = coordinator
        .state
        .write()
        .await
        .install_test_control_plane_save_gate(false);
    let steering = {
        let coordinator = coordinator.clone();
        let request = message(&agent_id, "key-2", "one more thing", SessionRunMode::Steer);
        tokio::spawn(async move { accept(&coordinator, request).await })
    };
    tokio::time::timeout(Duration::from_secs(5), save_gate.entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    // Meanwhile the run's only model call ends and its inbox closes.
    gate.release();
    for _ in 0..500 {
        if control.steering.is_closed() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(control.steering.is_closed());
    save_gate.release.add_permits(1);

    let answer = steering.await.unwrap();
    assert!(
        matches!(&answer, AcceptedRun::Steered(record) if record.id == first.id),
        "{answer:?}"
    );
    gate.release();
    let second = wait_for_key(&coordinator, &agent_id, "key-2", RunStatus::Completed).await;
    assert_eq!(second.input.text, "one more thing");
    wait_for_key(&coordinator, &agent_id, "key-1", RunStatus::Completed).await;
    let guard = coordinator.state.read().await;
    assert!(guard.runs.get(&first.id).unwrap().pending_steers.is_empty());
    assert_eq!(guard.runs.for_session(&agent_id, "chat:s").len(), 2);
    assert_eq!(model.requests().len(), 2);
}

/// A run whose stop is saved but whose signal has not reached it yet ends
/// normally; the steers it left are still interrupted, not requeued, since
/// the stop is read from its record under the transaction (fix round 1).
#[tokio::test]
async fn a_saved_stop_that_has_not_reached_the_run_still_keeps_its_steers_from_running() {
    let gate = Gate::new();
    let model = ScriptedModel::gated(vec![Step::Text(vec!["First answer"])], gate.clone());
    let (coordinator, agent_id) = coordinator_with(model.clone()).await;
    add_chat(&coordinator, &agent_id, "chat:s").await;
    let AcceptedRun::Created(first) = accept(
        &coordinator,
        message(&agent_id, "key-1", "hello", SessionRunMode::Queue),
    )
    .await
    else {
        panic!("queued");
    };
    gate.entered().await;
    let AcceptedRun::Steered(_) = accept(
        &coordinator,
        message(
            &agent_id,
            "key-2",
            "and the weather?",
            SessionRunMode::Steer,
        ),
    )
    .await
    else {
        panic!("steered");
    };
    // The stop is saved; its signal comes only after that save.
    coordinator
        .state
        .write()
        .await
        .runs
        .get_mut(&first.id)
        .unwrap()
        .stop = Some(RunStopRequest {
        requested_at_ms: anima_core::primitives::now_millis(),
    });
    gate.release();

    let interrupted = wait_for_key(&coordinator, &agent_id, "key-2", RunStatus::Interrupted).await;
    assert_eq!(
        interrupted.error.as_ref().unwrap().code,
        "stopped_before_start"
    );
    wait_for_key(&coordinator, &agent_id, "key-1", RunStatus::Completed).await;
    gate.release.add_permits(2);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(model.requests().len(), 1, "the steer never ran");
}

/// Fix round 2: a crashed run is cleaned up under the control-plane
/// transaction. A steer whose acceptance save is in flight when the run's
/// task panics is answered with the run as it was when the steer joined
/// (still running), never with a run already failed, and is then offered
/// again once.
#[tokio::test]
async fn a_steer_racing_a_crashed_run_is_answered_with_the_run_it_joined() {
    let gate = Gate::new();
    let model = ScriptedModel::gated(vec![Step::Panic("the model crashed")], gate.clone());
    let (coordinator, agent_id) = coordinator_with(model).await;
    add_chat(&coordinator, &agent_id, "chat:s").await;
    let hub = coordinator.state.read().await.live.clone();
    let AcceptedRun::Created(first) = accept(
        &coordinator,
        message(&agent_id, "key-1", "compute", SessionRunMode::Queue),
    )
    .await
    else {
        panic!("queued");
    };
    gate.entered().await;
    let control = hub.runs().control(&first.id).unwrap();
    // The steer joins; its acceptance save is held.
    let save_gate = coordinator
        .state
        .write()
        .await
        .install_test_control_plane_save_gate(false);
    let steering = {
        let coordinator = coordinator.clone();
        let request = message(&agent_id, "key-2", "also this", SessionRunMode::Steer);
        tokio::spawn(async move { accept(&coordinator, request).await })
    };
    tokio::time::timeout(Duration::from_secs(5), save_gate.entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    // The run's task panics while that save is in flight.
    gate.release();
    for _ in 0..500 {
        let failed = coordinator
            .state
            .read()
            .await
            .runs
            .get(&first.id)
            .is_some_and(|record| record.status == RunStatus::Failed);
        if failed || control.steering.is_closed() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    save_gate.release.add_permits(1);

    let answer = steering.await.unwrap();
    let AcceptedRun::Steered(joined) = &answer else {
        panic!("the steer joined before the crash: {answer:?}");
    };
    assert_eq!(joined.id, first.id);
    assert_eq!(
        joined.status,
        RunStatus::Running,
        "never answered with a run already failed"
    );
    let offered = wait_for_key(&coordinator, &agent_id, "key-2", RunStatus::Interrupted).await;
    assert_eq!(offered.error.as_ref().unwrap().code, "failed_before_start");
    let failed = wait_for_key(&coordinator, &agent_id, "key-1", RunStatus::Failed).await;
    assert_eq!(failed.error.as_ref().unwrap().code, "run_aborted");
    let guard = coordinator.state.read().await;
    assert!(guard.runs.get(&first.id).unwrap().pending_steers.is_empty());
    assert_eq!(guard.runs.for_session(&agent_id, "chat:s").len(), 2);
}

/// Fix round 2: a crashed run's inbox closes as its task ends, so a steer
/// sent before its cleanup runs is a new message, not a steer into a run
/// that is about to fail.
#[tokio::test]
async fn a_steer_sent_as_a_run_crashes_waits_as_the_next_message() {
    let gate = Gate::new();
    let model = ScriptedModel::gated(vec![Step::Panic("the model crashed")], gate.clone());
    let (coordinator, agent_id) = coordinator_with(model.clone()).await;
    add_chat(&coordinator, &agent_id, "chat:s").await;
    let hub = coordinator.state.read().await.live.clone();
    let AcceptedRun::Created(first) = accept(
        &coordinator,
        message(&agent_id, "key-1", "compute", SessionRunMode::Queue),
    )
    .await
    else {
        panic!("queued");
    };
    gate.entered().await;
    let control = hub.runs().control(&first.id).unwrap();
    // The cleanup waits for this transaction.
    let transaction = coordinator.control_plane_transaction().await;
    gate.release();
    for _ in 0..500 {
        if control.steering.is_closed() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(control.steering.is_closed(), "closed as the task ended");
    assert_eq!(
        coordinator
            .state
            .read()
            .await
            .runs
            .get(&first.id)
            .unwrap()
            .status,
        RunStatus::Running,
        "not failed outside the transaction"
    );
    let steering = {
        let coordinator = coordinator.clone();
        let request = message(&agent_id, "key-2", "also this", SessionRunMode::Steer);
        tokio::spawn(async move { accept(&coordinator, request).await })
    };
    drop(transaction);

    let answer = steering.await.unwrap();
    assert!(
        matches!(&answer, AcceptedRun::Created(record) if record.status == RunStatus::Queued),
        "{answer:?}"
    );
    gate.release();
    let second = wait_for_key(&coordinator, &agent_id, "key-2", RunStatus::Completed).await;
    assert_eq!(second.input.text, "also this");
    wait_for_key(&coordinator, &agent_id, "key-1", RunStatus::Failed).await;
}

/// Final fix wave S2-B (review B, Minor 2): a steer handed on from a run the
/// session queue does not own (a check-in tick) keeps its acceptance order,
/// so it runs before a message accepted after it. The session's drainer
/// takes the room before it picks its next message, and the tick holds the
/// room until it has handed the steer on.
#[tokio::test]
async fn a_steer_a_check_in_left_unread_runs_before_a_later_message() {
    let gate = Gate::new();
    let model = ScriptedModel::gated(
        vec![
            Step::Text(vec!["Nothing new"]),
            Step::Text(vec!["About the check-in"]),
            Step::Text(vec!["About the question"]),
        ],
        gate.clone(),
    );
    let (coordinator, agent_id) = coordinator_with(model.clone()).await;
    add_chat(&coordinator, &agent_id, "chat:s").await;
    let tick = {
        let coordinator = coordinator.clone();
        let mut request = chat_request(&agent_id, "chat:s", "Check in on the owner");
        request.source = RunSource::Schedule;
        request.content.metadata = Some(std::collections::BTreeMap::from([(
            "kind".to_string(),
            DataValue::String("checkin".into()),
        )]));
        tokio::spawn(async move {
            coordinator
                .run_with_commit_waiting(request, |_, _| Ok(()), |_| Ok(()))
                .await
        })
    };
    gate.entered().await;
    let AcceptedRun::Steered(joined) = accept(
        &coordinator,
        message(
            &agent_id,
            "key-steer",
            "and the check-in?",
            SessionRunMode::Steer,
        ),
    )
    .await
    else {
        panic!("the steer joins the check-in");
    };
    assert_eq!(joined.source, RunSource::Schedule);
    let AcceptedRun::Created(_) = accept(
        &coordinator,
        message(
            &agent_id,
            "key-later",
            "a new question",
            SessionRunMode::Queue,
        ),
    )
    .await
    else {
        panic!("the later message is queued");
    };
    // The later message's session queue is waiting for the room by now.
    tokio::time::sleep(Duration::from_millis(20)).await;

    // The check-in ends without another model call, leaving the steer unread.
    gate.release();
    tick.await.unwrap().unwrap();
    gate.entered().await;
    gate.release();
    gate.entered().await;
    gate.release();
    wait_for_key(&coordinator, &agent_id, "key-steer", RunStatus::Completed).await;
    wait_for_key(&coordinator, &agent_id, "key-later", RunStatus::Completed).await;

    let sent: Vec<String> = model
        .requests()
        .iter()
        .map(|request| request.messages.last().unwrap().content.text.clone())
        .collect();
    assert_eq!(
        sent[1..],
        ["and the check-in?", "a new question"],
        "the steer, accepted first, runs first"
    );
}
