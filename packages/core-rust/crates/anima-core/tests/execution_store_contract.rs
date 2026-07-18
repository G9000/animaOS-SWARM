use anima_core::{
    assert_execution_store_conformance, ApprovalDecision, ApprovalGrantMutation,
    ApprovalResumeClaim, AttemptRecordState, AuthoritativeGrantChange,
    AuthoritativeGrantChangeKind, AuthoritativeGrantState, AuthoritativeGrantStatus, AutonomyGrant,
    Budget, CapabilityKind, CapabilityManifest, CapabilityReferenceId, CheckpointCursor,
    CheckpointMutation, CheckpointV1Builder, CompletedInvocationRecord, CreateRun, DefinitionPin,
    DurableCapabilityResult, DurableCapabilityStatus, DurableResultMutation, ExecutionCommit,
    ExecutionStep, ExecutionStore, ExecutionStoreError, ExecutionStoreErrorCode,
    ExecutionStoreFactory, GrantAuthorityBinding, GrantEffect, GrantScope, GrantStatus,
    InMemoryExecutionStore, InvocationAttemptRecord, LogicalInvocation, ManifestPin,
    ManualExecutionClock, OpaqueReference, PolicyContext, PolicyEngine, PolicyRestrictions,
    RecoveryMode, RiskLevel, Run, RunState, RuntimeCommand, RuntimeCompatibility, RuntimeEvent,
    RuntimeEventKind, Session, SessionConcurrencyPolicy, StoreReadPage, Usage, MAX_COMMIT_EVENTS,
};
use std::sync::Arc;
use uuid::Uuid;

fn id(value: u128) -> Uuid {
    Uuid::from_u128(value)
}

#[derive(Default)]
struct MemoryFactory {
    clock: ManualExecutionClock,
}

#[async_trait::async_trait]
impl ExecutionStoreFactory for MemoryFactory {
    type Store = InMemoryExecutionStore;

    async fn create_execution_store(&self) -> Result<Self::Store, ExecutionStoreError> {
        Ok(InMemoryExecutionStore::with_clock(Arc::new(
            self.clock.clone(),
        )))
    }

    fn advance_clock(&self, duration_ms: u64) -> Result<(), ExecutionStoreError> {
        self.clock.advance_ms(duration_ms)
    }
}

#[tokio::test]
async fn public_adapter_conformance_suite_runs_against_memory_store() {
    assert_execution_store_conformance(&MemoryFactory::default())
        .await
        .unwrap();
}

#[tokio::test]
async fn authoritative_grant_state_uses_trusted_create_update_and_revoke_cas() {
    let store = InMemoryExecutionStore::default();
    let owner_id = id(0x8f0);
    let created_grant = authority_fixture("grant-cas", 1, Some(2));
    let created = AuthoritativeGrantState::from_grant(owner_id, &created_grant).unwrap();
    assert_eq!(
        store
            .apply_authoritative_grant(owner_id, AuthoritativeGrantChange::create(created.clone()),)
            .await
            .unwrap(),
        created
    );
    assert_eq!(
        store
            .apply_authoritative_grant(owner_id, AuthoritativeGrantChange::create(created))
            .await
            .unwrap_err()
            .code(),
        ExecutionStoreErrorCode::VersionConflict
    );

    let upgraded_grant = authority_fixture("grant-cas", 2, Some(3));
    let upgraded = AuthoritativeGrantState::from_grant(owner_id, &upgraded_grant).unwrap();
    assert_eq!(
        store
            .apply_authoritative_grant(
                owner_id,
                AuthoritativeGrantChange::update(1, upgraded.clone()).unwrap(),
            )
            .await
            .unwrap(),
        upgraded
    );
    assert_eq!(
        store
            .apply_authoritative_grant(
                owner_id,
                AuthoritativeGrantChange::update(
                    1,
                    AuthoritativeGrantState::from_grant(
                        owner_id,
                        &authority_fixture("grant-cas", 3, Some(4)),
                    )
                    .unwrap(),
                )
                .unwrap(),
            )
            .await
            .unwrap_err()
            .code(),
        ExecutionStoreErrorCode::VersionConflict
    );

    let revoked = store
        .apply_authoritative_grant(
            owner_id,
            AuthoritativeGrantChange::revoke(upgraded.authority_key().clone(), 2).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoked.status(), AuthoritativeGrantStatus::Revoked);
    assert_eq!(
        store
            .load_authoritative_grant(owner_id, upgraded.authority_key())
            .await
            .unwrap(),
        Some(revoked)
    );
}

#[test]
fn external_adapters_can_exhaustively_inspect_validated_grant_changes() {
    fn inspect(change: &AuthoritativeGrantChange) -> (&str, Option<u32>, Option<u32>) {
        match change.kind() {
            AuthoritativeGrantChangeKind::Create(state) => {
                (state.authority_key_encoded(), None, Some(state.revision()))
            }
            AuthoritativeGrantChangeKind::Update {
                expected_revision,
                state,
            } => (
                state.authority_key_encoded(),
                Some(*expected_revision),
                Some(state.revision()),
            ),
            AuthoritativeGrantChangeKind::Revoke {
                authority_key,
                expected_revision,
            } => (authority_key.as_str(), Some(*expected_revision), None),
        }
    }

    let owner_id = id(0x8f1);
    let create = AuthoritativeGrantChange::create(
        AuthoritativeGrantState::from_grant(
            owner_id,
            &authority_fixture("external-grant", 1, Some(2)),
        )
        .unwrap(),
    );
    let update = AuthoritativeGrantChange::update(
        1,
        AuthoritativeGrantState::from_grant(
            owner_id,
            &authority_fixture("external-grant", 2, Some(1)),
        )
        .unwrap(),
    )
    .unwrap();
    let revoke = AuthoritativeGrantChange::revoke(update.authority_key().clone(), 2).unwrap();

    assert_eq!(inspect(&create).1, None);
    assert_eq!(inspect(&update).1, Some(1));
    assert_eq!(inspect(&revoke).1, Some(2));
    assert_eq!(inspect(&create).0, inspect(&update).0);
    assert_eq!(inspect(&update).0, inspect(&revoke).0);
}

