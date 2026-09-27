//! Accepted runs and admission waits (spec §4.2–§4.3, §4.6; M1 carry-forwards).

use std::collections::HashSet;
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::Value;

use super::test_support::{
    accept, accept_web, add_chat, chat_request, coordinator_with, is_terminal, next_event,
    quiet_for, wait_for, Gate, ScriptedModel,
};
use super::{
    AcceptedRun, AdmitMode, AgentRunCoordinator, QueuedRunStart, MAX_QUEUED_RUNS_PER_AGENT,
    RUN_NOT_QUEUED, RUN_STOPPED_BEFORE_START,
};
use crate::runs::{RunLedger, RunRecord, RunSource, RunStart, RunStatus};
use crate::sessions::{SessionKind, SessionOrigin, SessionRecord, TitleSource, DEFAULT_CHAT_TITLE};

#[tokio::test]
async fn cancelled_admission_waits_leave_no_room_or_slot_entries() {
    let gate = Gate::new();
    let (coordinator, agent_id) =
        coordinator_with(ScriptedModel::gated(vec![], gate.clone())).await;
    let coordinator = coordinator.with_max_runs_per_agent(1);
    let holding = {
        let coordinator = coordinator.clone();
        let request = chat_request(&agent_id, "room-a", "hold");
        tokio::spawn(async move { coordinator.run(request).await })
    };
    gate.entered().await;
    assert_eq!(coordinator.lock_counts(), (1, 1));

    // One wait for the held room, one for the only slot; both given up.
    for room in ["room-a", "room-b"] {
        let wait = tokio::time::timeout(
            Duration::from_millis(20),
            coordinator.admit(&agent_id, room, AdmitMode::Wait),
        )
        .await;
        assert!(wait.is_err(), "{room} has to wait");
    }
    assert_eq!(
        coordinator.lock_counts(),
        (1, 1),
        "a dropped wait leaves no registry entry behind"
    );

    gate.release();
    holding.await.unwrap().unwrap();
    assert_eq!(coordinator.lock_counts(), (0, 0));
}

#[tokio::test]
async fn an_accepted_run_whose_control_is_cancelled_while_it_waits_never_starts() {
    let gate = Gate::new();
    let model = ScriptedModel::gated(vec![], gate.clone());
    let (coordinator, agent_id) = coordinator_with(model.clone()).await;
    let coordinator = coordinator.with_max_runs_per_agent(1);
    add_chat(&coordinator, &agent_id, "chat:b").await;
    let holding = {
        let coordinator = coordinator.clone();
        let request = chat_request(&agent_id, "room-a", "hold");
        tokio::spawn(async move { coordinator.run(request).await })
    };
    gate.entered().await;

    let waiting = accept_web(&coordinator, &agent_id, "chat:b", "key-b").await;
    // Most likely waiting for the only slot by now; either way it must never start.
    tokio::time::sleep(Duration::from_millis(20)).await;
    let control = coordinator
        .state
        .read()
        .await
        .live
        .runs()
        .control(&waiting)
        .unwrap();
    control.cancel.cancel();

    wait_for(&coordinator, &waiting, RunStatus::Cancelled).await;
    {
        let guard = coordinator.state.read().await;
        let record = guard.runs.get(&waiting).unwrap();
        assert_eq!(record.error.as_ref().unwrap().code, "stopped");
        assert_eq!(record.started_at_ms, None);
        assert!(guard.live.runs().control(&waiting).is_none());
    }
    gate.release();
    holding.await.unwrap().unwrap();
    assert_eq!(
        model.requests().len(),
        1,
        "the stopped run never reached the model"
    );
    assert_eq!(coordinator.lock_counts(), (0, 0));
}

