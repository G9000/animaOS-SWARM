//! Approvals in coordinator runs (spec §7.3): the gate, the waiting call,
//! decisions, timeouts, and helpers.

use std::time::Duration;

use anima_core::{AgentConfigUpdate, CancelSignal, MessageRole};
use axum::http::StatusCode;
use serde_json::{json, Value};

use super::test_support::{
    approving_coordinator, ask_before_writes, calculate_call, chat_request, companion_config,
    decision, events_until, gated_context, lead_config, ledger_run, patient, pending_approvals,
    quiet_for, remember_call, tool_input, tool_results, Gate, ScriptedModel, Step,
};
use super::AgentRunCoordinator;
use crate::approvals::{
    ApprovalDecisionKind, ApprovalMatcher, ApprovalPolicy, ApprovalRule, ApprovalStatus,
    ApprovalTimeouts, MatcherKind, PolicyAction, ResolvedBy, RiskClass, Verdict,
    APPROVAL_ALREADY_RESOLVED, APPROVAL_NOT_SAVED, APPROVAL_REVISION_STALE, APPROVAL_TIMEOUT_MS,
    DENIED_BY_POLICY, HELPER_NEEDS_APPROVAL, TELEGRAM_APPROVAL_TIMEOUT_MS,
};
use crate::runs::{RunSource, RunStatus};
use crate::state::{OwnerDecision, Settlement};

fn quick() -> ApprovalTimeouts {
    ApprovalTimeouts {
        default: Duration::from_millis(250),
        telegram: Duration::from_millis(250),
    }
}

fn types(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .map(|event| event["type"].as_str().unwrap().to_string())
        .collect()
}

fn spawn_run(
    coordinator: &AgentRunCoordinator,
    request: super::AgentRunRequest,
) -> tokio::task::JoinHandle<Result<crate::routes::AgentRunEnvelope, crate::routes::ApiError>> {
    let coordinator = coordinator.clone();
    tokio::spawn(async move { coordinator.run(request).await })
}

/// The one tool result `agent_id` committed.
async fn only_result(coordinator: &AgentRunCoordinator, agent_id: &str) -> String {
    let mut results = tool_results(coordinator, agent_id).await;
    assert_eq!(results.len(), 1, "{results:?}");
    results.remove(0)
}

#[tokio::test]
async fn a_denied_class_never_runs_and_the_model_hears_why() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![remember_call("call-1", "the plan")]),
        Step::Text(vec!["Understood"]),
    ]);
    let deny_writes = ApprovalPolicy::default().with(RiskClass::Write, PolicyAction::Deny);
    let (coordinator, agent_id) =
        approving_coordinator(model.clone(), deny_writes, patient()).await;

    coordinator
        .run(chat_request(&agent_id, "chat:ask", "remember the plan"))
        .await
        .unwrap();

    assert!(only_result(&coordinator, &agent_id)
        .await
        .contains(DENIED_BY_POLICY));
    assert!(coordinator
        .state
        .read()
        .await
        .approvals
        .pending()
        .is_empty());
    assert!(model.requests()[1].messages.iter().any(|message| {
        message.role == MessageRole::Tool && message.content.text.contains(DENIED_BY_POLICY)
    }));
}

#[tokio::test]
async fn read_tools_never_ask_even_when_every_class_is_denied() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![calculate_call("call-1", "2+2")]),
        Step::Text(vec!["Four"]),
    ]);
    let deny_all = ApprovalPolicy {
        write: PolicyAction::Deny,
        exec: PolicyAction::Deny,
        network: PolicyAction::Deny,
        delegate: PolicyAction::Deny,
    };
    let (coordinator, agent_id) = approving_coordinator(model, deny_all, patient()).await;

    coordinator
        .run(chat_request(&agent_id, "chat:math", "add"))
        .await
        .unwrap();

    let result = only_result(&coordinator, &agent_id).await;
    assert!(result.contains('4'), "{result}");
    assert!(!result.contains(DENIED_BY_POLICY));
}

