use anima_core::execution::Step;
use anima_core::{
    Budget, BudgetDecision, CheckpointV1, CommandOutcome, CommandReceipt, ExecutionErrorCode,
    ManifestPin, Run, RunPauseReason, RunState, RuntimeCommand, RuntimeCommandKind, RuntimeEvent,
    RuntimeEventKind, Session, SessionConcurrencyPolicy, StepKind, Usage,
};
use uuid::Uuid;

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

#[test]
fn run_transitions_are_explicit_and_terminal_states_are_immutable() {
    let run = Run::queued(id(1), id(2), "writer", 3).unwrap();
    let running = run.transition(RunState::Running, None).unwrap();
    for target in [
        RunState::WaitingForApproval,
        RunState::Paused,
        RunState::Completed,
        RunState::Failed,
        RunState::Cancelled,
    ] {
        assert!(running
            .transition(
                target,
                if target == RunState::Paused {
                    Some(RunPauseReason::Requested)
                } else {
                    None
                }
            )
            .is_ok());
    }
    assert!(run.transition(RunState::Completed, None).is_err());
    for terminal in [RunState::Completed, RunState::Failed, RunState::Cancelled] {
        let terminal = running.transition(terminal, None).unwrap();
        assert!(terminal.transition(RunState::Running, None).is_err());
    }
}

#[test]
fn resume_requires_approval_or_recovery_and_control_applies_at_safe_boundary() {
    let running = Run::queued(id(1), id(2), "writer", 3)
        .unwrap()
        .transition(RunState::Running, None)
        .unwrap();
    let waiting = running
        .transition(RunState::WaitingForApproval, None)
        .unwrap();
    assert!(waiting.resume(None, None).is_err());
    assert!(waiting.resume_with_claim(id(10), id(11)).is_ok());
    let recovery = running
        .transition(RunState::Paused, Some(RunPauseReason::RecoveryRequired))
        .unwrap();
    assert!(recovery.resume(None, None).is_err());
    assert!(recovery.resume_with_recovery(id(12)).is_ok());
    assert_eq!(
        running
            .request_pause_or_cancel(false, true, false)
            .unwrap()
            .state(),
        RunState::Running
    );
    assert_eq!(
        running
            .request_pause_or_cancel(false, true, true)
            .unwrap()
            .state(),
        RunState::Paused
    );
    assert_eq!(
        running
            .request_pause_or_cancel(true, false, true)
            .unwrap()
            .state(),
        RunState::Cancelled
    );
}

#[test]
fn attempts_are_append_only_for_one_logical_invocation() {
    let step = Step::new(id(1), "step-a", StepKind::Capability).unwrap();
    let first = step.start_attempt(id(2)).unwrap();
    let retried = first.retry().unwrap();
    assert_eq!(first.number(), 1);
    assert_eq!(retried.number(), 2);
    assert_ne!(first.id(), retried.id());
    assert_eq!(
        first.logical_invocation_id(),
        retried.logical_invocation_id()
    );
}

#[test]
fn sessions_default_serial_and_concurrent_requires_pinned_definition() {
    assert_eq!(
        SessionConcurrencyPolicy::default(),
        SessionConcurrencyPolicy::Serial
    );
    assert!(Session::new(id(1), "writer", 3, SessionConcurrencyPolicy::Concurrent).is_err());
    assert!(Session::new_with_definition_setting(id(1), "writer", 3, true).is_ok());
}

#[test]
fn events_are_contiguous_and_token_deltas_are_live_not_checkpoint_semantic() {
    let start = RuntimeEvent::new(
        id(9),
        id(8),
        id(7),
        id(1),
        10,
        4,
        RuntimeEventKind::RunStarted,
    )
    .unwrap();
    assert!(RuntimeEvent::validate_batch(4, &[start.clone()]).is_ok());
    assert!(RuntimeEvent::validate_batch(4, &[start.clone(), start.clone()]).is_err());
    assert!(RuntimeEvent::validate_batch(1, &[start]).is_err());
}