#[tokio::test]
async fn legacy_and_telegram_owner_waits_share_one_budget_that_accepted_runs_do_not_use() {
    let gate = Gate::new();
    let (coordinator, agent_id) =
        coordinator_with(ScriptedModel::gated(vec![], gate.clone())).await;
    add_chat(&coordinator, &agent_id, "chat:accepted").await;
    let holding = {
        let coordinator = coordinator.clone();
        let request = chat_request(&agent_id, "room-x", "hold");
        tokio::spawn(async move { coordinator.run(request).await })
    };
    gate.entered().await;

    let mut waiting = Vec::new();
    for n in 0..MAX_QUEUED_RUNS_PER_AGENT {
        let unit = coordinator
            .try_take_waiting_unit(&agent_id)
            .expect("within the budget");
        let coordinator = coordinator.clone();
        let request = chat_request(&agent_id, "room-x", &format!("wait {n}"));
        waiting.push(tokio::spawn(async move {
            if n % 2 == 0 {
                // The legacy run route.
                coordinator.run_budgeted(request, unit).await.map(|_| ())
            } else {
                // A connector owner send.
                coordinator
                    .run_budgeted_with_commit_waiting(request, unit, |_, _| Ok(()), |_| Ok(()))
                    .await
                    .map(|_| ())
            }
        }));
    }
    assert!(
        coordinator.try_take_waiting_unit(&agent_id).is_none(),
        "a ninth waiter of either kind is refused"
    );
    let accepted = accept_web(&coordinator, &agent_id, "chat:accepted", "key-1").await;

    for _ in 0..(MAX_QUEUED_RUNS_PER_AGENT + 2) {
        gate.release();
    }
    holding.await.unwrap().unwrap();
    for task in waiting {
        task.await.unwrap().unwrap();
    }
    wait_for(&coordinator, &accepted, RunStatus::Completed).await;
    assert_eq!(coordinator.waiting_runs(&agent_id), 0);
}

#[tokio::test]
async fn queued_runs_count_as_active_and_a_restart_interrupts_them_as_never_started() {
    let gate = Gate::new();
    let (coordinator, agent_id) =
        coordinator_with(ScriptedModel::gated(vec![], gate.clone())).await;
    add_chat(&coordinator, &agent_id, "chat:q").await;
    let first = accept_web(&coordinator, &agent_id, "chat:q", "key-1").await;
    gate.entered().await;
    let second = accept_web(&coordinator, &agent_id, "chat:q", "key-2").await;

    let snapshot = {
        let guard = coordinator.state.read().await;
        assert_eq!(guard.runs.get(&second).unwrap().status, RunStatus::Queued);
        assert!(guard
            .runs
            .active_sessions()
            .contains(&(agent_id.clone(), "chat:q".to_string())));
        guard.control_plane_snapshot()
    };
    let restored = RunLedger::restored(
        snapshot.runs,
        &HashSet::from([agent_id.clone()]),
        anima_core::primitives::now_millis(),
    );
    let never_started = restored.get(&second).unwrap();
    assert_eq!(never_started.status, RunStatus::Interrupted);
    assert_eq!(
        never_started.error.as_ref().unwrap().code,
        "restart_before_start"
    );
    assert_eq!(
        restored.get(&first).unwrap().error.as_ref().unwrap().code,
        "restart_during_run"
    );

    gate.release();
    gate.entered().await;
    gate.release();
    wait_for(&coordinator, &second, RunStatus::Completed).await;
}

#[tokio::test]
async fn a_failed_acceptance_save_answers_503_and_leaves_nothing_behind() {
    let (coordinator, agent_id) = coordinator_with(ScriptedModel::new(vec![])).await;
    coordinator
        .state
        .write()
        .await
        .sessions
        .insert(SessionRecord::new(
            &agent_id,
            "chat:new",
            SessionKind::Chat,
            SessionOrigin::Web,
            DEFAULT_CHAT_TITLE.into(),
            TitleSource::FirstMessage,
            1,
        ));
    let save_gate = coordinator
        .state
        .write()
        .await
        .install_test_control_plane_save_gate(true);
    save_gate.release.add_permits(1);

    let start = coordinator.web_start(
        agent_id.clone(),
        "chat:new".into(),
        "Plan the offsite".into(),
        "key-1".into(),
    );
    let mut request = accept(&agent_id, "chat:new", "key-1");
    request.text = "Plan the offsite".into();
    let error = coordinator.accept_run(request, start).await.unwrap_err();

    assert_eq!(error.status(), StatusCode::SERVICE_UNAVAILABLE);
    let guard = coordinator.state.read().await;
    assert_eq!(guard.runs.queued_count(&agent_id), 0);
    assert_eq!(
        guard.sessions.get(&agent_id, "chat:new").unwrap().title,
        DEFAULT_CHAT_TITLE,
        "the acceptance title is reverted with the run"
    );
}