#[tokio::test]
async fn an_asked_call_waits_for_the_owner_and_runs_once_allowed() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![remember_call("call-1", "the plan")]),
        Step::Text(vec!["Saved"]),
    ]);
    let (coordinator, agent_id) =
        approving_coordinator(model.clone(), ask_before_writes(), patient()).await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();
    let running = spawn_run(
        &coordinator,
        chat_request(&agent_id, "chat:ask", "remember"),
    );

    let asked = events_until(&mut subscription, "approval.requested").await;
    let asked_types = types(&asked);
    let awaiting = asked_types
        .iter()
        .position(|kind| kind == "run.awaiting_approval")
        .expect("the run awaits approval");
    assert_eq!(
        awaiting,
        asked_types.len() - 2,
        "then the request is announced"
    );
    assert!(asked_types[..awaiting].contains(&"tool.started".to_string()));
    assert_eq!(asked[awaiting]["run"]["status"], "awaiting_approval");
    let announced = &asked[asked.len() - 1];
    assert_eq!(announced["sessionId"], "chat:ask");
    let approval = &announced["approval"];
    assert_eq!(approval["tool"], "memory_add");
    assert_eq!(approval["class"], "write");
    assert_eq!(approval["status"], "pending");
    assert_eq!(approval["revision"], 1);
    assert_eq!(approval["toolCallId"], "call-1");
    assert_eq!(approval["arguments"], "{\"content\":\"the plan\"}");
    assert_eq!(approval["argumentsTruncated"], false);
    assert_eq!(approval["matcherKinds"], json!(["any"]));
    assert_eq!(
        approval["suggestedMatcher"],
        json!({"kind": "any", "value": ""})
    );
    assert_eq!(approval["resolution"], Value::Null);

    let pending = pending_approvals(&coordinator, 1).await.remove(0);
    assert_eq!(pending.expires_at_ms - pending.created_at_ms, 30_000);
    assert_eq!(
        coordinator
            .state
            .read()
            .await
            .runs
            .get(&pending.run_id)
            .unwrap()
            .status,
        RunStatus::AwaitingApproval
    );

    let allowed = coordinator
        .decide_approval(&pending.id, decision(ApprovalDecisionKind::AllowOnce, 1))
        .await
        .unwrap();
    assert_eq!(allowed.status, ApprovalStatus::Allowed);
    let envelope = running.await.unwrap().unwrap();
    assert_eq!(envelope.result.error, None);
    assert_eq!(
        tool_results(&coordinator, &agent_id).await,
        ["stored memory: the plan"]
    );

    let after = events_until(&mut subscription, "run.completed").await;
    let after_types = types(&after);
    let resolved = after_types
        .iter()
        .position(|kind| kind == "approval.resolved")
        .unwrap();
    assert_eq!(after[resolved]["approval"]["status"], "allowed");
    assert_eq!(
        after[resolved]["approval"]["resolution"]["decision"],
        "allow_once"
    );
    assert_eq!(after_types[resolved + 1], "run.started", "the run resumes");
    assert_eq!(after[resolved + 1]["run"]["status"], "running");
    assert_eq!(
        after_types
            .iter()
            .filter(|kind| *kind == "approval.resolved")
            .count(),
        1
    );
    assert_eq!(model.requests().len(), 2);
}

#[tokio::test]
async fn a_denial_reaches_the_model_with_the_owners_note() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![remember_call("call-1", "the plan")]),
        Step::Text(vec!["I won't"]),
    ]);
    let (coordinator, agent_id) =
        approving_coordinator(model.clone(), ask_before_writes(), patient()).await;
    let running = spawn_run(
        &coordinator,
        chat_request(&agent_id, "chat:ask", "remember"),
    );
    let pending = pending_approvals(&coordinator, 1).await.remove(0);

    coordinator
        .decide_approval(
            &pending.id,
            OwnerDecision {
                kind: ApprovalDecisionKind::Deny,
                note: Some("not now".into()),
                matcher: None,
                revision: 1,
            },
        )
        .await
        .unwrap();
    running.await.unwrap().unwrap();

    assert!(only_result(&coordinator, &agent_id)
        .await
        .contains("Denied by owner: not now"));
    assert!(model.requests()[1].messages.iter().any(|message| {
        message.role == MessageRole::Tool
            && message.content.text.contains("Denied by owner: not now")
    }));
}

#[tokio::test]
async fn allow_for_the_session_skips_the_next_ask_there_and_only_there() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![remember_call("call-1", "one")]),
        Step::Text(vec!["ok"]),
        Step::Tools(vec![remember_call("call-2", "two")]),
        Step::Text(vec!["ok"]),
        Step::Tools(vec![remember_call("call-3", "three")]),
        Step::Text(vec!["ok"]),
    ]);
    let (coordinator, agent_id) =
        approving_coordinator(model, ask_before_writes(), patient()).await;

    let first = spawn_run(&coordinator, chat_request(&agent_id, "chat:one", "first"));
    let pending = pending_approvals(&coordinator, 1).await.remove(0);
    coordinator
        .decide_approval(&pending.id, decision(ApprovalDecisionKind::AllowSession, 1))
        .await
        .unwrap();
    first.await.unwrap().unwrap();

    tokio::time::timeout(
        Duration::from_secs(5),
        coordinator.run(chat_request(&agent_id, "chat:one", "second")),
    )
    .await
    .expect("the session's allowance covers the second call")
    .unwrap();

    let third = spawn_run(&coordinator, chat_request(&agent_id, "chat:two", "third"));
    let asked = pending_approvals(&coordinator, 1).await.remove(0);
    assert_eq!(asked.session_id, "chat:two", "another session still asks");
    coordinator
        .decide_approval(&asked.id, decision(ApprovalDecisionKind::Deny, 1))
        .await
        .unwrap();
    third.await.unwrap().unwrap();
}