#[test]
fn public_grant_transitions_validate_cas_revoke_and_exact_consumption() {
    let owner_id = id(0x8f3);
    let grant = authority_fixture("public-transition-grant", 1, Some(1));
    let binding = GrantAuthorityBinding::from_grant(&grant).unwrap();
    let created = AuthoritativeGrantState::from_grant(owner_id, &grant).unwrap();
    let create = AuthoritativeGrantChange::create(created.clone());

    assert_eq!(create.apply_to(None).unwrap(), created);
    assert_eq!(
        create.apply_to(Some(&created)).unwrap_err().code(),
        ExecutionStoreErrorCode::VersionConflict
    );

    let consumed = created.consume(&binding, 1_000).unwrap();
    assert_eq!(consumed.remaining_uses(), Some(0));
    assert_eq!(
        consumed.consume(&binding, 1_000).unwrap_err().code(),
        ExecutionStoreErrorCode::GrantConflict
    );

    let revoked = AuthoritativeGrantChange::revoke(created.authority_key().clone(), 1)
        .unwrap()
        .apply_to(Some(&created))
        .unwrap();
    assert_eq!(revoked.status(), GrantStatus::Revoked);
    assert_eq!(
        revoked
            .validate_binding(&binding, 1_000)
            .unwrap_err()
            .code(),
        ExecutionStoreErrorCode::GrantConflict
    );

    let mut forged = grant;
    forged.scope.workspace_id = "forged-workspace".into();
    let forged_binding = GrantAuthorityBinding::from_grant(&forged).unwrap();
    assert_eq!(
        created.consume(&forged_binding, 1_000).unwrap_err().code(),
        ExecutionStoreErrorCode::GrantConflict
    );
}

fn authority_fixture(raw_id: &str, revision: u32, remaining_uses: Option<u32>) -> AutonomyGrant {
    AutonomyGrant::new_with_effect(
        raw_id,
        revision,
        GrantStatus::Active,
        GrantScope::new(
            "authority-owner",
            "authority-actor",
            "authority-agent",
            1,
            "authority-workspace",
            CapabilityReferenceId::new(id(0x8f2)),
            "workspace.write",
            1,
            None,
        )
        .unwrap(),
        RiskLevel::High,
        1,
        Some(10_000),
        remaining_uses,
        GrantEffect::ApprovalRequired,
    )
    .unwrap()
}

#[test]
fn authoritative_grants_are_owner_scoped_opaque_and_bind_immutable_authority() {
    let owner = id(0x900);
    let grant = AutonomyGrant::new_with_effect(
        "raw-grant-id-must-never-persist",
        7,
        GrantStatus::Active,
        GrantScope::new(
            "policy-owner",
            "actor",
            "writer",
            3,
            "workspace",
            CapabilityReferenceId::new(id(0x901)),
            "workspace.write",
            1,
            Some(id(0x902)),
        )
        .unwrap(),
        RiskLevel::Critical,
        500,
        Some(2_000),
        Some(2),
        GrantEffect::ApprovalRequired,
    )
    .unwrap();
    let state = AuthoritativeGrantState::from_grant(owner, &grant).unwrap();
    let encoded = serde_json::to_string(&state).unwrap();
    let debug = format!("{state:?}");

    assert_eq!(state.owner_id(), owner);
    assert_eq!(state.revision(), grant.revision);
    assert_eq!(state.remaining_uses(), grant.remaining_uses);
    assert_eq!(state.effect(), grant.effect);
    assert_eq!(state.maximum_risk(), grant.maximum_risk);
    assert!(!state.authority_key().as_str().contains(&grant.id));
    assert!(!state.full_grant_digest().contains(&grant.id));
    assert!(!encoded.contains(&grant.id));
    assert!(!debug.contains(&grant.id));

    let mut changed_scope = grant.clone();
    changed_scope.scope.workspace_id = "other-workspace".into();
    let changed = AuthoritativeGrantState::from_grant(owner, &changed_scope).unwrap();
    assert_eq!(state.authority_key(), changed.authority_key());
    assert_ne!(state.full_grant_digest(), changed.full_grant_digest());
}

#[test]
fn counted_grant_authority_digest_is_stable_while_live_count_bindings_advance() {
    let owner = id(0x901);
    let mut live_grant = authority_fixture("multi-use-grant", 1, Some(3));
    let initial_binding = GrantAuthorityBinding::from_grant(&live_grant).unwrap();
    let initial = AuthoritativeGrantState::from_grant(owner, &live_grant).unwrap();

    live_grant.remaining_uses = Some(2);
    let second_binding = GrantAuthorityBinding::from_grant(&live_grant).unwrap();
    assert_eq!(
        initial_binding.full_grant_digest(),
        second_binding.full_grant_digest()
    );
    assert_ne!(initial_binding, second_binding);

    let after_first = initial.consume(&initial_binding, 1_000).unwrap();
    assert_eq!(after_first.remaining_uses(), Some(2));
    after_first
        .validate_binding(&second_binding, 1_000)
        .unwrap();
    assert_eq!(
        after_first
            .validate_binding(&initial_binding, 1_000)
            .unwrap_err()
            .code(),
        ExecutionStoreErrorCode::GrantConflict
    );
}

fn approval_resume_parts(
    session: &Session,
    run_id: Uuid,
) -> (
    Run,
    Run,
    RuntimeCommand,
    ApprovalGrantMutation,
    AutonomyGrant,
) {
    let manifest = CapabilityManifest::new(anima_core::CapabilityManifestInput {
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
        compatibility: RuntimeCompatibility {
            minimum_runtime_schema_version: 1,
            maximum_runtime_schema_version: 1,
            manifest_schema_version: 1,
        },
    })
    .unwrap();
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
    let claim =
        ApprovalResumeClaim::new(&request, &decision, &context, std::slice::from_ref(&grant))
            .unwrap();
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
        grant,
    )
}

