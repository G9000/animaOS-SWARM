use anima_core::{
    ApprovalDecision, ApprovalGrantMutation, ApprovalResumeClaim, AttemptRecordState,
    AutonomyGrant, Budget, CapabilityKind, CapabilityManifest, CapabilityReferenceId,
    CheckpointV1Builder, CompletedInvocationRecord, CreateRun, DefinitionPin,
    DurableCapabilityResult, DurableCapabilityStatus, DurableResultMutation, ExecutionCommit,
    ExecutionStore, ExecutionStoreErrorCode, GrantEffect, GrantScope, GrantStatus,
    InMemoryExecutionStore, InvocationAttemptRecord, LogicalInvocation, ManifestPin,
    OpaqueReference, PolicyContext, PolicyEngine, PolicyRestrictions, RecoveryMode, RiskLevel, Run,
    RunState, RuntimeCommand, RuntimeCompatibility, RuntimeEvent, RuntimeEventKind, Session,
    SessionConcurrencyPolicy, Usage,
};
use uuid::Uuid;

fn id(value: u128) -> Uuid {
    Uuid::from_u128(value)
}

fn approval_resume_parts(
    session: &Session,
    run_id: Uuid,
) -> (Run, Run, RuntimeCommand, ApprovalGrantMutation) {
    let manifest = CapabilityManifest {
        id: "workspace.write".into(),
        version: 1,
        kind: CapabilityKind::Workspace,
        label: "Write".into(),
        description: "Writes a workspace file".into(),
        input_schema: serde_json::json!({"type": "object"}),
        output_schema: serde_json::json!({"type": "object"}),
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
        schema_digest: "sha256:workspace.write:1".into(),
        compatibility: RuntimeCompatibility {
            minimum_runtime_schema_version: 1,
            maximum_runtime_schema_version: 1,
            manifest_schema_version: 1,
        },
    };
    let invocation = LogicalInvocation::new(
        run_id,
        "write",
        "workspace.write",
        1,
        serde_json::json!({"path": "a"}),
    )
    .unwrap();
    let context = PolicyContext::new(
        "owner",
        "actor",
        "writer",
        3,
        "workspace",
        CapabilityReferenceId::new(id(61)),
        &manifest,
        &invocation,
        1,
        PolicyRestrictions::default(),
        1_000,
    )
    .unwrap();
    let grant = AutonomyGrant::new_with_effect(
        "approval-grant",
        1,
        GrantStatus::Active,
        GrantScope::new(
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
        .unwrap(),
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
    let waiting = Run::queued(run_id, session.id(), "writer", 3)
        .unwrap()
        .transition(RunState::Running, None)
        .unwrap()
        .wait_for_approval(request)
        .unwrap();
    let command =
        RuntimeCommand::resume_with_approval(id(62), session.id(), run_id, claim.binding().clone())
            .unwrap();
    let target = waiting
        .apply_resume_command(&command, Some(&claim), None)
        .unwrap()
        .run()
        .clone();
    (
        waiting,
        target,
        command,
        ApprovalGrantMutation::from_claim(claim),
    )
}

#[tokio::test]
async fn validated_approval_claim_and_grant_consumption_commit_once_with_command_replay() {
    let store = InMemoryExecutionStore::default();
    let session = Session::new(id(60), "writer", 3, SessionConcurrencyPolicy::Serial).unwrap();
    let (waiting, target, command, approval) = approval_resume_parts(&session, id(63));
    store
        .create_run(CreateRun::new(
            session.clone(),
            waiting,
            0,
            SessionConcurrencyPolicy::Serial,
        ))
        .await
        .unwrap();
    let lease = store.acquire_lease(id(63), 1, 1_000).await.unwrap();
    let event = RuntimeEvent::new(
        id(64),
        id(65),
        session.id(),
        id(63),
        1,
        1,
        RuntimeEventKind::RunResumed,
    )
    .unwrap();
    let commit = ExecutionCommit::new(
        1,
        0,
        lease,
        command,
        vec![event],
        vec![],
        vec![],
        vec![],
        Some(approval),
        target,
    );
    store.commit_execution(commit.clone()).await.unwrap();
    store.commit_execution(commit).await.unwrap();
}

#[tokio::test]
async fn terminal_commit_releases_the_serial_session_claim() {
    let store = InMemoryExecutionStore::default();
    let session = Session::new(id(40), "writer", 1, SessionConcurrencyPolicy::Serial).unwrap();
    let queued = Run::queued(id(41), session.id(), "writer", 1).unwrap();
    store
        .create_run(CreateRun::new(
            session.clone(),
            queued.clone(),
            0,
            SessionConcurrencyPolicy::Serial,
        ))
        .await
        .unwrap();
    let lease = store.acquire_lease(queued.id(), 1, 1_000).await.unwrap();
    let running = queued.transition(RunState::Running, None).unwrap();
    let started = RuntimeEvent::new(
        id(43),
        id(44),
        session.id(),
        queued.id(),
        1,
        1,
        RuntimeEventKind::RunStarted,
    )
    .unwrap();
    store
        .commit_execution(ExecutionCommit::new(
            1,
            0,
            lease.clone(),
            RuntimeCommand::start(id(42), session.id(), queued.id()).unwrap(),
            vec![started],
            vec![],
            vec![],
            vec![],
            None,
            running.clone(),
        ))
        .await
        .unwrap();
    let cancelled = running.transition(RunState::Cancelled, None).unwrap();
    let cancelled_event = RuntimeEvent::new(
        id(46),
        id(44),
        session.id(),
        queued.id(),
        2,
        2,
        RuntimeEventKind::RunCancelled,
    )
    .unwrap();
    store
        .commit_execution(ExecutionCommit::new(
            2,
            0,
            store.renew_lease(lease, 1_000).await.unwrap(),
            RuntimeCommand::cancel(id(47), session.id(), queued.id()).unwrap(),
            vec![cancelled_event],
            vec![],
            vec![],
            vec![],
            None,
            cancelled,
        ))
        .await
        .unwrap();

    store
        .create_run(CreateRun::new(
            session.clone(),
            Run::queued(id(48), session.id(), "writer", 1).unwrap(),
            1,
            SessionConcurrencyPolicy::Serial,
        ))
        .await
        .unwrap();
}

#[tokio::test]
async fn logical_invocation_results_accept_identical_replays_and_reject_conflicts() {
    let store = InMemoryExecutionStore::default();
    let session = Session::new(id(50), "writer", 1, SessionConcurrencyPolicy::Serial).unwrap();
    let queued = Run::queued(id(51), session.id(), "writer", 1).unwrap();
    store
        .create_run(CreateRun::new(
            session.clone(),
            queued.clone(),
            0,
            SessionConcurrencyPolicy::Serial,
        ))
        .await
        .unwrap();
    let invocation = LogicalInvocation::new(
        queued.id(),
        "capability-step",
        "workspace.write",
        1,
        serde_json::json!({"path": "a"}),
    )
    .unwrap();
    let manifest = ManifestPin::new_with_recovery_mode(
        "workspace.write",
        1,
        "sha256:manifest",
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
        manifest,
        RecoveryMode::KeyedIdempotent,
        OpaqueReference::new(id(52)).unwrap(),
    )
    .unwrap();
    let result = DurableCapabilityResult::new(
        CapabilityReferenceId::new(id(52)),
        "jcs-v1:result",
        "sha256:result",
        1,
        DurableCapabilityStatus::Completed,
    )
    .unwrap();
    let running = queued.transition(RunState::Running, None).unwrap();
    let lease = store.acquire_lease(queued.id(), 1, 1_000).await.unwrap();
    let first_event = RuntimeEvent::new(
        id(53),
        id(54),
        session.id(),
        queued.id(),
        1,
        1,
        RuntimeEventKind::RunStarted,
    )
    .unwrap();
    store
        .commit_execution(ExecutionCommit::new(
            1,
            0,
            lease.clone(),
            RuntimeCommand::start(id(55), session.id(), queued.id()).unwrap(),
            vec![first_event],
            vec![],
            vec![attempt.clone()],
            vec![DurableResultMutation::new(
                completed.clone(),
                result.clone(),
            )],
            None,
            running.clone(),
        ))
        .await
        .unwrap();

    let renewed = store.renew_lease(lease, 1_000).await.unwrap();
    let conflicting_result = DurableCapabilityResult::new(
        CapabilityReferenceId::new(id(52)),
        "jcs-v1:other",
        "sha256:result",
        1,
        DurableCapabilityStatus::Completed,
    )
    .unwrap();
    let second_event = RuntimeEvent::new(
        id(56),
        id(54),
        session.id(),
        queued.id(),
        2,
        2,
        RuntimeEventKind::StepCompleted,
    )
    .unwrap();
    assert_eq!(
        store
            .commit_execution(ExecutionCommit::new(
                2,
                0,
                renewed,
                RuntimeCommand::start(id(57), session.id(), queued.id()).unwrap(),
                vec![second_event],
                vec![],
                vec![attempt],
                vec![DurableResultMutation::new(completed, conflicting_result)],
                None,
                running,
            ))
            .await
            .unwrap_err()
            .code(),
        ExecutionStoreErrorCode::ResultConflict
    );
}

#[tokio::test]
async fn commit_is_all_or_nothing_and_checkpoint_versions_co_commit_with_state() {
    let store = InMemoryExecutionStore::default();
    let session = Session::new(id(20), "writer", 1, SessionConcurrencyPolicy::Serial).unwrap();
    let queued = Run::queued(id(21), session.id(), "writer", 1).unwrap();
    store
        .create_run(CreateRun::new(
            session.clone(),
            queued.clone(),
            0,
            SessionConcurrencyPolicy::Serial,
        ))
        .await
        .unwrap();
    let lease = store.acquire_lease(queued.id(), 1, 1_000).await.unwrap();
    let running = queued.transition(RunState::Running, None).unwrap();
    let command = RuntimeCommand::start(id(22), session.id(), queued.id()).unwrap();
    let invalid = RuntimeEvent::new(
        id(23),
        id(24),
        session.id(),
        queued.id(),
        1,
        2,
        RuntimeEventKind::RunStarted,
    )
    .unwrap();

    assert_eq!(
        store
            .commit_execution(ExecutionCommit::new(
                1,
                0,
                lease.clone(),
                command.clone(),
                vec![invalid],
                vec![],
                vec![],
                vec![],
                None,
                running.clone(),
            ))
            .await
            .unwrap_err()
            .code(),
        ExecutionStoreErrorCode::EventConflict
    );
    assert_eq!(
        store.load_run(queued.id()).await.unwrap().unwrap().run(),
        &queued
    );
    assert!(store
        .replay_events(queued.id(), 0)
        .await
        .unwrap()
        .is_empty());

    let event = RuntimeEvent::new(
        id(25),
        id(24),
        session.id(),
        queued.id(),
        1,
        1,
        RuntimeEventKind::RunStarted,
    )
    .unwrap();
    let checkpoint = CheckpointV1Builder::new(
        session.id(),
        queued.id(),
        DefinitionPin::new(1, "writer", 1).unwrap(),
        1,
        vec![],
        Budget::default(),
        Usage::default(),
    )
    .state(RunState::Running, None)
    .build()
    .unwrap();
    store
        .commit_execution(
            ExecutionCommit::new(
                1,
                0,
                lease,
                command,
                vec![event],
                vec![],
                vec![],
                vec![],
                None,
                running,
            )
            .with_checkpoint(checkpoint),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn command_replay_is_idempotent_but_conflicting_payload_is_rejected() {
    let store = InMemoryExecutionStore::default();
    let session = Session::new(id(30), "writer", 1, SessionConcurrencyPolicy::Serial).unwrap();
    let queued = Run::queued(id(31), session.id(), "writer", 1).unwrap();
    store
        .create_run(CreateRun::new(
            session.clone(),
            queued.clone(),
            0,
            SessionConcurrencyPolicy::Serial,
        ))
        .await
        .unwrap();
    let lease = store.acquire_lease(queued.id(), 1, 1_000).await.unwrap();
    let running = queued.transition(RunState::Running, None).unwrap();
    let command = RuntimeCommand::start(id(32), session.id(), queued.id()).unwrap();
    let event = RuntimeEvent::new(
        id(33),
        id(34),
        session.id(),
        queued.id(),
        1,
        1,
        RuntimeEventKind::RunStarted,
    )
    .unwrap();
    let commit = ExecutionCommit::new(
        1,
        0,
        lease.clone(),
        command.clone(),
        vec![event],
        vec![],
        vec![],
        vec![],
        None,
        running.clone(),
    );
    store.commit_execution(commit.clone()).await.unwrap();
    store.commit_execution(commit).await.unwrap();

    let conflicting = RuntimeCommand::cancel(command.id(), session.id(), queued.id()).unwrap();
    assert_eq!(
        store
            .commit_execution(ExecutionCommit::new(
                2,
                0,
                lease,
                conflicting,
                vec![],
                vec![],
                vec![],
                vec![],
                None,
                running,
            ))
            .await
            .unwrap_err()
            .code(),
        ExecutionStoreErrorCode::CommandConflict
    );
}

#[tokio::test]
async fn reclaimed_lease_fences_stale_execution_and_commits_contiguous_events() {
    let store = InMemoryExecutionStore::default();
    let session = Session::new(id(10), "writer", 1, SessionConcurrencyPolicy::Serial).unwrap();
    let queued = Run::queued(id(11), session.id(), "writer", 1).unwrap();
    store
        .create_run(CreateRun::new(
            session.clone(),
            queued.clone(),
            0,
            SessionConcurrencyPolicy::Serial,
        ))
        .await
        .unwrap();

    let stale = store.acquire_lease(queued.id(), 1, 1).await.unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    let current = store.acquire_lease(queued.id(), 1, 1_000).await.unwrap();
    let running = queued.transition(RunState::Running, None).unwrap();
    let event = RuntimeEvent::new(
        id(12),
        id(13),
        session.id(),
        queued.id(),
        1,
        1,
        RuntimeEventKind::RunStarted,
    )
    .unwrap();
    let command = RuntimeCommand::start(id(14), session.id(), queued.id()).unwrap();

    assert_eq!(
        store
            .commit_execution(ExecutionCommit::new(
                1,
                0,
                stale,
                command.clone(),
                vec![event.clone()],
                vec![],
                vec![],
                vec![],
                None,
                running.clone(),
            ))
            .await
            .unwrap_err()
            .code(),
        ExecutionStoreErrorCode::LeaseExpired
    );

    store
        .commit_execution(ExecutionCommit::new(
            1,
            0,
            current,
            command,
            vec![event.clone()],
            vec![],
            vec![],
            vec![],
            None,
            running,
        ))
        .await
        .unwrap();
    assert_eq!(
        store.replay_events(queued.id(), 0).await.unwrap(),
        vec![event]
    );
}

/// Reusable adapter conformance case: serial sessions hold one nonterminal run claim.
pub async fn serial_session_claim_contract<S: ExecutionStore>(store: &S) {
    let session = Session::new(id(1), "writer", 1, SessionConcurrencyPolicy::Serial).unwrap();
    let first = Run::queued(id(2), session.id(), "writer", 1).unwrap();
    let second = Run::queued(id(3), session.id(), "writer", 1).unwrap();

    store
        .create_run(CreateRun::new(
            session.clone(),
            first,
            0,
            SessionConcurrencyPolicy::Serial,
        ))
        .await
        .unwrap();

    assert_eq!(
        store
            .create_run(CreateRun::new(
                session.clone(),
                second,
                1,
                SessionConcurrencyPolicy::Serial,
            ))
            .await
            .unwrap_err()
            .code(),
        ExecutionStoreErrorCode::ActiveRunConflict
    );
    assert_eq!(
        store
            .create_run(CreateRun::new(
                session,
                Run::queued(id(4), id(1), "writer", 1).unwrap(),
                0,
                SessionConcurrencyPolicy::Serial,
            ))
            .await
            .unwrap_err()
            .code(),
        ExecutionStoreErrorCode::VersionConflict
    );
}

#[tokio::test]
async fn serial_session_claims_exactly_one_nonterminal_run() {
    serial_session_claim_contract(&InMemoryExecutionStore::default()).await;
}