#[tokio::test]
async fn always_allow_creates_a_rule_that_later_runs_follow() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![remember_call("call-1", "one")]),
        Step::Text(vec!["ok"]),
        Step::Tools(vec![remember_call("call-2", "two")]),
        Step::Text(vec!["ok"]),
    ]);
    let (coordinator, agent_id) =
        approving_coordinator(model, ask_before_writes(), patient()).await;

    let first = spawn_run(&coordinator, chat_request(&agent_id, "chat:one", "first"));
    let pending = pending_approvals(&coordinator, 1).await.remove(0);
    let always = coordinator
        .decide_approval(&pending.id, decision(ApprovalDecisionKind::AllowAlways, 1))
        .await
        .unwrap();
    first.await.unwrap().unwrap();
    {
        let guard = coordinator.state.read().await;
        let rules = guard.approvals.rules_for(&agent_id);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].tool, "memory_add");
        assert_eq!(rules[0].matcher, ApprovalMatcher::any());
        assert_eq!(
            rules[0].from_approval_id.as_deref(),
            Some(pending.id.as_str())
        );
        assert_eq!(
            always.resolution.unwrap().rule_id.as_deref(),
            Some(rules[0].id.as_str())
        );
    }

    tokio::time::timeout(
        Duration::from_secs(5),
        coordinator.run(chat_request(&agent_id, "chat:two", "second")),
    )
    .await
    .expect("the rule covers every session")
    .unwrap();
}

#[tokio::test]
async fn a_timeout_denies_the_call_and_a_late_decision_conflicts() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![remember_call("call-1", "the plan")]),
        Step::Text(vec!["ok"]),
    ]);
    let (coordinator, agent_id) = approving_coordinator(model, ask_before_writes(), quick()).await;

    tokio::time::timeout(
        Duration::from_secs(5),
        coordinator.run(chat_request(&agent_id, "chat:ask", "remember")),
    )
    .await
    .expect("the timeout ends the wait")
    .unwrap();

    assert!(only_result(&coordinator, &agent_id)
        .await
        .contains("Denied by owner: Approval timed out"));
    let timed_out = coordinator.state.read().await.approvals.decided()[0].clone();
    assert_eq!(timed_out.status, ApprovalStatus::Denied);
    assert_eq!(
        timed_out.resolution.as_ref().unwrap().resolved_by,
        ResolvedBy::Timeout
    );
    let late = coordinator
        .decide_approval(&timed_out.id, decision(ApprovalDecisionKind::AllowOnce, 1))
        .await
        .unwrap_err();
    assert_eq!(late.status(), StatusCode::CONFLICT);
    assert_eq!(late.message(), APPROVAL_ALREADY_RESOLVED);
}

#[tokio::test]
async fn telegram_started_runs_wait_fifteen_minutes_and_others_thirty() {
    assert_eq!(APPROVAL_TIMEOUT_MS, 30 * 60 * 1000);
    assert_eq!(TELEGRAM_APPROVAL_TIMEOUT_MS, 15 * 60 * 1000);
    assert_eq!(
        ApprovalTimeouts::default(),
        ApprovalTimeouts {
            default: Duration::from_millis(APPROVAL_TIMEOUT_MS),
            telegram: Duration::from_millis(TELEGRAM_APPROVAL_TIMEOUT_MS),
        }
    );
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![remember_call("call-1", "one")]),
        Step::Text(vec!["ok"]),
        Step::Tools(vec![remember_call("call-2", "two")]),
        Step::Text(vec!["ok"]),
    ]);
    let timeouts = ApprovalTimeouts {
        default: Duration::from_secs(60),
        telegram: Duration::from_secs(20),
    };
    let (coordinator, agent_id) = approving_coordinator(model, ask_before_writes(), timeouts).await;

    let mut telegram = chat_request(&agent_id, "chat:telegram", "from telegram");
    telegram.source = RunSource::Telegram;
    let running = spawn_run(&coordinator, telegram);
    let asked = pending_approvals(&coordinator, 1).await.remove(0);
    assert_eq!(asked.expires_at_ms - asked.created_at_ms, 20_000);
    coordinator
        .decide_approval(&asked.id, decision(ApprovalDecisionKind::Deny, 1))
        .await
        .unwrap();
    running.await.unwrap().unwrap();

    let running = spawn_run(
        &coordinator,
        chat_request(&agent_id, "chat:api", "from the api"),
    );
    let asked = pending_approvals(&coordinator, 1).await.remove(0);
    assert_eq!(asked.expires_at_ms - asked.created_at_ms, 60_000);
    coordinator
        .decide_approval(&asked.id, decision(ApprovalDecisionKind::Deny, 1))
        .await
        .unwrap();
    running.await.unwrap().unwrap();
}

/// Spawns the waiter's own timeout settlement of `id`, exactly as the
/// waiting call makes it when its deadline passes.
fn spawn_timeout(
    coordinator: &AgentRunCoordinator,
    id: &str,
) -> tokio::task::JoinHandle<Option<crate::approvals::ApprovalRequest>> {
    let coordinator = coordinator.clone();
    let id = id.to_string();
    tokio::spawn(async move { coordinator.settle_approval(&id, Settlement::TimedOut).await })
}

fn spawn_decision(
    coordinator: &AgentRunCoordinator,
    id: &str,
    kind: ApprovalDecisionKind,
) -> tokio::task::JoinHandle<Result<crate::approvals::ApprovalRequest, crate::routes::ApiError>> {
    let coordinator = coordinator.clone();
    let id = id.to_string();
    tokio::spawn(async move { coordinator.decide_approval(&id, decision(kind, 1)).await })
}