/// Fix round 1 (review Minor 5): accepted runs of two sessions of one agent
/// run at once, within the agent's slots; only one session's runs queue.
#[tokio::test]
async fn two_sessions_of_one_agent_run_their_messages_in_parallel() {
    let gate = Gate::new();
    let model = ScriptedModel::gated(vec![], gate.clone());
    let (coordinator, agent_id) = coordinator_with(model.clone()).await;
    let coordinator = coordinator.with_max_runs_per_agent(2);
    add_chat(&coordinator, &agent_id, "chat:one").await;
    add_chat(&coordinator, &agent_id, "chat:two").await;

    let one = accept_web(&coordinator, &agent_id, "chat:one", "key-one").await;
    let two = accept_web(&coordinator, &agent_id, "chat:two", "key-two").await;
    // Both calls are held at once: neither waited for the other.
    gate.entered().await;
    gate.entered().await;
    {
        let guard = coordinator.state.read().await;
        assert_eq!(guard.runs.get(&one).unwrap().status, RunStatus::Running);
        assert_eq!(guard.runs.get(&two).unwrap().status, RunStatus::Running);
    }
    let mut texts: Vec<String> = model
        .requests()
        .iter()
        .map(|request| request.messages.last().unwrap().content.text.clone())
        .collect();
    texts.sort();
    assert_eq!(texts, ["key-one", "key-two"]);

    gate.release();
    gate.release();
    wait_for(&coordinator, &one, RunStatus::Completed).await;
    wait_for(&coordinator, &two, RunStatus::Completed).await;
    assert_eq!(coordinator.lock_counts(), (0, 0));
}

/// Fix round 1 (review Important 1): a stream opened while the acceptance
/// save is in flight lists the queued run in its snapshot, so a failed save
/// must end it there, and a reverted title is announced too.
#[tokio::test]
async fn a_stream_that_saw_a_run_whose_acceptance_save_failed_is_told_it_ended() {
    let (coordinator, agent_id) = coordinator_with(ScriptedModel::new(vec![])).await;
    coordinator
        .state
        .write()
        .await
        .sessions
        .insert(SessionRecord::new(
            &agent_id,
            "chat:new",
            SessionKind::Chat,
            SessionOrigin::Web,
            DEFAULT_CHAT_TITLE.into(),
            TitleSource::FirstMessage,
            1,
        ));
    let save_gate = coordinator
        .state
        .write()
        .await
        .install_test_control_plane_save_gate(true);
    let accepting = {
        let coordinator = coordinator.clone();
        let start = coordinator.web_start(
            agent_id.clone(),
            "chat:new".into(),
            "Plan the offsite".into(),
            "key-1".into(),
        );
        let mut request = accept(&agent_id, "chat:new", "key-1");
        request.text = "Plan the offsite".into();
        tokio::spawn(async move { coordinator.accept_run(request, start).await })
    };
    tokio::time::timeout(Duration::from_secs(5), save_gate.entered.acquire())
        .await
        .expect("the acceptance save starts within five seconds")
        .unwrap()
        .forget();

    // A stream opens during the save, as the events route does it:
    // subscribe and snapshot under one read lock.
    let (mut subscription, snapshot) = {
        let guard = coordinator.state.read().await;
        (
            guard.live.subscribe(&agent_id).unwrap(),
            guard.live_snapshot_runs(&agent_id),
        )
    };
    assert_eq!(snapshot.len(), 1, "the stream saw the queued run");
    assert_eq!(snapshot[0].record.status, RunStatus::Queued);
    let run_id = snapshot[0].record.id.clone();
    save_gate.release.add_permits(1);

    let error = tokio::time::timeout(Duration::from_secs(5), accepting)
        .await
        .expect("acceptance ends within five seconds")
        .unwrap()
        .unwrap_err();
    assert_eq!(error.status(), StatusCode::SERVICE_UNAVAILABLE);
    let events = [
        next_event(&mut subscription).await.to_json(1),
        next_event(&mut subscription).await.to_json(2),
    ];
    assert_eq!(events[0]["type"], "run.failed");
    assert_eq!(events[0]["runId"], run_id.as_str());
    assert_eq!(events[0]["run"]["status"], "failed");
    assert_eq!(events[0]["run"]["error"]["code"], "commit_failed");
    assert_eq!(
        events[0]["run"]["error"]["message"],
        "injected control-plane save failure"
    );
    assert_eq!(events[1]["type"], "session.updated", "the title went back");
    assert_eq!(events[1]["sessionId"], "chat:new");
    assert!(
        quiet_for(&mut subscription).await.is_empty(),
        "one terminal event only"
    );
    let guard = coordinator.state.read().await;
    assert!(guard.runs.get(&run_id).is_none());
    assert!(guard.live.runs().control(&run_id).is_none());
    assert_eq!(
        guard.sessions.get(&agent_id, "chat:new").unwrap().title,
        DEFAULT_CHAT_TITLE
    );
}