#[tokio::test]
async fn validated_approval_claim_and_grant_consumption_commit_once_with_command_replay() {
    let store = InMemoryExecutionStore::with_clock(Arc::new(ManualExecutionClock::new(1_000)));
    let owner_id = id(65);
    let session = Session::new(id(60), "writer", 3, SessionConcurrencyPolicy::Serial).unwrap();
    let (waiting, target, command, approval, grant) = approval_resume_parts(&session, id(63));
    assert_eq!(approval.remaining_uses(), Some(1));
    assert!(approval.grant_consumption().is_some());
    let queued = Run::queued(id(63), session.id(), "writer", 3).unwrap();
    let created = store
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                id(65),
                session.clone(),
                queued.clone(),
                0,
                SessionConcurrencyPolicy::Serial,
            ),
        )
        .await
        .unwrap();
    let lease = store
        .acquire_lease(owner_id, id(63), created.run_version(), 1_000)
        .await
        .unwrap();
    let running = queued.transition(RunState::Running, None).unwrap();
    let started = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                created.run_version(),
                0,
                lease.clone(),
                RuntimeCommand::start(id(66), session.id(), queued.id()).unwrap(),
                vec![],
                vec![],
                vec![],
                vec![],
                None,
                running,
            ),
        )
        .await
        .unwrap();
    let waiting_outcome = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                started.stored_run().run_version(),
                started.checkpoint_version(),
                lease.clone(),
                RuntimeCommand::request_approval(
                    id(67),
                    session.id(),
                    queued.id(),
                    waiting.pending_approval().unwrap().clone(),
                )
                .unwrap(),
                vec![],
                vec![],
                vec![],
                vec![],
                None,
                waiting,
            ),
        )
        .await
        .unwrap();
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
    let expected_consumption = approval.grant_consumption().cloned();
    let commit = ExecutionCommit::new(
        waiting_outcome.stored_run().run_version(),
        waiting_outcome.checkpoint_version(),
        lease,
        command,
        vec![event],
        vec![],
        vec![],
        vec![],
        Some(approval),
        target,
    );
    assert_eq!(
        store
            .commit_execution(owner_id, commit.clone())
            .await
            .unwrap_err()
            .code(),
        ExecutionStoreErrorCode::GrantConflict
    );
    store
        .apply_authoritative_grant(
            owner_id,
            AuthoritativeGrantChange::create(
                AuthoritativeGrantState::from_grant(owner_id, &grant).unwrap(),
            ),
        )
        .await
        .unwrap();
    let outcome = store
        .commit_execution(owner_id, commit.clone())
        .await
        .unwrap();
    assert_eq!(outcome.grant_consumption(), expected_consumption.as_ref());
    assert_eq!(
        store.commit_execution(owner_id, commit).await.unwrap(),
        outcome
    );
}

#[tokio::test]
async fn terminal_commit_releases_the_serial_session_claim() {
    let store = InMemoryExecutionStore::default();
    let owner_id = id(44);
    let session = Session::new(id(40), "writer", 1, SessionConcurrencyPolicy::Serial).unwrap();
    let queued = Run::queued(id(41), session.id(), "writer", 1).unwrap();
    store
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                id(44),
                session.clone(),
                queued.clone(),
                0,
                SessionConcurrencyPolicy::Serial,
            ),
        )
        .await
        .unwrap();
    let lease = store
        .acquire_lease(owner_id, queued.id(), 1, 1_000)
        .await
        .unwrap();
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
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
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
            ),
        )
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
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                2,
                0,
                store.renew_lease(owner_id, lease, 1_000).await.unwrap(),
                RuntimeCommand::cancel(id(47), session.id(), queued.id()).unwrap(),
                vec![cancelled_event],
                vec![],
                vec![],
                vec![],
                None,
                cancelled,
            ),
        )
        .await
        .unwrap();

    store
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                id(44),
                session.clone(),
                Run::queued(id(48), session.id(), "writer", 1).unwrap(),
                2,
                SessionConcurrencyPolicy::Serial,
            ),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn session_identity_pins_owner_definition_and_initial_lifecycle() {
    let store = InMemoryExecutionStore::default();
    let owner = id(80);
    let owner_id = owner;
    let session = Session::new(id(81), "writer", 1, SessionConcurrencyPolicy::Serial).unwrap();
    let queued = Run::queued(id(82), session.id(), "writer", 1).unwrap();
    let created = store
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                owner,
                session.clone(),
                queued.clone(),
                0,
                SessionConcurrencyPolicy::Serial,
            ),
        )
        .await
        .unwrap();
    let lease = store
        .acquire_lease(owner_id, queued.id(), created.run_version(), 1_000)
        .await
        .unwrap();
    let running = queued.transition(RunState::Running, None).unwrap();
    let started = RuntimeEvent::new(
        id(83),
        owner,
        session.id(),
        queued.id(),
        1,
        1,
        RuntimeEventKind::RunStarted,
    )
    .unwrap();
    let running_outcome = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                created.run_version(),
                0,
                lease.clone(),
                RuntimeCommand::start(id(84), session.id(), queued.id()).unwrap(),
                vec![started],
                vec![],
                vec![],
                vec![],
                None,
                running.clone(),
            ),
        )
        .await
        .unwrap();
    let cancelled = running.transition(RunState::Cancelled, None).unwrap();
    let cancelled_event = RuntimeEvent::new(
        id(85),
        owner,
        session.id(),
        queued.id(),
        2,
        2,
        RuntimeEventKind::RunCancelled,
    )
    .unwrap();
    let terminal = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                running_outcome.stored_run().run_version(),
                0,
                store.renew_lease(owner_id, lease, 1_000).await.unwrap(),
                RuntimeCommand::cancel(id(86), session.id(), queued.id()).unwrap(),
                vec![cancelled_event],
                vec![],
                vec![],
                vec![],
                None,
                cancelled,
            ),
        )
        .await
        .unwrap();

    let wrong_owner_run = Run::queued(id(87), session.id(), "writer", 1).unwrap();
    assert_eq!(
        store
            .create_run(
                owner_id,
                CreateRun::new_for_owner(
                    id(88),
                    session.clone(),
                    wrong_owner_run.clone(),
                    terminal.stored_run().session_version(),
                    SessionConcurrencyPolicy::Serial,
                )
            )
            .await
            .unwrap_err()
            .code(),
        ExecutionStoreErrorCode::InvalidRequest
    );
    assert!(store
        .load_run(owner_id, wrong_owner_run.id())
        .await
        .unwrap()
        .is_none());

    let changed_session =
        Session::new(session.id(), "writer", 2, SessionConcurrencyPolicy::Serial).unwrap();
    let changed_run = Run::queued(id(89), changed_session.id(), "writer", 2).unwrap();
    assert_eq!(
        store
            .create_run(
                owner_id,
                CreateRun::new_for_owner(
                    owner,
                    changed_session,
                    changed_run.clone(),
                    terminal.stored_run().session_version(),
                    SessionConcurrencyPolicy::Serial,
                )
            )
            .await
            .unwrap_err()
            .code(),
        ExecutionStoreErrorCode::InvalidRequest
    );
    assert!(store
        .load_run(owner_id, changed_run.id())
        .await
        .unwrap()
        .is_none());

    let terminal_session =
        Session::new(id(90), "writer", 1, SessionConcurrencyPolicy::Serial).unwrap();
    let terminal_run = Run::queued(id(91), terminal_session.id(), "writer", 1)
        .unwrap()
        .transition(RunState::Running, None)
        .unwrap()
        .transition(RunState::Completed, None)
        .unwrap();
    assert_eq!(
        store
            .create_run(
                owner_id,
                CreateRun::new_for_owner(
                    owner,
                    terminal_session,
                    terminal_run,
                    0,
                    SessionConcurrencyPolicy::Serial,
                )
            )
            .await
            .unwrap_err()
            .code(),
        ExecutionStoreErrorCode::InvalidRequest
    );
}