// The two races below run on one thread with timeouts that never fire: a
// spawned settlement runs, on `yield_now`, up to the held transaction and
// queues there (tokio's mutex is fair), so the queue order is the spawn order.

#[tokio::test(flavor = "current_thread")]
async fn a_timeout_that_queued_first_beats_a_later_decision() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![remember_call("call-1", "the plan")]),
        Step::Text(vec!["ok"]),
    ]);
    let (coordinator, agent_id) =
        approving_coordinator(model, ask_before_writes(), patient()).await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();
    let running = spawn_run(
        &coordinator,
        chat_request(&agent_id, "chat:race", "remember"),
    );
    let pending = pending_approvals(&coordinator, 1).await.remove(0);

    let held = coordinator.control_plane_transaction().await;
    let timing_out = spawn_timeout(&coordinator, &pending.id);
    tokio::task::yield_now().await;
    let deciding = spawn_decision(&coordinator, &pending.id, ApprovalDecisionKind::AllowOnce);
    tokio::task::yield_now().await;
    drop(held);

    let timed_out = timing_out.await.unwrap().expect("the timeout settles it");
    assert_eq!(timed_out.status, ApprovalStatus::Denied);
    assert_eq!(
        timed_out.resolution.as_ref().unwrap().resolved_by,
        ResolvedBy::Timeout
    );
    let refused = deciding.await.unwrap().unwrap_err();
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    assert_eq!(refused.message(), APPROVAL_ALREADY_RESOLVED);
    running.await.unwrap().unwrap();
    assert!(only_result(&coordinator, &agent_id)
        .await
        .contains("Denied by owner: Approval timed out"));
    let events = events_until(&mut subscription, "run.completed").await;
    assert_eq!(
        types(&events)
            .iter()
            .filter(|kind| *kind == "approval.resolved")
            .count(),
        1
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_decision_that_queued_first_beats_the_timeout() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![remember_call("call-1", "the plan")]),
        Step::Text(vec!["ok"]),
    ]);
    let (coordinator, agent_id) =
        approving_coordinator(model, ask_before_writes(), patient()).await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();
    let running = spawn_run(
        &coordinator,
        chat_request(&agent_id, "chat:race", "remember"),
    );
    let pending = pending_approvals(&coordinator, 1).await.remove(0);

    let held = coordinator.control_plane_transaction().await;
    let deciding = spawn_decision(&coordinator, &pending.id, ApprovalDecisionKind::AllowOnce);
    tokio::task::yield_now().await;
    let timing_out = spawn_timeout(&coordinator, &pending.id);
    tokio::task::yield_now().await;
    drop(held);

    let allowed = deciding.await.unwrap().unwrap();
    assert_eq!(allowed.status, ApprovalStatus::Allowed);
    let seen = timing_out
        .await
        .unwrap()
        .expect("the record is still there");
    assert_eq!(seen, allowed, "the timeout finds it already allowed");
    running.await.unwrap().unwrap();
    assert_eq!(
        tool_results(&coordinator, &agent_id).await,
        ["stored memory: the plan"]
    );
    let events = events_until(&mut subscription, "run.completed").await;
    assert_eq!(
        types(&events)
            .iter()
            .filter(|kind| *kind == "approval.resolved")
            .count(),
        1
    );
}

#[tokio::test]
async fn helpers_are_denied_instead_of_waiting() {
    let (coordinator, companion) =
        approving_coordinator(ScriptedModel::new(vec![]), ask_before_writes(), patient()).await;
    let helper = {
        let mut guard = coordinator.state.write().await;
        let parent = guard.get_agent(&companion).unwrap().state;
        guard
            .create_agent(super::helper_config(&parent, "Helper".into()))
            .unwrap()
            .state
    };
    let run = ledger_run(&coordinator, &helper.id, "room-helper").await;
    let context = gated_context(&coordinator, &run, CancelSignal::new()).await;

    let result = context
        .execute_tool(
            helper.clone(),
            tool_input(&helper.id, "room-helper"),
            remember_call("call-1", "x"),
        )
        .await;

    assert_eq!(result.error.as_deref(), Some(HELPER_NEEDS_APPROVAL));
    assert!(coordinator
        .state
        .read()
        .await
        .approvals
        .pending()
        .is_empty());
}

