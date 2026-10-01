//! Stopping a run that awaits approval (spec §4.6), waits that are
//! abandoned, and steers that wait for the next model call (spec §4.7).

use std::time::Duration;

use anima_core::{CancelSignal, MessageRole, CANCELLED_TOOL_RESULT};
use axum::http::StatusCode;

use super::test_support::{
    accept, accept_web, add_chat, approving_coordinator, ask_before_writes, chat_request, decision,
    events_until, gated_context, ledger_run, patient, pending_approvals, remember_call, tool_input,
    tool_results, wait_for, Gate, ScriptedModel, Step,
};
use super::{AcceptRun, AcceptedRun, AgentRunCoordinator, SessionRunMode};
use crate::approvals::{
    ApprovalDecisionKind, ApprovalRequest, ApprovalStatus, ResolvedBy, APPROVAL_ALREADY_RESOLVED,
};
use crate::routes::ApiError;
use crate::runs::{RunLink, RunStatus};
use crate::state::ApprovalAsk;

/// Waits (up to five seconds) until approval `id` is no longer pending.
async fn settled(coordinator: &AgentRunCoordinator, id: &str) -> ApprovalRequest {
    for _ in 0..500 {
        {
            let guard = coordinator.state.read().await;
            let approval = guard.approvals.get(id).unwrap();
            if !approval.is_pending() {
                return approval.clone();
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("approval {id} never settled");
}

fn assert_stopped(approval: &ApprovalRequest) {
    assert_eq!(approval.status, ApprovalStatus::Stopped);
    assert_eq!(
        approval.resolution.as_ref().unwrap().resolved_by,
        ResolvedBy::Stop
    );
}

#[tokio::test]
async fn stopping_a_run_that_awaits_approval_resolves_it_as_stopped() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![remember_call("call-1", "the plan")]),
        Step::Text(vec!["never sent"]),
    ]);
    let (coordinator, agent_id) =
        approving_coordinator(model.clone(), ask_before_writes(), patient()).await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();
    let running = {
        let coordinator = coordinator.clone();
        let request = chat_request(&agent_id, "chat:stop", "remember");
        tokio::spawn(async move { coordinator.run(request).await })
    };
    let pending = pending_approvals(&coordinator, 1).await.remove(0);

    let stopped = coordinator
        .stop_run(&agent_id, &pending.run_id)
        .await
        .unwrap();
    assert!(stopped.stop.is_some());
    assert_stopped(
        coordinator
            .state
            .read()
            .await
            .approvals
            .get(&pending.id)
            .unwrap(),
    );

    let envelope = running.await.unwrap().unwrap();
    assert_eq!(envelope.result.error.as_deref(), Some("stopped"));
    let results = tool_results(&coordinator, &agent_id).await;
    assert_eq!(results.len(), 1);
    assert!(results[0].contains(CANCELLED_TOOL_RESULT), "{}", results[0]);
    assert_eq!(
        model.requests().len(),
        1,
        "a stopped run makes no more calls"
    );
    assert_eq!(
        coordinator
            .state
            .read()
            .await
            .runs
            .get(&pending.run_id)
            .unwrap()
            .status,
        RunStatus::Cancelled
    );
    let late = coordinator
        .decide_approval(&pending.id, decision(ApprovalDecisionKind::AllowOnce, 1))
        .await
        .unwrap_err();
    assert_eq!(late.status(), StatusCode::CONFLICT);
    assert_eq!(late.message(), APPROVAL_ALREADY_RESOLVED);

    let events = events_until(&mut subscription, "run.cancelled").await;
    let resolved_at = events
        .iter()
        .position(|event| event["type"] == "approval.resolved")
        .expect("the stop announces the approval");
    let resolved = &events[resolved_at];
    assert_eq!(resolved["approval"]["status"], "stopped");
    assert_eq!(resolved["approval"]["resolution"]["resolvedBy"], "stop");
    let after = &events[resolved_at + 1..];
    assert!(
        !after
            .iter()
            .any(|event| event["type"] == "approval.resolved"),
        "announced once"
    );
    assert!(
        !after.iter().any(|event| event["type"] == "run.started"),
        "a stopped run never flickers back to running"
    );
    assert_eq!(coordinator.approval_waiters.len(), 0);
}