#[test]
fn command_receipts_are_idempotent_only_for_same_canonical_payload() {
    let command = RuntimeCommand::start(id(1), id(2), id(3)).unwrap();
    let receipt = CommandReceipt::accepted(&command).unwrap();
    assert_eq!(receipt.replay(&command).unwrap(), CommandOutcome::Accepted);
    let conflicting = RuntimeCommand::pause(id(1), id(2), id(3)).unwrap();
    assert!(receipt.replay(&conflicting).is_err());
    assert_eq!(command.kind(), RuntimeCommandKind::Start);
}

#[test]
fn checkpoints_round_trip_and_fail_closed_for_mismatch_or_secrets() {
    let checkpoint = CheckpointV1::new_minimal(
        id(1),
        id(2),
        "writer",
        3,
        4,
        vec![ManifestPin::new("cap", 1, "sha256:abc").unwrap()],
    )
    .unwrap();
    let encoded = serde_json::to_string(&checkpoint).unwrap();
    let restored: CheckpointV1 = serde_json::from_str(&encoded).unwrap();
    assert_eq!(restored, checkpoint);
    assert!(encoded.contains("schema_version"));
    let mut value = serde_json::to_value(&checkpoint).unwrap();
    value["runtime_schema_version"] = serde_json::json!(99);
    assert!(serde_json::from_value::<CheckpointV1>(value).is_err());
    assert!(ManifestPin::new("cap", 1, "token-value").is_err());
}

#[test]
fn budget_usage_is_checked_and_policy_driven() {
    let budget = Budget {
        max_wall_time_ms: Some(10),
        max_turns: Some(2),
        max_capability_steps: Some(3),
        max_input_tokens: Some(4),
        max_output_tokens: Some(5),
        max_total_tokens: Some(8),
        max_estimated_cost_micros: Some(9),
        max_concurrent_runs: Some(1),
        max_artifact_bytes: Some(10),
        max_download_bytes: Some(11),
        require_approval_at_percent: Some(80),
    };
    let usage = Usage {
        wall_time_ms: 8,
        turns: 1,
        capability_steps: 1,
        input_tokens: 3,
        output_tokens: 3,
        total_tokens: 6,
        estimated_cost_micros: 7,
        concurrent_runs: 1,
        artifact_bytes: 2,
        download_bytes: 3,
    };
    assert_eq!(
        budget.evaluate(&usage).unwrap(),
        BudgetDecision::RequireApproval
    );
    assert_eq!(
        budget
            .accumulate(
                &usage,
                &Usage {
                    input_tokens: 3,
                    total_tokens: 3,
                    ..Usage::default()
                }
            )
            .unwrap_err()
            .code(),
        ExecutionErrorCode::BudgetExceeded
    );
    assert_eq!(
        Usage {
            wall_time_ms: u64::MAX,
            ..Usage::default()
        }
        .checked_add(&Usage {
            wall_time_ms: 1,
            ..Usage::default()
        })
        .unwrap_err()
        .code(),
        ExecutionErrorCode::ArithmeticOverflow
    );
}

#[test]
fn standalone_budget_and_usage_serde_revalidate_invariants() {
    let mut usage = serde_json::to_value(Usage::default()).unwrap();
    usage["total_tokens"] = serde_json::json!(1);
    assert!(serde_json::from_value::<Usage>(usage).is_err());
    let mut budget = serde_json::to_value(Budget {
        max_wall_time_ms: Some(1),
        max_turns: None,
        max_capability_steps: None,
        max_input_tokens: None,
        max_output_tokens: None,
        max_total_tokens: None,
        max_estimated_cost_micros: None,
        max_concurrent_runs: None,
        max_artifact_bytes: None,
        max_download_bytes: None,
        require_approval_at_percent: None,
    })
    .unwrap();
    budget["max_wall_time_ms"] = serde_json::json!(0);
    assert!(serde_json::from_value::<Budget>(budget).is_err());
}