#[tokio::test]
async fn an_approved_call_is_checked_again_before_it_runs() {
    let (coordinator, _) = approving_coordinator(
        ScriptedModel::new(vec![]),
        ApprovalPolicy::default(),
        patient(),
    )
    .await;
    let (lead_id, specialist) = {
        let mut guard = coordinator.state.write().await;
        let mut lead = lead_config("Lead");
        lead.tools = Some(
            crate::tools::ToolRegistry::new()
                .resolve_descriptors(["memory_add"])
                .unwrap(),
        );
        let lead_id = guard.create_agent(lead).unwrap().state.id;
        let mut specialist = companion_config("Specialist");
        specialist.tools = lead_tools(&guard, &lead_id);
        let specialist = guard.create_agent(specialist).unwrap().state;
        guard
            .approvals
            .set_policy(&specialist.id, ask_before_writes());
        (lead_id, specialist)
    };
    let run = ledger_run(&coordinator, &specialist.id, "room-delegated").await;
    let context = gated_context(&coordinator, &run, CancelSignal::new())
        .await
        .with_delegated_parent(Some(lead_id.clone()));
    let calling = {
        let specialist = specialist.clone();
        tokio::spawn(async move {
            context
                .execute_tool(
                    specialist.clone(),
                    tool_input(&specialist.id, "room-delegated"),
                    remember_call("call-1", "x"),
                )
                .await
        })
    };
    let pending = pending_approvals(&coordinator, 1).await.remove(0);

    // While the owner decides, the manager loses the tool (spec §7.3: the
    // live checks run again after an approval).
    coordinator
        .state
        .write()
        .await
        .update_agent(
            &lead_id,
            AgentConfigUpdate {
                tools: Some(vec![]),
                ..Default::default()
            },
        )
        .unwrap();
    coordinator
        .decide_approval(&pending.id, decision(ApprovalDecisionKind::AllowOnce, 1))
        .await
        .unwrap();

    let result = calling.await.unwrap();
    assert_eq!(
        result.error.as_deref(),
        Some("The manager no longer has permission for this delegated tool")
    );
}

fn lead_tools(
    guard: &crate::state::DaemonState,
    lead_id: &str,
) -> Option<Vec<anima_core::ToolDescriptor>> {
    guard.get_agent(lead_id).unwrap().state.config.tools
}

#[tokio::test]
async fn a_request_that_cannot_be_saved_never_runs_its_tool() {
    let gate = Gate::new();
    let model = ScriptedModel::gated(
        vec![
            Step::Tools(vec![remember_call("call-1", "the plan")]),
            Step::Text(vec!["ok"]),
        ],
        gate.clone(),
    );
    let (coordinator, agent_id) =
        approving_coordinator(model, ask_before_writes(), patient()).await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();
    let running = spawn_run(
        &coordinator,
        chat_request(&agent_id, "chat:ask", "remember"),
    );
    // The run's start is saved; the next save is the request's, and it fails.
    gate.entered().await;
    let save = coordinator
        .state
        .write()
        .await
        .install_test_control_plane_save_gate(true);
    gate.release();
    tokio::time::timeout(Duration::from_secs(5), save.entered.acquire())
        .await
        .expect("the request is saved")
        .unwrap()
        .forget();
    save.release.add_permits(1);
    // The model is asked again, so the refused call is over: its run is back
    // at running.
    gate.entered().await;
    {
        let guard = coordinator.state.read().await;
        let runs = guard.runs.for_session(&agent_id, "chat:ask");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, RunStatus::Running);
    }
    gate.release();

    running.await.unwrap().unwrap();
    assert!(only_result(&coordinator, &agent_id)
        .await
        .contains(APPROVAL_NOT_SAVED));
    {
        let guard = coordinator.state.read().await;
        assert!(guard.approvals.pending().is_empty());
        assert!(guard.approvals.decided().is_empty());
    }
    let events = types(&events_until(&mut subscription, "run.completed").await);
    for unannounced in ["approval.requested", "run.awaiting_approval"] {
        assert!(!events.iter().any(|kind| kind == unannounced), "{events:?}");
    }
}

#[tokio::test]
async fn a_decision_that_cannot_be_saved_leaves_the_call_waiting() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![remember_call("call-1", "the plan")]),
        Step::Text(vec!["ok"]),
    ]);
    let (coordinator, agent_id) =
        approving_coordinator(model, ask_before_writes(), patient()).await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();
    let running = spawn_run(
        &coordinator,
        chat_request(&agent_id, "chat:ask", "remember"),
    );
    events_until(&mut subscription, "approval.requested").await;
    let pending = pending_approvals(&coordinator, 1).await.remove(0);

    let save = coordinator
        .state
        .write()
        .await
        .install_test_control_plane_save_gate(true);
    let deciding = spawn_decision(&coordinator, &pending.id, ApprovalDecisionKind::AllowAlways);
    tokio::time::timeout(Duration::from_secs(5), save.entered.acquire())
        .await
        .expect("the decision is saved")
        .unwrap()
        .forget();
    save.release.add_permits(1);
    let refused = deciding.await.unwrap().unwrap_err();
    assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);

    assert_still_waiting(&coordinator, &agent_id, &pending).await;
    assert_not_resolved(&mut subscription).await;

    // The failing save gate was for one save; the owner decides again.
    let allowed = coordinator
        .decide_approval(&pending.id, decision(ApprovalDecisionKind::AllowAlways, 1))
        .await
        .unwrap();
    assert_eq!(allowed.status, ApprovalStatus::Allowed);
    running.await.unwrap().unwrap();
    assert_eq!(
        tool_results(&coordinator, &agent_id).await,
        ["stored memory: the plan"]
    );
}