#[tokio::test]
async fn create_run_requires_a_queued_initial_run() {
    let store = InMemoryExecutionStore::default();
    let owner_id = id(0x92);
    let session = Session::new(id(0x93), "writer", 1, SessionConcurrencyPolicy::Serial).unwrap();
    let running = Run::queued(id(0x94), session.id(), "writer", 1)
        .unwrap()
        .transition(RunState::Running, None)
        .unwrap();

    assert_eq!(
        store
            .create_run(
                owner_id,
                CreateRun::new_for_owner(
                    owner_id,
                    session,
                    running.clone(),
                    0,
                    SessionConcurrencyPolicy::Serial,
                ),
            )
            .await
            .unwrap_err()
            .code(),
        ExecutionStoreErrorCode::InvalidRequest
    );
    assert!(store
        .load_run(owner_id, running.id())
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn logical_invocation_results_accept_identical_replays_and_reject_conflicts() {
    let store = InMemoryExecutionStore::default();
    let owner_id = id(54);
    let session = Session::new(id(50), "writer", 1, SessionConcurrencyPolicy::Serial).unwrap();
    let queued = Run::queued(id(51), session.id(), "writer", 1).unwrap();
    let created = store
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                id(54),
                session.clone(),
                queued.clone(),
                0,
                SessionConcurrencyPolicy::Serial,
            ),
        )
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
        format!("sha256:{}", "2".repeat(64)),
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
        OpaqueReference::new(id(52)).unwrap(),
    )
    .unwrap();
    let result = DurableCapabilityResult::new(
        CapabilityReferenceId::new(id(52)),
        format!("jcs-v1:{}", "1".repeat(64)),
        format!("sha256:{}", "2".repeat(64)),
        1,
        DurableCapabilityStatus::Completed,
    )
    .unwrap();
    let running = queued.transition(RunState::Running, None).unwrap();
    let lease = store
        .acquire_lease(owner_id, queued.id(), 1, 1_000)
        .await
        .unwrap();
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
    let missing_attempt = ExecutionCommit::new(
        1,
        0,
        lease.clone(),
        RuntimeCommand::start(id(58), session.id(), queued.id()).unwrap(),
        vec![first_event.clone()],
        vec![],
        vec![],
        vec![DurableResultMutation::new(
            completed.clone(),
            result.clone(),
        )],
        None,
        running.clone(),
    );
    assert_eq!(
        store
            .commit_execution(owner_id, missing_attempt)
            .await
            .unwrap_err()
            .code(),
        ExecutionStoreErrorCode::LineageConflict
    );
    assert_eq!(
        store
            .load_run(owner_id, queued.id())
            .await
            .unwrap()
            .unwrap(),
        created
    );
    assert!(store
        .load_durable_result(owner_id, queued.id(), invocation.id())
        .await
        .unwrap()
        .is_none());
    let mismatched_result = DurableCapabilityResult::new(
        CapabilityReferenceId::new(id(52)),
        format!("jcs-v1:{}", "4".repeat(64)),
        format!("sha256:{}", "3".repeat(64)),
        1,
        DurableCapabilityStatus::Completed,
    )
    .unwrap();
    assert_eq!(
        store
            .commit_execution(
                owner_id,
                ExecutionCommit::new(
                    1,
                    0,
                    lease.clone(),
                    RuntimeCommand::start(id(59), session.id(), queued.id()).unwrap(),
                    vec![first_event.clone()],
                    vec![],
                    vec![attempt.clone()],
                    vec![DurableResultMutation::new(
                        completed.clone(),
                        mismatched_result,
                    )],
                    None,
                    running.clone(),
                ),
            )
            .await
            .unwrap_err()
            .code(),
        ExecutionStoreErrorCode::LineageConflict
    );
    assert_eq!(
        store
            .load_run(owner_id, queued.id())
            .await
            .unwrap()
            .unwrap(),
        created
    );
    assert!(store
        .load_attempts_page(owner_id, queued.id(), StoreReadPage::first(1).unwrap())
        .await
        .unwrap()
        .items()
        .is_empty());
    store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
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
            ),
        )
        .await
        .unwrap();

    let renewed = store.renew_lease(owner_id, lease, 1_000).await.unwrap();
    let conflicting_result = DurableCapabilityResult::new(
        CapabilityReferenceId::new(id(52)),
        format!("jcs-v1:{}", "3".repeat(64)),
        format!("sha256:{}", "2".repeat(64)),
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
            .commit_execution(
                owner_id,
                ExecutionCommit::new(
                    2,
                    0,
                    renewed,
                    RuntimeCommand::record_progress(id(57), session.id(), queued.id()).unwrap(),
                    vec![second_event],
                    vec![],
                    vec![attempt],
                    vec![DurableResultMutation::new(completed, conflicting_result)],
                    None,
                    running,
                )
            )
            .await
            .unwrap_err()
            .code(),
        ExecutionStoreErrorCode::ResultConflict
    );
}