/// A session's next accepted run starts once a stopped one is settled, and
/// each accepted run has its own control (M3 Task 2 carry-forward), so
/// stopping one never stops the next.
#[tokio::test]
async fn a_stopped_queued_run_is_skipped_and_the_next_one_still_runs() {
    let gate = Gate::new();
    let model = ScriptedModel::gated(vec![], gate.clone());
    let (coordinator, agent_id) = coordinator_with(model.clone()).await;
    add_chat(&coordinator, &agent_id, "chat:s").await;
    let first = accept_web(&coordinator, &agent_id, "chat:s", "key-1").await;
    gate.entered().await;
    let stopped = accept_web(&coordinator, &agent_id, "chat:s", "key-2").await;
    let next = accept_web(&coordinator, &agent_id, "chat:s", "key-3").await;
    let (stopped_control, next_control) = {
        let guard = coordinator.state.read().await;
        (
            guard.live.runs().control(&stopped).unwrap(),
            guard.live.runs().control(&next).unwrap(),
        )
    };
    stopped_control.cancel.cancel();
    assert!(
        !next_control.cancel.is_cancelled(),
        "every run has its own control"
    );

    gate.release();
    wait_for(&coordinator, &first, RunStatus::Completed).await;
    wait_for(&coordinator, &stopped, RunStatus::Cancelled).await;
    gate.entered().await;
    gate.release();
    wait_for(&coordinator, &next, RunStatus::Completed).await;

    let texts: Vec<String> = model
        .requests()
        .iter()
        .map(|request| request.messages.last().unwrap().content.text.clone())
        .collect();
    assert_eq!(texts, ["key-1", "key-3"], "the stopped run never ran");
    let guard = coordinator.state.read().await;
    let record = guard.runs.get(&stopped).unwrap();
    assert_eq!(record.error.as_ref().unwrap().code, "stopped");
    assert_eq!(record.error.as_ref().unwrap().message, "Stopped by owner");
    assert_eq!(record.started_at_ms, None);
}

fn crash() -> Result<(), super::QueuedStartError> {
    panic!("the start crashed before its run started")
}

/// A start that panics before its run starts is settled as failed, and the
/// session's queue goes on to its next run.
#[tokio::test]
async fn a_start_that_panics_is_settled_and_the_session_queue_continues() {
    let (coordinator, agent_id) = coordinator_with(ScriptedModel::new(vec![])).await;
    add_chat(&coordinator, &agent_id, "chat:p").await;
    let panicking: QueuedRunStart = Box::new(|_| Box::pin(async { crash() }));
    let crashed = match coordinator
        .accept_run(accept(&agent_id, "chat:p", "key-1"), panicking)
        .await
        .unwrap()
    {
        AcceptedRun::Created(record) => record.id,
        other => panic!("expected a new run, got {other:?}"),
    };
    let next = accept_web(&coordinator, &agent_id, "chat:p", "key-2").await;

    wait_for(&coordinator, &crashed, RunStatus::Failed).await;
    wait_for(&coordinator, &next, RunStatus::Completed).await;
    let guard = coordinator.state.read().await;
    let error = guard.runs.get(&crashed).unwrap().error.clone().unwrap();
    assert_eq!(error.code, "run_failed");
    assert_eq!(
        error.message,
        "The run stopped unexpectedly before it started"
    );
    assert!(guard.live.runs().control(&crashed).is_none());
}