/// `pending` still waits at revision 1, no rule was made, and its run still
/// awaits approval.
async fn assert_still_waiting(
    coordinator: &AgentRunCoordinator,
    agent_id: &str,
    pending: &crate::approvals::ApprovalRequest,
) {
    let guard = coordinator.state.read().await;
    let still = guard.approvals.pending();
    assert_eq!(still.len(), 1);
    assert_eq!(still[0].id, pending.id);
    assert_eq!(still[0].status, ApprovalStatus::Pending);
    assert_eq!(still[0].revision, 1);
    assert_eq!(still[0].resolution, None);
    assert!(guard.approvals.rules_for(agent_id).is_empty());
    assert_eq!(
        guard.runs.get(&pending.run_id).unwrap().status,
        RunStatus::AwaitingApproval
    );
}

async fn assert_not_resolved(subscription: &mut crate::live::LiveSubscription) {
    let events = types(&quiet_for(subscription).await);
    assert!(
        !events.iter().any(|kind| kind == "approval.resolved"),
        "{events:?}"
    );
}

type RunHandle =
    tokio::task::JoinHandle<Result<crate::routes::AgentRunEnvelope, crate::routes::ApiError>>;

/// Starts the owner's allow, then drops its caller while the decision's save
/// is held, as a disconnecting HTTP client would; the save then fails or
/// succeeds. Returns once the decision has let go of the transaction.
async fn decide_and_walk_away(
    fail: bool,
) -> (
    AgentRunCoordinator,
    String,
    crate::approvals::ApprovalRequest,
    RunHandle,
    crate::live::LiveSubscription,
) {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![remember_call("call-1", "the plan")]),
        Step::Text(vec!["ok"]),
    ]);
    let (coordinator, agent_id) =
        approving_coordinator(model, ask_before_writes(), patient()).await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();
    let running = spawn_run(
        &coordinator,
        chat_request(&agent_id, "chat:ask", "remember"),
    );
    events_until(&mut subscription, "approval.requested").await;
    let pending = pending_approvals(&coordinator, 1).await.remove(0);

    let save = coordinator
        .state
        .write()
        .await
        .install_test_control_plane_save_gate(fail);
    let deciding = spawn_decision(&coordinator, &pending.id, ApprovalDecisionKind::AllowOnce);
    tokio::time::timeout(Duration::from_secs(5), save.entered.acquire())
        .await
        .expect("the decision is saved")
        .unwrap()
        .forget();
    deciding.abort();
    assert!(deciding.await.unwrap_err().is_cancelled());
    save.release.add_permits(1);
    drop(
        tokio::time::timeout(
            Duration::from_secs(5),
            coordinator.control_plane_transaction(),
        )
        .await
        .expect("the decision finishes on its own"),
    );
    (coordinator, agent_id, pending, running, subscription)
}

#[tokio::test]
async fn a_decision_whose_caller_leaves_and_whose_save_fails_is_taken_back() {
    let (coordinator, agent_id, pending, running, mut subscription) =
        decide_and_walk_away(true).await;

    assert_still_waiting(&coordinator, &agent_id, &pending).await;
    assert_not_resolved(&mut subscription).await;
    assert!(!running.is_finished());

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
async fn a_decision_whose_caller_leaves_is_still_saved_announced_and_woken() {
    let (coordinator, agent_id, pending, running, mut subscription) =
        decide_and_walk_away(false).await;

    let events = events_until(&mut subscription, "run.completed").await;
    let resolved = events
        .iter()
        .find(|event| event["type"] == "approval.resolved")
        .expect("the decision is announced");
    assert_eq!(resolved["approval"]["status"], "allowed");
    tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .expect("the waiting call is woken")
        .unwrap()
        .unwrap();
    assert_eq!(
        tool_results(&coordinator, &agent_id).await,
        ["stored memory: the plan"]
    );
    let replay = coordinator
        .decide_approval(&pending.id, decision(ApprovalDecisionKind::AllowOnce, 1))
        .await
        .unwrap();
    assert_eq!(replay.status, ApprovalStatus::Allowed, "it was kept");
}

#[tokio::test]
async fn a_lost_wake_up_is_not_a_timeout() {
    // A deadline no test machine reaches: the wait is ended by the stop.
    let timeouts = ApprovalTimeouts {
        default: Duration::from_secs(60),
        telegram: Duration::from_secs(60),
    };
    let (coordinator, agent_id) =
        approving_coordinator(ScriptedModel::new(vec![]), ask_before_writes(), timeouts).await;
    let agent = coordinator
        .state
        .read()
        .await
        .get_agent(&agent_id)
        .unwrap()
        .state;
    let run = ledger_run(&coordinator, &agent_id, "room-woken").await;
    let cancel = CancelSignal::new();
    let context = gated_context(&coordinator, &run, cancel.clone()).await;
    let calling = tokio::spawn(async move {
        let input = tool_input(&agent.id, "room-woken");
        context
            .execute_tool(agent, input, remember_call("call-1", "x"))
            .await
    });
    let pending = pending_approvals(&coordinator, 1).await.remove(0);

    // The wake-up is dropped unsent; the call keeps waiting rather than
    // settling the request as timed out.
    coordinator.forget_approval_waiter(&pending.id);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_still_waiting(&coordinator, &agent_id, &pending).await;
    assert!(!calling.is_finished());

    // The stop still ends the wait, and settles it as stopped.
    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(5), calling)
        .await
        .expect("the stop ends the wait")
        .unwrap();
    assert_eq!(
        result.error.as_deref(),
        Some(anima_core::CANCELLED_TOOL_RESULT)
    );
    let guard = coordinator.state.read().await;
    assert_eq!(
        guard.approvals.get(&pending.id).unwrap().status,
        ApprovalStatus::Stopped
    );
    assert!(guard.approvals.rules_for(&agent_id).is_empty());
}