#[tokio::test]
async fn execution_step_and_attempt_history_is_append_only() {
    let store = InMemoryExecutionStore::default();
    let owner_id = id(73);
    let session = Session::new(id(70), "writer", 1, SessionConcurrencyPolicy::Serial).unwrap();
    let queued = Run::queued(id(71), session.id(), "writer", 1).unwrap();
    store
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                id(73),
                session.clone(),
                queued.clone(),
                0,
                SessionConcurrencyPolicy::Serial,
            ),
        )
        .await
        .unwrap();
    let invocation = LogicalInvocation::new(
        queued.id(),
        "append-only",
        "workspace.write",
        1,
        serde_json::json!({"path": "append.txt"}),
    )
    .unwrap();
    let manifest = ManifestPin::new_with_recovery_mode(
        "workspace.write",
        1,
        format!("sha256:{}", "4".repeat(64)),
        RecoveryMode::KeyedIdempotent,
    )
    .unwrap();
    let pending = InvocationAttemptRecord::new(
        invocation.binding(),
        1,
        AttemptRecordState::Pending,
        manifest.clone(),
        RecoveryMode::KeyedIdempotent,
    )
    .unwrap();
    let step =
        ExecutionStep::new(queued.id(), "append-only", anima_core::StepKind::Capability).unwrap();
    let running = queued.transition(RunState::Running, None).unwrap();
    let lease = store
        .acquire_lease(owner_id, queued.id(), 1, 1_000)
        .await
        .unwrap();
    let event = RuntimeEvent::new(
        id(72),
        id(73),
        session.id(),
        queued.id(),
        1,
        1,
        RuntimeEventKind::RunStarted,
    )
    .unwrap();
    store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                1,
                0,
                lease.clone(),
                RuntimeCommand::start(id(74), session.id(), queued.id()).unwrap(),
                vec![event],
                vec![step.clone()],
                vec![pending.clone()],
                vec![],
                None,
                running.clone(),
            ),
        )
        .await
        .unwrap();

    store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                2,
                0,
                lease.clone(),
                RuntimeCommand::record_progress(id(75), session.id(), queued.id()).unwrap(),
                vec![],
                vec![step.clone()],
                vec![pending.clone()],
                vec![],
                None,
                running.clone(),
            ),
        )
        .await
        .unwrap();

    let conflicting_step =
        ExecutionStep::new(queued.id(), "append-only", anima_core::StepKind::Policy).unwrap();
    assert_eq!(
        store
            .commit_execution(
                owner_id,
                ExecutionCommit::new(
                    3,
                    0,
                    lease.clone(),
                    RuntimeCommand::record_progress(id(76), session.id(), queued.id()).unwrap(),
                    vec![],
                    vec![conflicting_step],
                    vec![],
                    vec![],
                    None,
                    running.clone(),
                )
            )
            .await
            .unwrap_err()
            .code(),
        ExecutionStoreErrorCode::HistoryConflict
    );
    let conflicting_attempt = InvocationAttemptRecord::new(
        invocation.binding(),
        1,
        AttemptRecordState::Completed,
        manifest,
        RecoveryMode::KeyedIdempotent,
    )
    .unwrap();
    assert_eq!(
        store
            .commit_execution(
                owner_id,
                ExecutionCommit::new(
                    3,
                    0,
                    lease,
                    RuntimeCommand::record_progress(id(77), session.id(), queued.id()).unwrap(),
                    vec![],
                    vec![],
                    vec![conflicting_attempt],
                    vec![],
                    None,
                    running,
                )
            )
            .await
            .unwrap_err()
            .code(),
        ExecutionStoreErrorCode::HistoryConflict
    );
    assert_eq!(
        store
            .load_steps_page(owner_id, queued.id(), StoreReadPage::first(256).unwrap())
            .await
            .unwrap()
            .into_items(),
        vec![step]
    );
    assert_eq!(
        store
            .load_attempts_page(owner_id, queued.id(), StoreReadPage::first(256).unwrap())
            .await
            .unwrap()
            .into_items(),
        vec![pending]
    );
}

