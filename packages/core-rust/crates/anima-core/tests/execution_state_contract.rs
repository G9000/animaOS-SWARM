use anima_core::execution::Step;
use anima_core::{
    AgentDefinition, ApprovalDecision, ApprovalResumeClaim, AttemptRecordState, AutonomyGrant,
    Budget, BudgetDecision, CapabilityKind, CapabilityManifest, CapabilityReferenceId,
    CheckpointCursor, CheckpointV1, CheckpointV1Builder, CommandOutcome, CommandReceipt,
    CompletedInvocationRecord, DefinitionPin, ExecutionErrorCode, ExecutionLease, GrantConsumption,
    GrantEffect, GrantScope, GrantStatus, InvocationAttemptRecord, LifecyclePolicy,
    LogicalInvocation, ManifestCatalog, ManifestPin, MemoryPolicy, ModelPolicy, OpaqueReference,
    PendingApprovalRecord, PolicyContext, PolicyEngine, PolicyRestrictions, ProfileRef,
    RecoveryMode, RecoveryPauseReason, RecoveryPauseRecord, RecoveryTerminalResolution,
    ResolvedCapability, RiskLevel, Run, RunPauseReason, RunState, RuntimeCommand,
    RuntimeCommandKind, RuntimeCompatibility, RuntimeEvent, RuntimeEventKind, RuntimeLimits,
    SafeEventPayload, Session, SessionConcurrencyPolicy, StepKind, UncertainInvocationRecord,
    Usage,
};
use serde_json::json;
use uuid::Uuid;

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn session_definition(
    definition_id: &str,
    version: u32,
    allows_concurrent_sessions: bool,
) -> AgentDefinition {
    AgentDefinition {
        schema_version: 1,
        id: definition_id.into(),
        version,
        name: "writer".into(),
        display_name: "Writer".into(),
        description: "Writes".into(),
        persona: "Careful".into(),
        system: "Write carefully".into(),
        model: ModelPolicy {
            provider: "test".into(),
            model: "test".into(),
            credential_reference: None,
            temperature: None,
        },
        source_profile: ProfileRef {
            profile_id: "profile".into(),
            profile_version: 1,
        },
        resolved_capabilities: vec![],
        memory: MemoryPolicy {
            enabled: false,
            namespace: "session".into(),
            retention_days: None,
        },
        approval_policy_id: "policy".into(),
        approval_policy_revision: 1,
        approval_restrictions: vec![],
        limits: RuntimeLimits {
            max_turns: 1,
            timeout_ms: 1,
            max_concurrent_tasks: 1,
        },
        lifecycle: LifecyclePolicy {
            auto_start: false,
            restart_on_failure: false,
            max_restarts: 0,
            allows_concurrent_sessions,
        },
        host_requirements: vec![],
    }
}

fn approval_manifest(capability_id: &str, manifest_version: u32) -> CapabilityManifest {
    CapabilityManifest::new(anima_core::CapabilityManifestInput {
        id: capability_id.into(),
        version: manifest_version,
        kind: CapabilityKind::Workspace,
        label: "Write".into(),
        description: "Writes a workspace file".into(),
        input_schema: json!({ "type": "object" }),
        output_schema: json!({ "type": "object" }),
        side_effects: true,
        risk_level: RiskLevel::High,
        host_permissions: vec![],
        secret_references: vec![],
        environment_requirements: vec![],
        timeout_ms: 1_000,
        cancellation_supported: true,
        max_retries: 0,
        idempotent: false,
        recovery_mode: RecoveryMode::NonRetryable,
        supports_streaming: false,
        supports_artifacts: false,
        supports_citations: false,
        compatibility: RuntimeCompatibility {
            minimum_runtime_schema_version: 1,
            maximum_runtime_schema_version: 1,
            manifest_schema_version: 1,
        },
    })
    .unwrap()
}

fn approval_context_for(
    manifest: &CapabilityManifest,
    invocation: &LogicalInvocation,
) -> PolicyContext {
    PolicyContext::new(
        "owner",
        "actor",
        "writer",
        3,
        "workspace",
        CapabilityReferenceId::new(id(99)),
        manifest,
        invocation,
        1,
        PolicyRestrictions::default(),
        1_000,
    )
    .unwrap()
}

fn approval_context(arguments: serde_json::Value) -> PolicyContext {
    let manifest = approval_manifest("workspace.write", 1);
    let invocation =
        LogicalInvocation::new(id(1), "write", "workspace.write", 1, arguments).unwrap();
    approval_context_for(&manifest, &invocation)
}