#[tokio::test]
async fn a_tool_the_agent_lost_during_the_wait_does_not_run() {
    let (coordinator, agent_id) =
        approving_coordinator(ScriptedModel::new(vec![]), ask_before_writes(), patient()).await;
    let agent = coordinator
        .state
        .read()
        .await
        .get_agent(&agent_id)
        .unwrap()
        .state;
    let run = ledger_run(&coordinator, &agent_id, "room-lost").await;
    let context = gated_context(&coordinator, &run, CancelSignal::new()).await;
    let calling = tokio::spawn(async move {
        let input = tool_input(&agent.id, "room-lost");
        context
            .execute_tool(agent, input, remember_call("call-1", "x"))
            .await
    });
    let pending = pending_approvals(&coordinator, 1).await.remove(0);

    // While the call waits, the owner takes the tool away from the agent.
    coordinator
        .state
        .write()
        .await
        .update_agent(
            &agent_id,
            AgentConfigUpdate {
                tools: Some(
                    crate::tools::ToolRegistry::new()
                        .resolve_descriptors(["calculate"])
                        .unwrap(),
                ),
                ..Default::default()
            },
        )
        .unwrap();
    coordinator
        .decide_approval(&pending.id, decision(ApprovalDecisionKind::AllowOnce, 1))
        .await
        .unwrap();

    let result = calling.await.unwrap();
    assert_eq!(
        result.error.as_deref(),
        Some(anima_core::tool_not_configured_error("memory_add").as_str())
    );
}

#[tokio::test]
async fn an_approved_tool_the_run_added_to_the_agent_runs() {
    // `broadcast_message` is not in the companion's own configuration; every
    // run that is not delegated adds it.
    let broadcast = anima_core::ToolCall {
        id: "call-1".into(),
        name: "broadcast_message".into(),
        args: std::collections::BTreeMap::from([(
            "message".to_string(),
            anima_core::DataValue::String("hello".into()),
        )]),
    };
    let model = ScriptedModel::new(vec![Step::Tools(vec![broadcast]), Step::Text(vec!["Sent"])]);
    let ask_before_delegating =
        ApprovalPolicy::default().with(RiskClass::Delegate, PolicyAction::Ask);
    let (coordinator, agent_id) =
        approving_coordinator(model, ask_before_delegating, patient()).await;
    assert!(!coordinator.state.read().await.agents[&agent_id]
        .config()
        .allows_tool("broadcast_message"));
    let running = spawn_run(
        &coordinator,
        chat_request(&agent_id, "chat:ask", "say hello"),
    );
    let pending = pending_approvals(&coordinator, 1).await.remove(0);
    assert_eq!(pending.tool, "broadcast_message");

    coordinator
        .decide_approval(&pending.id, decision(ApprovalDecisionKind::AllowOnce, 1))
        .await
        .unwrap();

    running.await.unwrap().unwrap();
    assert_eq!(
        only_result(&coordinator, &agent_id).await,
        r#"{"deliveries":[]}"#
    );
}

#[tokio::test]
async fn an_agent_removed_during_the_wait_does_not_run_its_tool() {
    let (coordinator, agent_id) =
        approving_coordinator(ScriptedModel::new(vec![]), ask_before_writes(), patient()).await;
    let agent = coordinator
        .state
        .read()
        .await
        .get_agent(&agent_id)
        .unwrap()
        .state;
    let run = ledger_run(&coordinator, &agent_id, "room-removed").await;
    let context = gated_context(&coordinator, &run, CancelSignal::new()).await;
    let calling = tokio::spawn(async move {
        let input = tool_input(&agent.id, "room-removed");
        context
            .execute_tool(agent, input, remember_call("call-1", "x"))
            .await
    });
    let pending = pending_approvals(&coordinator, 1).await.remove(0);

    // While the call waits, the owner deletes the agent.
    coordinator.state.write().await.remove_agent(&agent_id);
    coordinator
        .decide_approval(&pending.id, decision(ApprovalDecisionKind::AllowOnce, 1))
        .await
        .unwrap();

    let result = calling.await.unwrap();
    assert_eq!(
        result.error.as_deref(),
        Some(anima_core::tool_not_configured_error("memory_add").as_str())
    );
}