#[tokio::test]
async fn commit_is_all_or_nothing_and_checkpoint_versions_co_commit_with_state() {
    let store = InMemoryExecutionStore::default();
    let owner_id = id(24);
    let session = Session::new(id(20), "writer", 1, SessionConcurrencyPolicy::Serial).unwrap();
    let queued = Run::queued(id(21), session.id(), "writer", 1).unwrap();
    store
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                id(24),
                session.clone(),
                queued.clone(),
                0,
                SessionConcurrencyPolicy::Serial,
            ),
        )
        .await
        .unwrap();
    let lease = store
        .acquire_lease(owner_id, queued.id(), 1, 1_000)
        .await
        .unwrap();
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
            .commit_execution(
                owner_id,
                ExecutionCommit::new(
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
                )
            )
            .await
            .unwrap_err()
            .code(),
        ExecutionStoreErrorCode::EventConflict
    );
    assert_eq!(
        store
            .load_run(owner_id, queued.id())
            .await
            .unwrap()
            .unwrap()
            .run(),
        &queued
    );
    assert!(store
        .replay_events(owner_id, queued.id(), StoreReadPage::first(256).unwrap())
        .await
        .unwrap()
        .events()
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
    let checkpoint_invocation = LogicalInvocation::new(
        queued.id(),
        "checkpoint-step",
        "workspace.write",
        1,
        serde_json::json!({"path": "checkpoint.txt"}),
    )
    .unwrap();
    let checkpoint_manifest = ManifestPin::new_with_recovery_mode(
        "workspace.write",
        1,
        format!("sha256:{}", "5".repeat(64)),
        RecoveryMode::KeyedIdempotent,
    )
    .unwrap();
    let checkpoint_attempt = InvocationAttemptRecord::new(
        checkpoint_invocation.binding(),
        1,
        AttemptRecordState::Pending,
        checkpoint_manifest.clone(),
        RecoveryMode::KeyedIdempotent,
    )
    .unwrap();
    let checkpoint_step = ExecutionStep::new(
        queued.id(),
        "checkpoint-step",
        anima_core::StepKind::Capability,
    )
    .unwrap();
    let divergent_checkpoint = CheckpointV1Builder::new(
        session.id(),
        queued.id(),
        DefinitionPin::new(1, "writer", 1).unwrap(),
        1,
        vec![checkpoint_manifest.clone()],
        Budget::default(),
        Usage::default(),
    )
    .state(RunState::Running, None)
    .build()
    .unwrap();
    assert_eq!(
        store
            .commit_execution(
                owner_id,
                ExecutionCommit::new(
                    1,
                    0,
                    lease.clone(),
                    command.clone(),
                    vec![event.clone()],
                    vec![checkpoint_step.clone()],
                    vec![checkpoint_attempt.clone()],
                    vec![],
                    None,
                    running.clone(),
                )
                .with_checkpoint(divergent_checkpoint),
            )
            .await
            .unwrap_err()
            .code(),
        ExecutionStoreErrorCode::CheckpointConflict
    );
    assert!(store
        .load_steps_page(owner_id, queued.id(), StoreReadPage::first(256).unwrap())
        .await
        .unwrap()
        .is_empty());
    assert!(store
        .load_attempts_page(owner_id, queued.id(), StoreReadPage::first(256).unwrap())
        .await
        .unwrap()
        .is_empty());
    let checkpoint = CheckpointV1Builder::new(
        session.id(),
        queued.id(),
        DefinitionPin::new(1, "writer", 1).unwrap(),
        1,
        vec![checkpoint_manifest],
        Budget::default(),
        Usage::default(),
    )
    .state(RunState::Running, None)
    .attempts(vec![checkpoint_attempt.clone()])
    .cursor(Some(
        CheckpointCursor::new(checkpoint_invocation.id(), 1, "checkpoint-step").unwrap(),
    ))
    .build()
    .unwrap();
    let committed = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                1,
                0,
                lease.clone(),
                command,
                vec![event],
                vec![checkpoint_step],
                vec![checkpoint_attempt],
                vec![],
                None,
                running,
            )
            .with_checkpoint(checkpoint.clone()),
        )
        .await
        .unwrap();
    assert_eq!(committed.checkpoint_version(), 1);
    assert_eq!(committed.checkpoint(), Some(&checkpoint));

    let lease = store.renew_lease(owner_id, lease, 1_000).await.unwrap();
    let progress = RuntimeCommand::record_progress(id(0x26), session.id(), queued.id()).unwrap();
    let progressed = RuntimeEvent::new(
        id(0x27),
        owner_id,
        session.id(),
        queued.id(),
        2,
        2,
        RuntimeEventKind::StepStarted,
    )
    .unwrap();
    let stale = ExecutionCommit::new(
        committed.stored_run().run_version(),
        committed.checkpoint_version(),
        lease.clone(),
        progress.clone(),
        vec![progressed.clone()],
        vec![],
        vec![],
        vec![],
        None,
        committed.stored_run().run().clone(),
    )
    .with_checkpoint_mutation(CheckpointMutation::Unchanged);
    assert_eq!(
        store
            .commit_execution(owner_id, stale)
            .await
            .unwrap_err()
            .code(),
        ExecutionStoreErrorCode::CheckpointConflict
    );
    assert_eq!(
        store.load_checkpoint(owner_id, queued.id()).await.unwrap(),
        Some((1, checkpoint))
    );

    let clear = ExecutionCommit::new(
        committed.stored_run().run_version(),
        committed.checkpoint_version(),
        lease,
        progress,
        vec![progressed],
        vec![],
        vec![],
        vec![],
        None,
        committed.stored_run().run().clone(),
    )
    .with_checkpoint_mutation(CheckpointMutation::Clear);
    let cleared = store
        .commit_execution(owner_id, clear.clone())
        .await
        .unwrap();
    assert_eq!(cleared.checkpoint_version(), 2);
    assert_eq!(cleared.checkpoint(), None);
    assert_eq!(
        store.commit_execution(owner_id, clear).await.unwrap(),
        cleared
    );
    assert_eq!(
        store.load_checkpoint(owner_id, queued.id()).await.unwrap(),
        None
    );
}

#[tokio::test]
async fn command_replay_is_idempotent_but_conflicting_payload_is_rejected() {
    let store = InMemoryExecutionStore::default();
    let owner_id = id(34);
    let session = Session::new(id(30), "writer", 1, SessionConcurrencyPolicy::Serial).unwrap();
    let queued = Run::queued(id(31), session.id(), "writer", 1).unwrap();
    store
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                id(34),
                session.clone(),
                queued.clone(),
                0,
                SessionConcurrencyPolicy::Serial,
            ),
        )
        .await
        .unwrap();
    let lease = store
        .acquire_lease(owner_id, queued.id(), 1, 1_000)
        .await
        .unwrap();
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
    store
        .commit_execution(owner_id, commit.clone())
        .await
        .unwrap();
    store.commit_execution(owner_id, commit).await.unwrap();

    let conflicting = RuntimeCommand::cancel(command.id(), session.id(), queued.id()).unwrap();
    assert_eq!(
        store
            .commit_execution(
                owner_id,
                ExecutionCommit::new(
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
                )
            )
            .await
            .unwrap_err()
            .code(),
        ExecutionStoreErrorCode::CommandConflict
    );
}