#[tokio::test]
async fn a_stop_that_cannot_be_saved_leaves_the_approval_waiting() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![remember_call("call-1", "the plan")]),
        Step::Text(vec!["ok"]),
    ]);
    let (coordinator, agent_id) =
        approving_coordinator(model, ask_before_writes(), patient()).await;
    let running = {
        let coordinator = coordinator.clone();
        let request = chat_request(&agent_id, "chat:stop", "remember");
        tokio::spawn(async move { coordinator.run(request).await })
    };
    let pending = pending_approvals(&coordinator, 1).await.remove(0);
    let save = coordinator
        .state
        .write()
        .await
        .install_test_control_plane_save_gate(true);
    let stopping = {
        let coordinator = coordinator.clone();
        let (agent_id, run_id) = (agent_id.clone(), pending.run_id.clone());
        tokio::spawn(async move { coordinator.stop_run(&agent_id, &run_id).await })
    };
    tokio::time::timeout(Duration::from_secs(5), save.entered.acquire())
        .await
        .expect("the stop saves")
        .unwrap()
        .forget();
    save.release.add_permits(1);

    let error = stopping.await.unwrap().unwrap_err();
    assert_eq!(error.status(), StatusCode::SERVICE_UNAVAILABLE);
    {
        let guard = coordinator.state.read().await;
        assert!(guard.approvals.get(&pending.id).unwrap().is_pending());
        let run = guard.runs.get(&pending.run_id).unwrap();
        assert_eq!(run.status, RunStatus::AwaitingApproval);
        assert_eq!(run.stop, None);
    }

    coordinator
        .decide_approval(&pending.id, decision(ApprovalDecisionKind::AllowOnce, 1))
        .await
        .unwrap();
    running.await.unwrap().unwrap();
    assert_eq!(
        tool_results(&coordinator, &agent_id).await,
        ["stored memory: the plan"]
    );
}

#[tokio::test]
async fn a_cancel_without_a_saved_stop_still_resolves_the_approval() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![remember_call("call-1", "the plan")]),
        Step::Text(vec!["never sent"]),
    ]);
    let (coordinator, agent_id) =
        approving_coordinator(model, ask_before_writes(), patient()).await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();
    let running = {
        let coordinator = coordinator.clone();
        let request = chat_request(&agent_id, "chat:cancel", "remember");
        tokio::spawn(async move { coordinator.run(request).await })
    };
    let pending = pending_approvals(&coordinator, 1).await.remove(0);

    // What a helper's deadline or a shutdown does: the signal, no saved stop.
    coordinator
        .state
        .read()
        .await
        .live
        .runs()
        .control(&pending.run_id)
        .expect("the run is live")
        .cancel
        .cancel();

    let envelope = running.await.unwrap().unwrap();
    assert_eq!(envelope.result.error.as_deref(), Some("stopped"));
    assert_stopped(
        coordinator
            .state
            .read()
            .await
            .approvals
            .get(&pending.id)
            .unwrap(),
    );
    assert!(tool_results(&coordinator, &agent_id).await[0].contains(CANCELLED_TOOL_RESULT));

    let events = events_until(&mut subscription, "run.cancelled").await;
    let resolved_at = events
        .iter()
        .position(|event| event["type"] == "approval.resolved")
        .expect("the cancel announces the approval");
    assert_eq!(events[resolved_at]["approval"]["status"], "stopped");
    assert!(
        !events[resolved_at + 1..]
            .iter()
            .any(|event| event["type"] == "run.started"),
        "a stopped settlement never flickers the run back to running"
    );
}

#[tokio::test]
async fn an_abandoned_wait_resolves_its_approval_as_stopped() {
    let (coordinator, agent_id) =
        approving_coordinator(ScriptedModel::new(vec![]), ask_before_writes(), patient()).await;
    let agent = coordinator
        .state
        .read()
        .await
        .get_agent(&agent_id)
        .unwrap()
        .state;
    let run = ledger_run(&coordinator, &agent_id, "chat:abandon").await;
    let context = gated_context(&coordinator, &run, CancelSignal::new()).await;
    let input = tool_input(&agent_id, "chat:abandon");
    let calling = tokio::spawn(async move {
        context
            .execute_tool(agent, input, remember_call("call-1", "x"))
            .await
    });
    let pending = pending_approvals(&coordinator, 1).await.remove(0);

    calling.abort();
    let _ = calling.await;

    assert_stopped(&settled(&coordinator, &pending.id).await);
    wait_for(&coordinator, &run.run_id, RunStatus::Running).await;
    assert_eq!(coordinator.approval_waiters.len(), 0, "no waiter is left");
}