/// A queued record the coordinator is asked to start: `status` in the ledger,
/// its control registered (and cancelled when `cancelled`).
async fn seeded_accepted_run(
    coordinator: &AgentRunCoordinator,
    agent_id: &str,
    session_id: &str,
    status: RunStatus,
    cancelled: bool,
) -> String {
    let mut record = RunRecord::queued(
        RunStart {
            agent_id: agent_id.into(),
            session_id: session_id.into(),
            source: RunSource::Web,
            source_ref: None,
            idempotency_key: Some("seeded".into()),
            text: "seeded".into(),
            model: "gpt-5.4".into(),
            provider: None,
            parent_run_id: None,
        },
        1,
    );
    record.status = status;
    let run_id = record.id.clone();
    let mut guard = coordinator.state.write().await;
    guard.runs.insert(record);
    let control = guard.live.runs().register(&run_id);
    if cancelled {
        control.cancel.cancel();
    }
    run_id
}

fn seeded_request(agent_id: &str, room_id: &str) -> super::AgentRunRequest {
    let mut request = chat_request(agent_id, room_id, "seeded");
    request.source = RunSource::Web;
    request.idempotency_key = Some("seeded".into());
    request
}

#[tokio::test]
async fn an_accepted_run_that_is_not_waiting_or_was_stopped_is_refused_before_it_waits() {
    let model = ScriptedModel::new(vec![]);
    let (coordinator, agent_id) = coordinator_with(model.clone()).await;
    add_chat(&coordinator, &agent_id, "chat:r").await;

    // No registered control: nothing accepted this run.
    let unknown = coordinator
        .run_accepted(seeded_request(&agent_id, "chat:r"), "run_unknown".into())
        .await
        .unwrap_err();
    assert_eq!(unknown.status(), StatusCode::CONFLICT);
    assert_eq!(unknown.message(), RUN_NOT_QUEUED);
    assert_eq!(unknown.message(), "This run is no longer waiting to start");

    // Stopped before its turn came.
    let stopped =
        seeded_accepted_run(&coordinator, &agent_id, "chat:r", RunStatus::Queued, true).await;
    let refused = coordinator
        .run_accepted(seeded_request(&agent_id, "chat:r"), stopped.clone())
        .await
        .unwrap_err();
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    assert_eq!(refused.message(), RUN_STOPPED_BEFORE_START);
    assert_eq!(refused.message(), "This run was stopped before it started");
    assert_eq!(
        coordinator
            .state
            .read()
            .await
            .runs
            .get(&stopped)
            .unwrap()
            .status,
        RunStatus::Queued,
        "the session queue settles it, not the refused start"
    );
    assert!(model.requests().is_empty());
    assert_eq!(coordinator.lock_counts(), (0, 0));
}

/// Controller ruling (M3 pre-flight audit M6): the accepted record is looked
/// up before Phase A touches any session state, so a run that is no longer
/// queued leaves no session behind.
#[tokio::test]
async fn a_start_whose_record_left_the_queue_changes_no_session_state() {
    let model = ScriptedModel::new(vec![]);
    let (coordinator, agent_id) = coordinator_with(model.clone()).await;
    let settled = seeded_accepted_run(
        &coordinator,
        &agent_id,
        "chat:unseen",
        RunStatus::Cancelled,
        false,
    )
    .await;

    let refused = coordinator
        .run_accepted(seeded_request(&agent_id, "chat:unseen"), settled.clone())
        .await
        .unwrap_err();

    assert_eq!(refused.status(), StatusCode::CONFLICT);
    assert_eq!(refused.message(), RUN_NOT_QUEUED);
    let guard = coordinator.state.read().await;
    assert!(
        guard.sessions.get(&agent_id, "chat:unseen").is_none(),
        "no session was created for a run that never started"
    );
    assert_eq!(
        guard.runs.get(&settled).unwrap().status,
        RunStatus::Cancelled
    );
    assert!(model.requests().is_empty());
}