#[tokio::test]
async fn commands_bind_session_kind_and_exact_run_transition() {
    let store = InMemoryExecutionStore::default();
    let owner = id(100);
    let owner_id = owner;
    let session = Session::new(id(101), "writer", 1, SessionConcurrencyPolicy::Serial).unwrap();
    let queued = Run::queued(id(102), session.id(), "writer", 1).unwrap();
    let created = store
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                owner,
                session.clone(),
                queued.clone(),
                0,
                SessionConcurrencyPolicy::Serial,
            ),
        )
        .await
        .unwrap();
    let lease = store
        .acquire_lease(owner_id, queued.id(), created.run_version(), 1_000)
        .await
        .unwrap();
    let running = queued.transition(RunState::Running, None).unwrap();
    let event = RuntimeEvent::new(
        id(103),
        owner,
        session.id(),
        queued.id(),
        1,
        1,
        RuntimeEventKind::RunStarted,
    )
    .unwrap();
    for command in [
        RuntimeCommand::start(id(104), id(999), queued.id()).unwrap(),
        RuntimeCommand::cancel(id(105), session.id(), queued.id()).unwrap(),
    ] {
        assert_eq!(
            store
                .commit_execution(
                    owner_id,
                    ExecutionCommit::new(
                        created.run_version(),
                        0,
                        lease.clone(),
                        command,
                        vec![event.clone()],
                        vec![],
                        vec![],
                        vec![],
                        None,
                        running.clone(),
                    )
                )
                .await
                .unwrap_err()
                .code(),
            ExecutionStoreErrorCode::InvalidRequest
        );
    }
    let started = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                created.run_version(),
                0,
                lease.clone(),
                RuntimeCommand::start(id(106), session.id(), queued.id()).unwrap(),
                vec![event],
                vec![],
                vec![],
                vec![],
                None,
                running.clone(),
            ),
        )
        .await
        .unwrap();
    let advanced = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                started.stored_run().run_version(),
                0,
                lease.clone(),
                RuntimeCommand::record_progress(id(107), session.id(), queued.id()).unwrap(),
                vec![],
                vec![],
                vec![],
                vec![],
                None,
                running.clone(),
            ),
        )
        .await
        .unwrap();
    assert_eq!(advanced.stored_run().run(), &running);
    assert_eq!(
        store
            .commit_execution(
                owner_id,
                ExecutionCommit::new(
                    advanced.stored_run().run_version(),
                    0,
                    lease,
                    RuntimeCommand::start(id(108), session.id(), queued.id()).unwrap(),
                    vec![],
                    vec![],
                    vec![],
                    vec![],
                    None,
                    running,
                )
            )
            .await
            .unwrap_err()
            .code(),
        ExecutionStoreErrorCode::InvalidRequest
    );
}

#[tokio::test]
async fn commit_batches_are_bounded_and_history_reads_are_paged() {
    let store = InMemoryExecutionStore::default();
    let owner = id(110);
    let owner_id = owner;
    let session = Session::new(id(111), "writer", 1, SessionConcurrencyPolicy::Serial).unwrap();
    let queued = Run::queued(id(112), session.id(), "writer", 1).unwrap();
    let created = store
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                owner,
                session.clone(),
                queued.clone(),
                0,
                SessionConcurrencyPolicy::Serial,
            ),
        )
        .await
        .unwrap();
    let lease = store
        .acquire_lease(owner_id, queued.id(), created.run_version(), 1_000)
        .await
        .unwrap();
    let running = queued.transition(RunState::Running, None).unwrap();
    let oversized_events = (0..=MAX_COMMIT_EVENTS)
        .map(|offset| {
            let sequence = u64::try_from(offset).unwrap() + 1;
            RuntimeEvent::new(
                id(0x1_000 + u128::from(offset as u64)),
                owner,
                session.id(),
                queued.id(),
                sequence,
                sequence,
                RuntimeEventKind::StepStarted,
            )
            .unwrap()
        })
        .collect();
    assert_eq!(
        store
            .commit_execution(
                owner_id,
                ExecutionCommit::new(
                    created.run_version(),
                    0,
                    lease.clone(),
                    RuntimeCommand::start(id(113), session.id(), queued.id()).unwrap(),
                    oversized_events,
                    vec![],
                    vec![],
                    vec![],
                    None,
                    running.clone(),
                )
            )
            .await
            .unwrap_err()
            .code(),
        ExecutionStoreErrorCode::BoundsExceeded
    );
    assert_eq!(
        store
            .load_run(owner_id, queued.id())
            .await
            .unwrap()
            .unwrap(),
        created
    );

    let steps = vec![
        ExecutionStep::new(queued.id(), "page-a", anima_core::StepKind::Capability).unwrap(),
        ExecutionStep::new(queued.id(), "page-b", anima_core::StepKind::Capability).unwrap(),
        ExecutionStep::new(queued.id(), "page-c", anima_core::StepKind::Capability).unwrap(),
    ];
    let events = [
        RuntimeEventKind::RunStarted,
        RuntimeEventKind::StepStarted,
        RuntimeEventKind::StepCompleted,
    ]
    .into_iter()
    .enumerate()
    .map(|(offset, kind)| {
        let sequence = u64::try_from(offset).unwrap() + 1;
        RuntimeEvent::new(
            id(114 + u128::try_from(offset).unwrap()),
            owner,
            session.id(),
            queued.id(),
            sequence,
            sequence,
            kind,
        )
        .unwrap()
    })
    .collect::<Vec<_>>();
    let first_commit = store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                created.run_version(),
                0,
                lease.clone(),
                RuntimeCommand::start(id(115), session.id(), queued.id()).unwrap(),
                events.clone(),
                steps.clone(),
                vec![],
                vec![],
                None,
                running,
            ),
        )
        .await
        .unwrap();
    let first_steps = store
        .load_steps_page(owner_id, queued.id(), StoreReadPage::first(2).unwrap())
        .await
        .unwrap();
    assert_eq!(first_steps.items(), &steps[..2]);
    assert!(first_steps.next_cursor().is_some());
    let inserted_between_pages =
        ExecutionStep::new(queued.id(), "page-0", anima_core::StepKind::Capability).unwrap();
    store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
                first_commit.stored_run().run_version(),
                0,
                store.renew_lease(owner_id, lease, 1_000).await.unwrap(),
                RuntimeCommand::record_progress(id(118), session.id(), queued.id()).unwrap(),
                vec![],
                vec![inserted_between_pages],
                vec![],
                vec![],
                None,
                first_commit.stored_run().run().clone(),
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .load_steps_page(
                owner_id,
                queued.id(),
                StoreReadPage::after(first_steps.next_cursor().unwrap().clone(), 2).unwrap(),
            )
            .await
            .unwrap()
            .into_items(),
        steps[2..]
    );
    assert!(StoreReadPage::first(0).is_err());
    let first_events = store
        .replay_events(owner_id, queued.id(), StoreReadPage::first(2).unwrap())
        .await
        .unwrap();
    assert_eq!(first_events.events(), &events[..2]);
    assert!(first_events.next_cursor().is_some());
    let last_events = store
        .replay_events(
            owner_id,
            queued.id(),
            StoreReadPage::after(first_events.next_cursor().unwrap().clone(), 2).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(last_events.events(), &events[2..]);
    assert!(last_events.next_cursor().is_none());
    assert!(StoreReadPage::first(anima_core::MAX_STORE_READ_PAGE_SIZE + 1).is_err());
}