#[tokio::test]
async fn repeating_a_decision_is_idempotent_and_a_different_one_conflicts() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![remember_call("call-1", "the plan")]),
        Step::Text(vec!["ok"]),
    ]);
    let (coordinator, agent_id) =
        approving_coordinator(model, ask_before_writes(), patient()).await;
    let running = spawn_run(
        &coordinator,
        chat_request(&agent_id, "chat:ask", "remember"),
    );
    let pending = pending_approvals(&coordinator, 1).await.remove(0);

    let stale = coordinator
        .decide_approval(&pending.id, decision(ApprovalDecisionKind::AllowOnce, 2))
        .await
        .unwrap_err();
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert_eq!(stale.message(), APPROVAL_REVISION_STALE);

    let first = coordinator
        .decide_approval(&pending.id, decision(ApprovalDecisionKind::AllowOnce, 1))
        .await
        .unwrap();
    let replay = coordinator
        .decide_approval(&pending.id, decision(ApprovalDecisionKind::AllowOnce, 1))
        .await
        .unwrap();
    assert_eq!(replay, first, "the same decision again changes nothing");
    let other = coordinator
        .decide_approval(&pending.id, decision(ApprovalDecisionKind::Deny, 1))
        .await
        .unwrap_err();
    assert_eq!(other.status(), StatusCode::CONFLICT);
    assert_eq!(other.message(), APPROVAL_ALREADY_RESOLVED);
    let missing = coordinator
        .decide_approval("apr_missing", decision(ApprovalDecisionKind::AllowOnce, 1))
        .await
        .unwrap_err();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    running.await.unwrap().unwrap();
}

#[tokio::test]
async fn two_calls_in_one_batch_wait_together_and_the_run_resumes_after_both() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![
            remember_call("call-1", "one"),
            remember_call("call-2", "two"),
        ]),
        Step::Text(vec!["both"]),
    ]);
    let (coordinator, agent_id) =
        approving_coordinator(model, ask_before_writes(), patient()).await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();
    let running = spawn_run(
        &coordinator,
        chat_request(&agent_id, "chat:batch", "remember both"),
    );
    let pending = pending_approvals(&coordinator, 2).await;

    coordinator
        .decide_approval(&pending[0].id, decision(ApprovalDecisionKind::AllowOnce, 1))
        .await
        .unwrap();
    assert_eq!(
        coordinator
            .state
            .read()
            .await
            .runs
            .get(&pending[0].run_id)
            .unwrap()
            .status,
        RunStatus::AwaitingApproval,
        "the other call still waits"
    );
    coordinator
        .decide_approval(&pending[1].id, decision(ApprovalDecisionKind::Deny, 1))
        .await
        .unwrap();
    running.await.unwrap().unwrap();

    let results = tool_results(&coordinator, &agent_id).await;
    assert_eq!(results.len(), 2);
    assert_eq!(
        results
            .iter()
            .filter(|text| text.starts_with("stored memory: "))
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|text| text.contains("Denied by owner"))
            .count(),
        1
    );
    let events = types(&events_until(&mut subscription, "run.completed").await);
    let count = |kind: &str| events.iter().filter(|event| *event == kind).count();
    assert_eq!(count("run.awaiting_approval"), 1);
    assert_eq!(count("approval.requested"), 2);
    assert_eq!(count("approval.resolved"), 2);
    assert_eq!(count("run.started"), 2, "the start, then one resume");
}

#[tokio::test]
async fn a_rule_never_covers_a_command_with_shell_operators() {
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![anima_core::ToolCall {
            id: "call-1".into(),
            name: "bash".into(),
            args: std::collections::BTreeMap::from([(
                "command".to_string(),
                anima_core::DataValue::String("git status; rm -rf ~".into()),
            )]),
        }]),
        Step::Text(vec!["ok"]),
    ]);
    let (coordinator, agent_id) =
        approving_coordinator(model, ApprovalPolicy::default(), patient()).await;
    {
        let mut guard = coordinator.state.write().await;
        guard
            .update_agent(
                &agent_id,
                AgentConfigUpdate {
                    tools: Some(
                        crate::tools::ToolRegistry::new()
                            .resolve_descriptors(["bash"])
                            .unwrap(),
                    ),
                    ..Default::default()
                },
            )
            .unwrap();
        guard
            .approvals
            .add_rule(ApprovalRule {
                id: "rule_git".into(),
                agent_id: agent_id.clone(),
                tool: "bash".into(),
                matcher: ApprovalMatcher {
                    kind: MatcherKind::CommandPrefix,
                    value: "git status".into(),
                },
                created_at_ms: 1,
                from_approval_id: None,
            })
            .unwrap();
        let agent = guard.get_agent(&agent_id).unwrap().state;
        let plain = anima_core::ToolCall {
            id: "call-0".into(),
            name: "bash".into(),
            args: std::collections::BTreeMap::from([(
                "command".to_string(),
                anima_core::DataValue::String("git status --short".into()),
            )]),
        };
        assert_eq!(
            guard.approval_verdict(&agent, "chat:shell", &plain),
            Verdict::Allow,
            "the rule covers the plain command"
        );
    }
    let running = spawn_run(
        &coordinator,
        chat_request(&agent_id, "chat:shell", "status"),
    );

    let asked = pending_approvals(&coordinator, 1).await.remove(0);
    assert_eq!(asked.tool, "bash");
    assert!(asked.arguments.contains("git status; rm -rf ~"));
    coordinator
        .decide_approval(&asked.id, decision(ApprovalDecisionKind::Deny, 1))
        .await
        .unwrap();
    running.await.unwrap().unwrap();
    assert!(only_result(&coordinator, &agent_id)
        .await
        .contains("Denied by owner"));
}