#[test]
fn run_transitions_are_explicit_and_terminal_states_are_immutable() {
    let run = Run::queued(id(1), id(2), "writer", 3).unwrap();
    let running = run.transition(RunState::Running, None).unwrap();
    for target in [
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
fn generic_transitions_cannot_bypass_resume_authorization() {
    let running = Run::queued(id(1), id(2), "writer", 3)
        .unwrap()
        .transition(RunState::Running, None)
        .unwrap();
    let request = PolicyEngine::approval_request(
        &approval_context_for(
            &approval_manifest("workspace.write", 1),
            &LogicalInvocation::new(
                id(1),
                "write",
                "workspace.write",
                1,
                json!({ "path": "a.md" }),
            )
            .unwrap(),
        ),
        None,
    )
    .unwrap();
    let waiting = running.wait_for_approval(request).unwrap();
    assert!(waiting.transition(RunState::Running, None).is_err());

    let manually_paused = running
        .transition(RunState::Paused, Some(RunPauseReason::Requested))
        .unwrap();
    assert!(manually_paused.transition(RunState::Running, None).is_err());
    assert_eq!(
        manually_paused.resume(None, None).unwrap().state(),
        RunState::Running
    );

    for (mode, reason) in [
        (
            RecoveryMode::KeyedIdempotent,
            RecoveryPauseReason::Retryable,
        ),
        (RecoveryMode::Manual, RecoveryPauseReason::ManualReview),
        (
            RecoveryMode::NonRetryable,
            RecoveryPauseReason::ManualReview,
        ),
    ] {
        let invocation = LogicalInvocation::new(
            id(1),
            format!("pause-{mode:?}"),
            "workspace.write",
            1,
            json!({ "path": "a.md" }),
        )
        .unwrap();
        let pause = RecoveryPauseRecord::new(
            invocation.binding(),
            1,
            ManifestPin::new_with_recovery_mode(
                "workspace.write",
                1,
                format!("sha256:{mode:?}"),
                mode,
            )
            .unwrap(),
            reason,
        )
        .unwrap();
        let paused = running.pause_for_recovery(pause).unwrap();
        assert!(paused.transition(RunState::Running, None).is_err());
    }
}

#[test]
fn arbitrary_uuid_resume_claims_are_rejected_and_control_applies_at_safe_boundary() {
    let running = Run::queued(id(1), id(2), "writer", 3)
        .unwrap()
        .transition(RunState::Running, None)
        .unwrap();
    let context = approval_context(json!({ "path": "a.md" }));
    let waiting = running
        .wait_for_approval(PolicyEngine::approval_request(&context, None).unwrap())
        .unwrap();
    assert!(waiting.resume(None, None).is_err());
    assert!(waiting.resume(Some((id(10), id(11))), None).is_err());
    assert!(waiting.resume_with_claim(id(10), id(11)).is_err());
    assert!(running.resume_with_recovery(id(12)).is_err());
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
fn approval_resume_requires_the_exact_pending_request_command_and_live_claim() {
    let context = approval_context(json!({ "path": "a.md" }));
    let request = PolicyEngine::approval_request(&context, None).unwrap();
    let decision = ApprovalDecision::new_approved(request.clone(), 1_000).unwrap();
    let claim = ApprovalResumeClaim::new(&request, &decision, &context, &[]).unwrap();
    assert!(claim.grant_consumption_snapshot().is_none());
    let waiting = Run::queued(id(1), id(2), "writer", 3)
        .unwrap()
        .transition(RunState::Running, None)
        .unwrap()
        .wait_for_approval(request.clone())
        .unwrap();
    let command =
        RuntimeCommand::resume_with_approval(id(50), id(2), id(1), claim.binding().clone())
            .unwrap();

    assert!(waiting
        .apply_resume_command(&command, Some(&claim), None)
        .is_ok());
    assert!(waiting.apply_resume_command(&command, None, None).is_err());

    let other_context = approval_context(json!({ "path": "b.md" }));
    let other_request = PolicyEngine::approval_request(&other_context, None).unwrap();
    let other_decision = ApprovalDecision::new_approved(other_request.clone(), 1_000).unwrap();
    let other_claim =
        ApprovalResumeClaim::new(&other_request, &other_decision, &other_context, &[]).unwrap();
    let other_command =
        RuntimeCommand::resume_with_approval(id(51), id(2), id(1), other_claim.binding().clone())
            .unwrap();
    assert!(waiting
        .apply_resume_command(&other_command, Some(&other_claim), None)
        .is_err());
}

#[test]
fn approval_resume_claim_binds_live_grant_and_consumption_proposal() {
    let context = approval_context(json!({ "path": "a.md" }));
    let scope = GrantScope::new(
        context.owner_id.clone(),
        context.actor_id.clone(),
        context.agent_definition_id.clone(),
        context.agent_definition_version,
        context.workspace_id.clone(),
        context.resource_boundary.clone(),
        context.capability_id.clone(),
        context.manifest_version,
        Some(context.canonical_argument_digest),
    )
    .unwrap();
    let grant = AutonomyGrant::new_with_effect(
        "approval-grant",
        1,
        GrantStatus::Active,
        scope,
        RiskLevel::High,
        500,
        Some(2_000),
        Some(1),
        GrantEffect::ApprovalRequired,
    )
    .unwrap();
    let request = PolicyEngine::approval_request(&context, Some(&grant)).unwrap();
    let decision = ApprovalDecision::new_approved(request.clone(), 1_000).unwrap();
    let claim = ApprovalResumeClaim::new(&request, &decision, &context, &[grant.clone()]).unwrap();
    assert_eq!(
        claim.grant_consumption(),
        Some(&GrantConsumption::new("approval-grant", 1, context.logical_invocation_id).unwrap())
    );
    assert_eq!(
        claim.grant_consumption_snapshot().unwrap().remaining_uses(),
        1
    );

    let mut uncounted = grant.clone();
    uncounted.remaining_uses = None;
    assert!(
        ApprovalResumeClaim::new_with_grants(&request, &decision, &context, &[uncounted]).is_err()
    );
    let mut inflated = grant.clone();
    inflated.remaining_uses = Some(2);
    assert!(
        ApprovalResumeClaim::new_with_grants(&request, &decision, &context, &[inflated]).is_err()
    );
    let mut uncounted_grant = grant.clone();
    uncounted_grant.remaining_uses = None;
    let uncounted_request =
        PolicyEngine::approval_request(&context, Some(&uncounted_grant)).unwrap();
    let uncounted_decision =
        ApprovalDecision::new_approved(uncounted_request.clone(), 1_000).unwrap();
    let uncounted_claim = ApprovalResumeClaim::new_with_grants(
        &uncounted_request,
        &uncounted_decision,
        &context,
        &[uncounted_grant],
    )
    .unwrap();
    assert!(uncounted_claim.grant_consumption().is_none());
    assert!(uncounted_claim.grant_consumption_snapshot().is_none());

    assert!(ApprovalResumeClaim::new(&request, &decision, &context, &[]).is_err());
    let mut revoked = grant.clone();
    revoked.status = GrantStatus::Revoked;
    assert!(ApprovalResumeClaim::new(&request, &decision, &context, &[revoked]).is_err());
    let mut expired = grant.clone();
    expired.valid_until_ms = Some(1_000);
    assert!(ApprovalResumeClaim::new(&request, &decision, &context, &[expired]).is_err());
    let mut revised = grant;
    revised.revision = 2;
    assert!(ApprovalResumeClaim::new(&request, &decision, &context, &[revised]).is_err());
}

#[test]
fn approval_resume_command_surfaces_counted_grant_consumption() {
    let context = approval_context(json!({ "path": "a.md" }));
    let scope = GrantScope::new(
        context.owner_id.clone(),
        context.actor_id.clone(),
        context.agent_definition_id.clone(),
        context.agent_definition_version,
        context.workspace_id.clone(),
        context.resource_boundary.clone(),
        context.capability_id.clone(),
        context.manifest_version,
        Some(context.canonical_argument_digest),
    )
    .unwrap();
    let grant = AutonomyGrant::new_with_effect(
        "approval-grant",
        1,
        GrantStatus::Active,
        scope,
        RiskLevel::High,
        500,
        Some(2_000),
        Some(1),
        GrantEffect::ApprovalRequired,
    )
    .unwrap();
    let request = PolicyEngine::approval_request(&context, Some(&grant)).unwrap();
    let decision = ApprovalDecision::new_approved(request.clone(), 1_000).unwrap();
    let claim = ApprovalResumeClaim::new(&request, &decision, &context, &[grant]).unwrap();
    let waiting = Run::queued(id(1), id(2), "writer", 3)
        .unwrap()
        .transition(RunState::Running, None)
        .unwrap()
        .wait_for_approval(request)
        .unwrap();
    let command =
        RuntimeCommand::resume_with_approval(id(50), id(2), id(1), claim.binding().clone())
            .unwrap();

    let outcome = waiting
        .apply_resume_command(&command, Some(&claim), None)
        .unwrap();
    let expected =
        GrantConsumption::new("approval-grant", 1, context.logical_invocation_id).unwrap();
    assert_eq!(outcome.run().state(), RunState::Running);
    assert_eq!(outcome.grant_consumption(), Some(&expected));
    let (resumed, consumption) = outcome.into_parts();
    assert_eq!(resumed.state(), RunState::Running);
    assert_eq!(consumption, Some(expected));
}

#[test]
fn approval_resume_shortcut_returns_the_counted_grant_consumption() {
    let context = approval_context(json!({ "path": "a.md" }));
    let scope = GrantScope::new(
        context.owner_id.clone(),
        context.actor_id.clone(),
        context.agent_definition_id.clone(),
        context.agent_definition_version,
        context.workspace_id.clone(),
        context.resource_boundary.clone(),
        context.capability_id.clone(),
        context.manifest_version,
        Some(context.canonical_argument_digest),
    )
    .unwrap();
    let grant = AutonomyGrant::new_with_effect(
        "approval-grant",
        1,
        GrantStatus::Active,
        scope,
        RiskLevel::High,
        500,
        Some(2_000),
        Some(1),
        GrantEffect::ApprovalRequired,
    )
    .unwrap();
    let request = PolicyEngine::approval_request(&context, Some(&grant)).unwrap();
    let decision = ApprovalDecision::new_approved(request.clone(), 1_000).unwrap();
    let waiting = Run::queued(id(1), id(2), "writer", 3)
        .unwrap()
        .transition(RunState::Running, None)
        .unwrap()
        .wait_for_approval(request.clone())
        .unwrap();

    let outcome = waiting
        .resume_with_pending_approval(&request, &decision, &context, &[grant])
        .unwrap();
    let expected =
        GrantConsumption::new("approval-grant", 1, context.logical_invocation_id).unwrap();
    assert_eq!(outcome.run().state(), RunState::Running);
    assert_eq!(outcome.grant_consumption(), Some(&expected));
    let (resumed, consumption) = outcome.into_parts();
    assert_eq!(resumed.state(), RunState::Running);
    assert_eq!(consumption, Some(expected));
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

    let allowed = session_definition("writer", 3, true);
    let serial_only = session_definition("writer", 3, false);
    let concurrent =
        Session::new_for_definition(id(1), &allowed, SessionConcurrencyPolicy::Concurrent).unwrap();
    assert_eq!(
        concurrent.concurrency().unwrap(),
        SessionConcurrencyPolicy::Concurrent
    );

    let restored: Session =
        serde_json::from_value(serde_json::to_value(&concurrent).unwrap()).unwrap();
    assert_eq!(
        restored.concurrency().unwrap_err().code(),
        ExecutionErrorCode::MissingPrerequisite
    );
    assert_eq!(
        restored
            .assert_compatible(&allowed)
            .unwrap()
            .concurrency()
            .unwrap(),
        SessionConcurrencyPolicy::Concurrent
    );
    assert!(restored.assert_compatible(&serial_only).is_err());
    assert!(restored
        .assert_compatible(&session_definition("other", 3, true))
        .is_err());
    assert!(restored
        .assert_compatible(&session_definition("writer", 4, true))
        .is_err());

    let legacy: Session = serde_json::from_value(json!({
        "id": id(3),
        "definition_id": "writer",
        "definition_version": 3,
        "concurrency": "concurrent",
        "concurrency_pinned": true
    }))
    .unwrap();
    assert!(legacy.concurrency().is_err());
    assert_eq!(
        legacy
            .assert_compatible(&allowed)
            .unwrap()
            .concurrency()
            .unwrap(),
        SessionConcurrencyPolicy::Concurrent
    );
    assert!(legacy.assert_compatible(&serial_only).is_err());
    assert!(serde_json::from_value::<Session>(json!({
        "id": id(4),
        "definition_id": "writer",
        "definition_version": 3,
        "concurrency": "concurrent",
        "concurrency_pinned": false
    }))
    .is_err());

    let serial =
        Session::new_for_definition(id(2), &serial_only, SessionConcurrencyPolicy::Serial).unwrap();
    let mut forged = serde_json::to_value(&serial).unwrap();
    forged["concurrency"] = json!("concurrent");
    forged["allows_concurrent_sessions"] = json!(true);
    let forged: Session = serde_json::from_value(forged).unwrap();
    assert!(forged.concurrency().is_err());
    assert!(forged.assert_compatible(&serial_only).is_err());

    let mut contradictory = serde_json::to_value(&concurrent).unwrap();
    contradictory["allows_concurrent_sessions"] = json!(false);
    assert!(serde_json::from_value::<Session>(contradictory).is_err());
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
fn durable_events_require_positive_nondecreasing_timestamps() {
    assert!(RuntimeEvent::new(
        id(9),
        id(8),
        id(7),
        id(1),
        0,
        1,
        RuntimeEventKind::RunStarted,
    )
    .is_err());

    let first = RuntimeEvent::new(
        id(9),
        id(8),
        id(7),
        id(1),
        20,
        1,
        RuntimeEventKind::RunStarted,
    )
    .unwrap();
    let backwards = RuntimeEvent::new(
        id(10),
        id(8),
        id(7),
        id(1),
        19,
        2,
        RuntimeEventKind::StepStarted,
    )
    .unwrap();
    assert!(RuntimeEvent::validate_batch(1, &[first, backwards]).is_err());
}

#[test]
fn durable_event_vocabulary_is_complete_and_live_events_validate_standalone() {
    let durable = [
        RuntimeEventKind::StepStarted,
        RuntimeEventKind::StepRetried,
        RuntimeEventKind::StepCompleted,
        RuntimeEventKind::StepFailed,
        RuntimeEventKind::CapabilityProposed,
        RuntimeEventKind::CapabilityApproved,
        RuntimeEventKind::CapabilityDenied,
        RuntimeEventKind::MemoryProposed,
        RuntimeEventKind::MemorySuperseded,
        RuntimeEventKind::MemoryForgotten,
        RuntimeEventKind::ArtifactProposed,
        RuntimeEventKind::ArtifactCreated,
        RuntimeEventKind::ArtifactUpdated,
        RuntimeEventKind::BudgetUpdated,
        RuntimeEventKind::BudgetExtensionRequested,
        RuntimeEventKind::BudgetExhausted,
    ];
    for kind in durable {
        assert!(!serde_json::to_string(&kind).unwrap().is_empty());
    }
    assert!(
        serde_json::from_value::<anima_core::LiveRuntimeEvent>(json!({
            "run_id": Uuid::nil(), "input_tokens": 1, "output_tokens": 1
        }))
        .is_err()
    );
    assert!(serde_json::to_value(SafeEventPayload::Reference {
        reference: Uuid::nil(),
    })
    .is_err());
    assert!(serde_json::to_value(SafeEventPayload::Error {
        code: ExecutionErrorCode::InvalidEvent,
        reference: Some(Uuid::nil()),
    })
    .is_err());
    assert!(serde_json::to_value(SafeEventPayload::None).is_ok());
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
fn durable_execution_records_expose_identity_and_safe_read_models() {
    let run = Run::queued(id(1), id(2), "writer", 3).unwrap();
    assert_eq!(run.definition_id(), "writer");
    assert_eq!(run.definition_version(), 3);

    let step = Step::new(id(1), "step-a", StepKind::Capability).unwrap();
    assert_eq!(step.run_id(), id(1));
    assert_eq!(step.logical_step_id(), "step-a");
    assert_eq!(step.kind(), StepKind::Capability);

    let command = RuntimeCommand::start(id(3), id(2), id(1)).unwrap();
    assert_eq!(command.session_id(), id(2));
    assert_eq!(command.target_run_id(), id(1));
    assert_ne!(command.payload_digest(), Uuid::nil());

    let rejected = CommandReceipt::rejected(&command).unwrap();
    assert_eq!(rejected.command_id(), command.id());
    assert_eq!(rejected.payload_digest(), command.payload_digest());
    assert_eq!(rejected.outcome(), CommandOutcome::Rejected);
    assert_eq!(rejected.replay(&command).unwrap(), CommandOutcome::Rejected);

    let event = RuntimeEvent::with_payload(
        id(4),
        id(5),
        id(2),
        id(1),
        10,
        1,
        RuntimeEventKind::ArtifactRecorded,
        SafeEventPayload::Reference { reference: id(6) },
    )
    .unwrap();
    assert_eq!(event.event_id(), id(4));
    assert_eq!(event.owner_id(), id(5));
    assert_eq!(event.session_id(), id(2));
    assert_eq!(event.timestamp_ms(), 10);
    assert_eq!(
        event.payload(),
        &SafeEventPayload::Reference { reference: id(6) }
    );
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
fn full_checkpoints_round_trip_and_reject_tampered_records() {
    let invocation = LogicalInvocation::new(
        id(2),
        "cap-step",
        "workspace.write",
        1,
        json!({ "path": "a.md" }),
    )
    .unwrap();
    let manifest = ManifestPin::new_with_recovery_mode(
        "workspace.write",
        1,
        "sha256:abc",
        RecoveryMode::KeyedIdempotent,
    )
    .unwrap();
    let attempt = InvocationAttemptRecord::new(
        invocation.binding(),
        1,
        AttemptRecordState::Completed,
        manifest.clone(),
        RecoveryMode::KeyedIdempotent,
    )
    .unwrap();
    let completed = CompletedInvocationRecord::new(
        invocation.binding(),
        1,
        manifest.clone(),
        RecoveryMode::KeyedIdempotent,
        OpaqueReference::new(id(200)).unwrap(),
    )
    .unwrap();
    let checkpoint = CheckpointV1Builder::new(
        id(1),
        id(2),
        DefinitionPin::new(1, "writer", 3).unwrap(),
        4,
        vec![manifest],
        Budget::default(),
        Usage::default(),
    )
    .state(RunState::Completed, None)
    .attempts(vec![attempt])
    .completed_invocations(vec![completed])
    .message_context_refs(vec![OpaqueReference::new(id(201)).unwrap()])
    .model_context_refs(vec![OpaqueReference::new(id(202)).unwrap()])
    .memory_refs(vec![OpaqueReference::new(id(203)).unwrap()])
    .artifact_refs(vec![OpaqueReference::new(id(204)).unwrap()])
    .build()
    .unwrap();

    let encoded = serde_json::to_value(&checkpoint).unwrap();
    let restored: CheckpointV1 = serde_json::from_value(encoded.clone()).unwrap();
    assert_eq!(restored, checkpoint);
    assert_eq!(restored.attempts().len(), 1);
    assert_eq!(restored.completed_invocations().len(), 1);
    assert_eq!(restored.message_context_refs().len(), 1);

    let mut raw_result = encoded.clone();
    raw_result["completed_invocations"][0]["result_ref"] = json!("raw executor body");
    assert!(serde_json::from_value::<CheckpointV1>(raw_result).is_err());
    let mut mismatched_attempt = encoded;
    mismatched_attempt["completed_invocations"][0]["attempt_number"] = json!(2);
    assert!(serde_json::from_value::<CheckpointV1>(mismatched_attempt).is_err());
}

#[test]
fn pending_checkpoint_approval_must_match_the_active_invocation_identity() {
    let manifest = approval_manifest("workspace.write", 1);
    let invocation = LogicalInvocation::new(
        id(2),
        "approval-step",
        "workspace.write",
        1,
        json!({ "path": "a.md" }),
    )
    .unwrap();
    let request =
        PolicyEngine::approval_request(&approval_context_for(&manifest, &invocation), None)
            .unwrap();
    let manifest_pin = ManifestPin::from_manifest(&manifest).unwrap();
    let attempt = InvocationAttemptRecord::new(
        invocation.binding(),
        1,
        AttemptRecordState::Pending,
        manifest_pin.clone(),
        manifest.recovery_mode,
    )
    .unwrap();
    let build = |request| {
        CheckpointV1Builder::new(
            id(1),
            invocation.run_id(),
            DefinitionPin::new(1, "writer", 3).unwrap(),
            1,
            vec![manifest_pin.clone()],
            Budget::default(),
            Usage::default(),
        )
        .state(RunState::WaitingForApproval, None)
        .cursor(Some(
            CheckpointCursor::new(invocation.id(), 1, invocation.logical_step_id()).unwrap(),
        ))
        .attempts(vec![attempt.clone()])
        .pending_approval(Some(PendingApprovalRecord::new(request, None)?))
        .build()
    };

    let checkpoint = build(request.clone()).unwrap();

    let mut definition = session_definition("writer", 3, false);
    definition.resolved_capabilities = vec![ResolvedCapability {
        capability_id: manifest.id.clone(),
        manifest_version: manifest.version,
        schema_digest: manifest.schema_digest().to_owned(),
        override_config: None,
        approval_policy_revision: 1,
    }];
    let mut catalog = ManifestCatalog::default();
    catalog.register_manifest(manifest.clone()).unwrap();
    assert!(checkpoint.assert_compatible(&definition, &catalog).is_ok());
    let mut changed_policy = definition.clone();
    changed_policy.approval_policy_revision = 2;
    changed_policy.resolved_capabilities[0].approval_policy_revision = 2;
    assert!(checkpoint
        .assert_compatible(&changed_policy, &catalog)
        .is_err());

    let mut wrong_invocation = request.clone();
    wrong_invocation.logical_invocation_id = id(999);
    assert!(build(wrong_invocation).is_err());

    let other_step = LogicalInvocation::new(
        invocation.run_id(),
        "other-step",
        "workspace.write",
        1,
        json!({ "path": "a.md" }),
    )
    .unwrap();
    let other_step_request =
        PolicyEngine::approval_request(&approval_context_for(&manifest, &other_step), None)
            .unwrap();
    assert!(build(other_step_request).is_err());

    let other_arguments = LogicalInvocation::new(
        invocation.run_id(),
        invocation.logical_step_id(),
        "workspace.write",
        1,
        json!({ "path": "b.md" }),
    )
    .unwrap();
    let other_arguments_request =
        PolicyEngine::approval_request(&approval_context_for(&manifest, &other_arguments), None)
            .unwrap();
    assert!(build(other_arguments_request).is_err());

    let other_manifest = approval_manifest("workspace.delete", 2);
    let other_capability = LogicalInvocation::new(
        invocation.run_id(),
        invocation.logical_step_id(),
        &other_manifest.id,
        other_manifest.version,
        json!({ "path": "a.md" }),
    )
    .unwrap();
    let other_manifest_request = PolicyEngine::approval_request(
        &approval_context_for(&other_manifest, &other_capability),
        None,
    )
    .unwrap();
    assert!(build(other_manifest_request).is_err());

    let mut wrong_definition = request.clone();
    wrong_definition.agent_definition_id = "other-writer".into();
    assert!(build(wrong_definition).is_err());
    let mut wrong_definition_version = request.clone();
    wrong_definition_version.agent_definition_version = 4;
    assert!(build(wrong_definition_version).is_err());

    let mut invalid_policy_binding = request.clone();
    invalid_policy_binding.policy_revision = 2;
    assert!(PendingApprovalRecord::new(invalid_policy_binding, None).is_err());

    let encoded = serde_json::to_value(checkpoint).unwrap();
    for (field, value) in [
        ("agent_definition_id", json!("other-writer")),
        ("agent_definition_version", json!(4)),
        ("run_id", json!(id(77))),
        ("logical_step_id", json!("other-step")),
        ("logical_invocation_id", json!(id(999))),
        ("capability_id", json!("workspace.delete")),
        ("manifest_version", json!(2)),
        ("canonical_argument_digest", json!(id(998))),
    ] {
        let mut tampered = encoded.clone();
        tampered["pending_approval"]["request"][field] = value;
        assert!(
            serde_json::from_value::<CheckpointV1>(tampered).is_err(),
            "tampered approval field {field} was accepted"
        );
    }
}

#[test]
fn standalone_checkpoint_records_and_canonical_order_fail_closed() {
    let invocation = LogicalInvocation::new(
        id(2),
        "cap-step",
        "workspace.write",
        1,
        json!({ "path": "a.md" }),
    )
    .unwrap();
    let manifest = ManifestPin::new_with_recovery_mode(
        "workspace.write",
        1,
        "sha256:abc",
        RecoveryMode::KeyedIdempotent,
    )
    .unwrap();
    let record = InvocationAttemptRecord::new(
        invocation.binding(),
        1,
        AttemptRecordState::Completed,
        manifest,
        RecoveryMode::KeyedIdempotent,
    )
    .unwrap();
    let mut standalone = serde_json::to_value(record).unwrap();
    standalone["attempt_number"] = json!(0);
    assert!(serde_json::from_value::<InvocationAttemptRecord>(standalone).is_err());

    let checkpoint = CheckpointV1::new_minimal(
        id(1),
        id(2),
        "writer",
        3,
        4,
        vec![
            ManifestPin::new("a", 1, "sha256:aa").unwrap(),
            ManifestPin::new("b", 1, "sha256:bb").unwrap(),
        ],
    )
    .unwrap();
    let mut reversed = serde_json::to_value(checkpoint).unwrap();
    reversed["manifests"].as_array_mut().unwrap().reverse();
    assert!(serde_json::from_value::<CheckpointV1>(reversed).is_err());
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
fn concurrent_run_usage_is_a_latest_value_gauge_not_an_accumulating_counter() {
    let budget = Budget {
        max_concurrent_runs: Some(1),
        ..Budget::default()
    };
    let active = Usage::default().with_concurrent_runs(1);
    let completed = Usage::default().with_concurrent_runs(0);

    let idle = budget.accumulate(&active, &completed).unwrap();
    assert_eq!(idle.concurrent_runs(), 0);
    assert_eq!(
        budget
            .accumulate(&idle, &Usage::default().with_concurrent_runs(1))
            .unwrap()
            .concurrent_runs(),
        1
    );
    assert_eq!(
        budget
            .accumulate(&idle, &Usage::default().with_concurrent_runs(2))
            .unwrap_err()
            .code(),
        ExecutionErrorCode::BudgetExceeded
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
    let invalid_usage = Usage {
        total_tokens: 1,
        ..Usage::default()
    };
    assert!(serde_json::to_value(invalid_usage).is_err());
    let invalid_budget = Budget {
        max_wall_time_ms: Some(0),
        ..Budget::default()
    };
    assert!(serde_json::to_value(invalid_budget).is_err());
    let lease = ExecutionLease::new(id(1), id(2), 10).unwrap();
    assert_eq!(lease.run_id(), id(1));
    assert!(serde_json::to_value(ExecutionLease {
        run_id: Uuid::nil(),
        fence: id(2),
        expires_at_ms: 10,
    })
    .is_err());
    let invalid_left = Usage {
        input_tokens: 1,
        total_tokens: 0,
        ..Usage::default()
    };
    let invalid_right = Usage {
        output_tokens: 1,
        total_tokens: 2,
        ..Usage::default()
    };
    assert_eq!(
        invalid_left.checked_add(&invalid_right).unwrap_err().code(),
        ExecutionErrorCode::InvalidUsage
    );
    assert_eq!(
        Budget::default()
            .accumulate(&invalid_left, &invalid_right)
            .unwrap_err()
            .code(),
        ExecutionErrorCode::InvalidUsage
    );
}

#[test]
fn manual_recovery_pause_has_no_automatic_resume_path() {
    let invocation = LogicalInvocation::new(
        id(2),
        "manual-step",
        "workspace.write",
        1,
        json!({ "path": "a.md" }),
    )
    .unwrap();
    let manifest = ManifestPin::new_with_recovery_mode(
        "workspace.write",
        1,
        "sha256:manual",
        RecoveryMode::Manual,
    )
    .unwrap();
    let pause = RecoveryPauseRecord::new(
        invocation.binding(),
        1,
        manifest.clone(),
        RecoveryPauseReason::ManualReview,
    )
    .unwrap();
    assert!(!pause.allows_automatic_resume());
    let run = Run::queued(id(2), id(1), "writer", 1)
        .unwrap()
        .transition(RunState::Running, None)
        .unwrap()
        .pause_for_recovery(pause.clone())
        .unwrap();
    assert!(run.resume(None, None).is_err());
    assert_eq!(
        run.resolve_recovery_terminal(RecoveryTerminalResolution::Fail)
            .unwrap()
            .state(),
        RunState::Failed
    );
    assert!(run
        .resolve_recovery_terminal(RecoveryTerminalResolution::AdoptExternallyVerifiedResult {
            result_ref: Uuid::nil()
        })
        .is_err());
    let adopted = run
        .resolve_recovery_terminal(RecoveryTerminalResolution::AdoptExternallyVerifiedResult {
            result_ref: id(61),
        })
        .unwrap();
    assert_eq!(adopted.state(), RunState::Completed);
    assert_eq!(adopted.adopted_result_ref().unwrap().value(), id(61));
    let attempt = InvocationAttemptRecord::new(
        invocation.binding(),
        1,
        AttemptRecordState::Uncertain,
        manifest.clone(),
        RecoveryMode::Manual,
    )
    .unwrap();
    let uncertain = UncertainInvocationRecord::new_with_pause(pause, None).unwrap();
    assert!(CheckpointV1Builder::new(
        id(1),
        id(2),
        DefinitionPin::new(1, "writer", 1).unwrap(),
        1,
        vec![manifest],
        Budget::default(),
        Usage::default(),
    )
    .state(RunState::Paused, Some(RunPauseReason::RecoveryRequired))
    .cursor_step_id(Some("manual-step".into()))
    .attempts(vec![attempt])
    .uncertain_invocations(vec![uncertain])
    .build()
    .is_ok());
}

#[test]
fn keyed_manifest_with_manual_pause_reason_cannot_resume_automatically() {
    let invocation = LogicalInvocation::new(
        id(2),
        "keyed-manual",
        "workspace.write",
        1,
        json!({ "path": "a.md" }),
    )
    .unwrap();
    let manifest = ManifestPin::new_with_recovery_mode(
        "workspace.write",
        1,
        "sha256:keyed-manual",
        RecoveryMode::KeyedIdempotent,
    )
    .unwrap();
    let pause = RecoveryPauseRecord::new(
        invocation.binding(),
        1,
        manifest,
        RecoveryPauseReason::ManualReview,
    )
    .unwrap();
    assert!(!pause.allows_automatic_resume());
}

#[test]
fn checkpoint_preserves_uncertain_then_completed_attempt_history_and_pinned_mode() {
    let invocation = LogicalInvocation::new(
        id(2),
        "retry-step",
        "workspace.write",
        1,
        json!({ "path": "a.md" }),
    )
    .unwrap();
    let manifest = ManifestPin::new_with_recovery_mode(
        "workspace.write",
        1,
        "sha256:keyed",
        RecoveryMode::KeyedIdempotent,
    )
    .unwrap();
    let attempt_one = InvocationAttemptRecord::new(
        invocation.binding(),
        1,
        AttemptRecordState::Uncertain,
        manifest.clone(),
        RecoveryMode::KeyedIdempotent,
    )
    .unwrap();
    let attempt_two = InvocationAttemptRecord::new(
        invocation.binding(),
        2,
        AttemptRecordState::Completed,
        manifest.clone(),
        RecoveryMode::KeyedIdempotent,
    )
    .unwrap();
    let pause = RecoveryPauseRecord::new(
        invocation.binding(),
        1,
        manifest.clone(),
        RecoveryPauseReason::UncertainOutcome,
    )
    .unwrap();
    let uncertain = UncertainInvocationRecord::new_with_pause(pause, None).unwrap();
    let completed = CompletedInvocationRecord::new(
        invocation.binding(),
        2,
        manifest.clone(),
        RecoveryMode::KeyedIdempotent,
        OpaqueReference::new(id(50)).unwrap(),
    )
    .unwrap();
    let checkpoint = CheckpointV1Builder::new(
        id(1),
        id(2),
        DefinitionPin::new(1, "writer", 1).unwrap(),
        2,
        vec![manifest],
        Budget::default(),
        Usage::default(),
    )
    .state(RunState::Completed, None)
    .attempts(vec![attempt_two, attempt_one])
    .uncertain_invocations(vec![uncertain])
    .completed_invocations(vec![completed])
    .build()
    .unwrap();
    assert_eq!(checkpoint.attempts().len(), 2);
    let mut tampered = serde_json::to_value(&checkpoint).unwrap();
    tampered["manifests"][0]["recovery_mode"] = json!("NonRetryable");
    assert!(serde_json::from_value::<CheckpointV1>(tampered).is_err());
    let mut duplicate = serde_json::to_value(&checkpoint).unwrap();
    let first = duplicate["completed_invocations"][0].clone();
    duplicate["completed_invocations"]
        .as_array_mut()
        .unwrap()
        .push(first);
    assert!(serde_json::from_value::<CheckpointV1>(duplicate).is_err());
}