#[tokio::test]
async fn reclaimed_lease_fences_stale_execution_and_commits_contiguous_events() {
    let clock = ManualExecutionClock::default();
    let store = InMemoryExecutionStore::with_clock(Arc::new(clock.clone()));
    let owner_id = id(13);
    let session = Session::new(id(10), "writer", 1, SessionConcurrencyPolicy::Serial).unwrap();
    let queued = Run::queued(id(11), session.id(), "writer", 1).unwrap();
    store
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                id(13),
                session.clone(),
                queued.clone(),
                0,
                SessionConcurrencyPolicy::Serial,
            ),
        )
        .await
        .unwrap();

    let stale = store
        .acquire_lease(owner_id, queued.id(), 1, 100)
        .await
        .unwrap();
    clock.advance_ms(250).unwrap();
    let current = store
        .acquire_lease(owner_id, queued.id(), 1, 1_000)
        .await
        .unwrap();
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
            .commit_execution(
                owner_id,
                ExecutionCommit::new(
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
                )
            )
            .await
            .unwrap_err()
            .code(),
        ExecutionStoreErrorCode::LeaseExpired
    );

    store
        .commit_execution(
            owner_id,
            ExecutionCommit::new(
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
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .replay_events(owner_id, queued.id(), StoreReadPage::first(256).unwrap())
            .await
            .unwrap()
            .events(),
        &[event]
    );
}

/// Reusable adapter conformance case: serial sessions hold one nonterminal run claim.
pub async fn serial_session_claim_contract<S: ExecutionStore>(store: &S) {
    let owner_id = id(5);
    let session = Session::new(id(1), "writer", 1, SessionConcurrencyPolicy::Serial).unwrap();
    let first = Run::queued(id(2), session.id(), "writer", 1).unwrap();
    let second = Run::queued(id(3), session.id(), "writer", 1).unwrap();

    store
        .create_run(
            owner_id,
            CreateRun::new_for_owner(
                id(5),
                session.clone(),
                first,
                0,
                SessionConcurrencyPolicy::Serial,
            ),
        )
        .await
        .unwrap();

    assert_eq!(
        store
            .create_run(
                owner_id,
                CreateRun::new_for_owner(
                    id(5),
                    session.clone(),
                    second,
                    1,
                    SessionConcurrencyPolicy::Serial,
                )
            )
            .await
            .unwrap_err()
            .code(),
        ExecutionStoreErrorCode::ActiveRunConflict
    );
    assert_eq!(
        store
            .create_run(
                owner_id,
                CreateRun::new_for_owner(
                    id(5),
                    session,
                    Run::queued(id(4), id(1), "writer", 1).unwrap(),
                    0,
                    SessionConcurrencyPolicy::Serial,
                )
            )
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

#[test]
fn small_commits_are_structurally_bounded_to_a_mutation_patch() {
    let source = include_str!("../src/execution/memory_store.rs").replace("\r\n", "\n");
    let patch_builder = source
        .split("fn build_commit_patch")
        .nth(1)
        .and_then(|tail| tail.split("fn apply_commit_patch").next())
        .expect("bounded patch builder must remain inspectable");
    assert!(
        source.contains("struct CommitPatch"),
        "commit application must build a bounded mutation patch"
    );
    assert!(
        !source.contains("#[derive(Clone)]\nstruct RunAggregate"),
        "the full run aggregate must not be clonable by commit application"
    );
    assert!(!patch_builder.contains(".values()"));
    assert!(!patch_builder.contains("aggregate.events.iter()"));
}

#[test]
fn lease_conformance_uses_a_manual_clock_without_wall_time_waits() {
    let memory_store = include_str!("../src/execution/memory_store.rs");
    let conformance = include_str!("../src/execution/store.rs");
    let contract = include_str!("execution_store_contract.rs");
    assert!(memory_store.contains("pub struct ManualExecutionClock"));
    assert!(!conformance.contains("futures_timer::Delay"));
    let wall_time_sleep = ["std::thread", "::sleep"].concat();
    assert!(!contract.contains(&wall_time_sleep));
}