#[tokio::test]
async fn a_request_dropped_while_it_saves_ends_stopped() {
    let (coordinator, agent_id) =
        approving_coordinator(ScriptedModel::new(vec![]), ask_before_writes(), patient()).await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();
    let agent = coordinator
        .state
        .read()
        .await
        .get_agent(&agent_id)
        .unwrap()
        .state;
    let run = ledger_run(&coordinator, &agent_id, "chat:dropped").await;
    let context = gated_context(&coordinator, &run, CancelSignal::new()).await;
    let input = tool_input(&agent_id, "chat:dropped");
    let save = coordinator
        .state
        .write()
        .await
        .install_test_control_plane_save_gate(false);
    let calling = tokio::spawn(async move {
        context
            .execute_tool(agent, input, remember_call("call-1", "x"))
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), save.entered.acquire())
        .await
        .expect("the request saves")
        .unwrap()
        .forget();
    // The request exists, its save has not finished, and nothing announced it.
    let id = coordinator
        .state
        .read()
        .await
        .approvals
        .pending_ids_for_run(&run.run_id)
        .remove(0);

    calling.abort();
    let _ = calling.await;

    assert_stopped(&settled(&coordinator, &id).await);
    wait_for(&coordinator, &run.run_id, RunStatus::Running).await;
    assert_eq!(coordinator.approval_waiters.len(), 0, "no waiter is left");
    let events = events_until(&mut subscription, "approval.resolved").await;
    assert!(
        !events
            .iter()
            .any(|event| event["type"] == "approval.requested"),
        "an unsaved request is never announced as waiting"
    );
}

/// What a gate whose future died mid-wait leaves behind (controller ruling
/// m2): a pending request of `agent_id`'s one in-flight run that no call
/// waits for, and no guard settles.
async fn leave_a_request(coordinator: &AgentRunCoordinator, agent_id: &str) -> ApprovalRequest {
    let mut guard = coordinator.state.write().await;
    let run = guard
        .runs
        .active_records()
        .into_iter()
        .find(|record| record.agent_id == agent_id)
        .expect("the run is in flight")
        .clone();
    let ask = ApprovalAsk {
        run: RunLink {
            run_id: run.id,
            session_id: run.session_id,
            agent_id: agent_id.into(),
        },
        call: remember_call("call-gone", "x"),
        timeout_ms: 60_000,
    };
    guard
        .open_approval(&ask, anima_core::primitives::now_millis())
        .unwrap()
        .approval
}

#[tokio::test]
async fn a_run_whose_gate_was_dropped_ends_with_its_approval_stopped() {
    let gate = Gate::new();
    let model = ScriptedModel::gated(vec![Step::Panic("the run task dies")], gate.clone());
    let (coordinator, agent_id) =
        approving_coordinator(model, ask_before_writes(), patient()).await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();
    let running = {
        let coordinator = coordinator.clone();
        let request = chat_request(&agent_id, "chat:aborted", "remember");
        tokio::spawn(async move { coordinator.run(request).await })
    };
    gate.entered().await;
    let left = leave_a_request(&coordinator, &agent_id).await;

    // The run's task unwinds, as one whose gate future was dropped does.
    gate.release();
    assert!(running.await.unwrap().is_err());

    assert_stopped(&settled(&coordinator, &left.id).await);
    wait_for(&coordinator, &left.run_id, RunStatus::Failed).await;
    let late = coordinator
        .decide_approval(&left.id, decision(ApprovalDecisionKind::AllowOnce, 1))
        .await
        .unwrap_err();
    assert_eq!(late.status(), StatusCode::CONFLICT);
    assert_eq!(late.message(), APPROVAL_ALREADY_RESOLVED);
    let events = events_until(&mut subscription, "run.failed").await;
    let resolved = events
        .iter()
        .filter(|event| event["type"] == "approval.resolved")
        .collect::<Vec<_>>();
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0]["approval"]["status"], "stopped");
}