/// Task 11 ruling (audit M6): the trimmed indicator is marked only after
/// the accepted-record check, so a refused start leaves a stale indicator
/// as it was, although this empty session's selection would clear it.
#[tokio::test]
async fn a_start_whose_record_left_the_queue_keeps_the_trimmed_indicator() {
    let model = ScriptedModel::new(vec![]);
    let (coordinator, agent_id) = coordinator_with(model.clone()).await;
    add_chat(&coordinator, &agent_id, "chat:kept").await;
    let stale = crate::sessions::SessionContextTrimmed {
        dropped_through_message_id: "gone".into(),
        at_ms: 1,
    };
    coordinator
        .state
        .write()
        .await
        .sessions
        .get_mut(&agent_id, "chat:kept")
        .unwrap()
        .context_trimmed = Some(stale.clone());
    let settled = seeded_accepted_run(
        &coordinator,
        &agent_id,
        "chat:kept",
        RunStatus::Completed,
        false,
    )
    .await;

    let refused = coordinator
        .run_accepted(seeded_request(&agent_id, "chat:kept"), settled)
        .await
        .unwrap_err();

    assert_eq!(refused.message(), RUN_NOT_QUEUED);
    assert_eq!(
        coordinator
            .state
            .read()
            .await
            .sessions
            .get(&agent_id, "chat:kept")
            .unwrap()
            .context_trimmed,
        Some(stale)
    );
    assert!(model.requests().is_empty());
}

/// An accepted run whose start save fails is settled once, as never
/// started, so its streams hear exactly one terminal event, matching the
/// ledger.
#[tokio::test]
async fn an_accepted_run_whose_start_save_fails_ends_with_one_terminal_event() {
    let model = ScriptedModel::new(vec![]);
    let (coordinator, agent_id) = coordinator_with(model.clone()).await;
    add_chat(&coordinator, &agent_id, "chat:f").await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();
    // Holds the session's room, so the run waits after acceptance.
    let reservation = coordinator
        .try_reserve_room(&agent_id, "chat:f")
        .expect("the room is free");

    let run_id = accept_web(&coordinator, &agent_id, "chat:f", "key-1").await;
    let save_gate = coordinator
        .state
        .write()
        .await
        .install_test_control_plane_save_gate(true);
    save_gate.release.add_permits(1);
    drop(reservation);

    let mut events: Vec<Value> = Vec::new();
    while events.iter().filter(|event| is_terminal(event)).count() == 0 {
        events.push(next_event(&mut subscription).await.to_json(0));
    }
    let quiet = quiet_for(&mut subscription).await;
    assert!(
        quiet.iter().all(|event| !is_terminal(event)),
        "one terminal event only, got {quiet:?}"
    );
    let types: Vec<&str> = events
        .iter()
        .map(|event| event["type"].as_str().unwrap())
        .collect();
    assert_eq!(types, ["run.queued", "run.failed"]);

    let guard = coordinator.state.read().await;
    let record = guard.runs.get(&run_id).unwrap();
    assert_eq!(record.status, RunStatus::Failed);
    assert_eq!(record.started_at_ms, None, "it never started durably");
    assert_eq!(record.error.as_ref().unwrap().code, "commit_failed");
    assert_eq!(events[1]["run"]["error"]["code"], "commit_failed");
    assert!(guard.live.runs().control(&run_id).is_none());
    assert!(model.requests().is_empty());
}

#[tokio::test]
async fn an_agent_being_deleted_refuses_new_messages() {
    let (coordinator, agent_id) = coordinator_with(ScriptedModel::new(vec![])).await;
    add_chat(&coordinator, &agent_id, "chat:d").await;

    let deleting = coordinator.begin_agent_deletion(&agent_id);
    let start = coordinator.web_start(agent_id.clone(), "chat:d".into(), "k1".into(), "k1".into());
    let error = coordinator
        .accept_run(accept(&agent_id, "chat:d", "k1"), start)
        .await
        .unwrap_err();
    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert_eq!(error.message(), "This companion is being deleted");
    assert!(coordinator.is_being_deleted(&agent_id));

    drop(deleting);
    assert!(!coordinator.is_being_deleted(&agent_id));
    accept_web(&coordinator, &agent_id, "chat:d", "k2").await;
}