#[tokio::test]
async fn a_finished_run_saves_its_orphaned_approvals_as_stopped() {
    let gate = Gate::new();
    let model = ScriptedModel::gated(vec![Step::Text(vec!["done"])], gate.clone());
    let (coordinator, agent_id) =
        approving_coordinator(model, ask_before_writes(), patient()).await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();
    let running = {
        let coordinator = coordinator.clone();
        let request = chat_request(&agent_id, "chat:orphan", "remember");
        tokio::spawn(async move { coordinator.run(request).await })
    };
    gate.entered().await;
    let left = leave_a_request(&coordinator, &agent_id).await;
    let mut woken = coordinator.approval_waiters.register(&left.id);

    gate.release();
    running.await.unwrap().unwrap();

    let approval = coordinator
        .state
        .read()
        .await
        .approvals
        .get(&left.id)
        .unwrap()
        .clone();
    assert_stopped(&approval);
    assert_eq!(
        woken.try_recv().unwrap().status,
        ApprovalStatus::Stopped,
        "a waiter still registered hears it"
    );
    assert_eq!(coordinator.approval_waiters.len(), 0);
    let events = events_until(&mut subscription, "run.completed").await;
    let types = events
        .iter()
        .map(|event| event["type"].as_str().unwrap())
        .collect::<Vec<_>>();
    let resolved_at = types
        .iter()
        .position(|kind| *kind == "approval.resolved")
        .expect("announced after the save");
    assert!(
        resolved_at < types.len() - 1,
        "before the run's terminal event"
    );
}

#[tokio::test]
async fn a_rolled_back_run_still_stops_its_orphaned_approvals() {
    let gate = Gate::new();
    let model = ScriptedModel::gated(vec![Step::Text(vec!["done"])], gate.clone());
    let (coordinator, agent_id) =
        approving_coordinator(model, ask_before_writes(), patient()).await;
    let running = {
        let coordinator = coordinator.clone();
        let request = chat_request(&agent_id, "chat:rejected", "remember");
        tokio::spawn(async move {
            coordinator
                .run_with_commit(request, |_, _| {
                    Err(ApiError::conflict("the source refused the commit"))
                })
                .await
        })
    };
    gate.entered().await;
    let left = leave_a_request(&coordinator, &agent_id).await;

    gate.release();
    assert!(running.await.unwrap().is_err());

    assert_stopped(
        coordinator
            .state
            .read()
            .await
            .approvals
            .get(&left.id)
            .unwrap(),
    );
    assert_eq!(
        coordinator
            .state
            .read()
            .await
            .runs
            .get(&left.run_id)
            .unwrap()
            .status,
        RunStatus::Failed
    );
}

#[tokio::test]
async fn a_steer_sent_while_awaiting_approval_waits_for_the_next_model_call() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![remember_call("call-1", "the plan")]),
        Step::Text(vec!["Saved, and noted"]),
    ]);
    let (coordinator, agent_id) =
        approving_coordinator(model.clone(), ask_before_writes(), patient()).await;
    add_chat(&coordinator, &agent_id, "chat:s").await;
    let first = accept_web(&coordinator, &agent_id, "chat:s", "remember the plan").await;
    let pending = pending_approvals(&coordinator, 1).await.remove(0);
    assert_eq!(pending.run_id, first);

    let steer = AcceptRun {
        mode: SessionRunMode::Steer,
        text: "also note the date".into(),
        ..accept(&agent_id, "chat:s", "key-steer")
    };
    let start = coordinator.web_start(
        steer.agent_id.clone(),
        steer.session_id.clone(),
        steer.text.clone(),
        steer.idempotency_key.clone(),
    );
    let AcceptedRun::Steered(joined) = coordinator.accept_run(steer, start).await.unwrap() else {
        panic!("the steer joins the run that awaits approval");
    };
    assert_eq!(joined.id, first);
    assert_eq!(joined.status, RunStatus::AwaitingApproval);

    coordinator
        .decide_approval(&pending.id, decision(ApprovalDecisionKind::AllowOnce, 1))
        .await
        .unwrap();
    wait_for(&coordinator, &first, RunStatus::Completed).await;

    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    assert!(!requests[0]
        .messages
        .iter()
        .any(|message| message.content.text == "also note the date"));
    let second = &requests[1].messages;
    let result_at = second
        .iter()
        .position(|message| {
            message.role == MessageRole::Tool && message.content.text.contains("stored memory")
        })
        .expect("the tool ran");
    let steer_at = second
        .iter()
        .position(|message| {
            message.role == MessageRole::User && message.content.text == "also note the date"
        })
        .expect("the steer joined");
    assert!(
        steer_at > result_at,
        "the steer comes after the tool's result"
    );
}
