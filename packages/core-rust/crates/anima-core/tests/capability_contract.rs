use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use anima_core::{
    CapabilityAttempt, CapabilityError, CapabilityErrorCode, CapabilityExecutionContext,
    CapabilityExecutor, CapabilityKind, CapabilityManifest, CapabilityRegistry,
    CapabilityRegistryError, CapabilityResult, LogicalInvocation, ManifestCatalog,
    ReconcileOutcome, RecoveryActionKind, RecoveryMode, RiskLevel, RuntimeCompatibility,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use uuid::Uuid;

fn manifest(id: &str, version: u32, recovery_mode: RecoveryMode) -> CapabilityManifest {
    CapabilityManifest {
        id: id.into(),
        version,
        kind: CapabilityKind::Automation,
        label: id.into(),
        description: "portable contract".into(),
        input_schema: json!({
            "type": "object",
            "required": ["query"],
            "properties": { "query": { "type": "string" } },
            "additionalProperties": false
        }),
        output_schema: json!({
            "type": "object",
            "required": ["ok"],
            "properties": { "ok": { "type": "boolean" } },
            "additionalProperties": false
        }),
        side_effects: true,
        risk_level: RiskLevel::Low,
        host_permissions: vec![],
        secret_references: vec!["github-token".into()],
        environment_requirements: vec![],
        timeout_ms: 5_000,
        cancellation_supported: true,
        max_retries: 2,
        idempotent: false,
        recovery_mode,
        supports_streaming: false,
        supports_artifacts: false,
        supports_citations: false,
        schema_digest: format!("sha256:{id}:{version}"),
        compatibility: RuntimeCompatibility {
            minimum_runtime_schema_version: 1,
            maximum_runtime_schema_version: 1,
            manifest_schema_version: 1,
        },
    }
}

fn registry(manifests: Vec<CapabilityManifest>) -> CapabilityRegistry {
    let mut catalog = ManifestCatalog::default();
    for manifest in manifests {
        catalog.register_manifest(manifest).unwrap();
    }
    CapabilityRegistry::new(catalog)
}

fn context(manifest: &CapabilityManifest, arguments: Value) -> CapabilityExecutionContext {
    let invocation = LogicalInvocation::new(
        Uuid::parse_str("e4d94bfa-7e8f-4874-a5c3-8f473ef71772").unwrap(),
        "step-7",
        manifest.id.clone(),
        manifest.version,
        arguments,
    );
    CapabilityExecutionContext::for_attempt(
        invocation.clone(),
        CapabilityAttempt::new(&invocation, 1),
    )
}

#[derive(Clone)]
struct RecordingExecutor {
    manifest: CapabilityManifest,
    execute_result: CapabilityResult,
    reconcile_result: ReconcileOutcome,
    executions: Arc<AtomicUsize>,
    reconciliations: Arc<AtomicUsize>,
    contexts: Arc<Mutex<Vec<CapabilityExecutionContext>>>,
}

impl RecordingExecutor {
    fn new(manifest: CapabilityManifest) -> Self {
        Self {
            manifest,
            execute_result: CapabilityResult::new(json!({ "ok": true })),
            reconcile_result: ReconcileOutcome::Pending,
            executions: Arc::new(AtomicUsize::new(0)),
            reconciliations: Arc::new(AtomicUsize::new(0)),
            contexts: Arc::new(Mutex::new(vec![])),
        }
    }
}

#[async_trait]
impl CapabilityExecutor for RecordingExecutor {
    fn manifest(&self) -> &CapabilityManifest {
        &self.manifest
    }

    async fn execute(
        &self,
        context: CapabilityExecutionContext,
    ) -> Result<CapabilityResult, CapabilityError> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        self.contexts.lock().unwrap().push(context);
        Ok(self.execute_result.clone())
    }

    async fn reconcile(
        &self,
        _context: CapabilityExecutionContext,
    ) -> Result<ReconcileOutcome, CapabilityError> {
        self.reconciliations.fetch_add(1, Ordering::SeqCst);
        Ok(self.reconcile_result.clone())
    }
}

#[tokio::test]
async fn registry_uses_exact_executor_versions_without_latest_fallback() {
    let v1 = manifest("workspace.apply", 1, RecoveryMode::KeyedIdempotent);
    let v2 = manifest("workspace.apply", 2, RecoveryMode::KeyedIdempotent);
    let mut registry = registry(vec![v1.clone(), v2.clone()]);
    registry
        .register_executor(Arc::new(RecordingExecutor::new(v2.clone())))
        .unwrap();

    let error = registry
        .execute(context(&v1, json!({ "query": "hello" })))
        .await
        .unwrap_err();
    assert_eq!(error.code, CapabilityErrorCode::Unavailable);
    assert!(registry.executor("workspace.apply", 1).is_none());
    assert!(registry.executor("workspace.apply", 2).is_some());
}

#[test]
fn executor_registration_rejects_duplicates_and_mismatches_transactionally() {
    let manifest = manifest("workspace.apply", 1, RecoveryMode::KeyedIdempotent);
    let mut registry = registry(vec![manifest.clone()]);
    registry
        .register_executor(Arc::new(RecordingExecutor::new(manifest.clone())))
        .unwrap();
    let before = registry.executor("workspace.apply", 1).unwrap();

    assert!(matches!(
        registry.register_executor(Arc::new(RecordingExecutor::new(manifest.clone()))),
        Err(CapabilityRegistryError::DuplicateExecutor { .. })
    ));
    let mut mismatch = manifest.clone();
    mismatch.version = 2;
    assert!(matches!(
        registry.register_executor(Arc::new(RecordingExecutor::new(mismatch))),
        Err(CapabilityRegistryError::ManifestExecutorMismatch { .. })
    ));
    assert!(Arc::ptr_eq(
        &before,
        &registry.executor("workspace.apply", 1).unwrap()
    ));
}

#[tokio::test]
async fn registry_validates_input_before_entering_the_executor() {
    let manifest = manifest("workspace.apply", 1, RecoveryMode::KeyedIdempotent);
    let executor = RecordingExecutor::new(manifest.clone());
    let executions = executor.executions.clone();
    let mut registry = registry(vec![manifest.clone()]);
    registry.register_executor(Arc::new(executor)).unwrap();

    let error = registry
        .execute(context(&manifest, json!({ "query": 3 })))
        .await
        .unwrap_err();
    assert_eq!(error.code, CapabilityErrorCode::Validation);
    assert_eq!(executions.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn registry_validates_output_before_returning_a_persistable_result() {
    let manifest = manifest("workspace.apply", 1, RecoveryMode::KeyedIdempotent);
    let mut executor = RecordingExecutor::new(manifest.clone());
    executor.execute_result = CapabilityResult::new(json!({ "ok": "not-a-boolean" }));
    let mut registry = registry(vec![manifest.clone()]);
    registry.register_executor(Arc::new(executor)).unwrap();

    let error = registry
        .execute(context(&manifest, json!({ "query": "hello" })))
        .await
        .unwrap_err();
    assert_eq!(error.code, CapabilityErrorCode::OutputValidation);
}

#[tokio::test]
async fn unavailable_exact_pinned_manifest_is_reported_safely() {
    let manifest = manifest("workspace.apply", 1, RecoveryMode::KeyedIdempotent);
    let registry = registry(vec![manifest.clone()]);

    let error = registry
        .execute(context(&manifest, json!({ "query": "hello" })))
        .await
        .unwrap_err();
    assert_eq!(error.code, CapabilityErrorCode::Unavailable);
    assert!(!error.retryable);
    assert!(!error.message.contains("token"));
}

#[test]
fn logical_invocations_are_stable_for_normalized_arguments_and_pinned_identity() {
    let run = Uuid::parse_str("e4d94bfa-7e8f-4874-a5c3-8f473ef71772").unwrap();
    let first = LogicalInvocation::new(
        run,
        "step-7",
        "workspace.apply",
        1,
        json!({ "a": 1, "b": 2 }),
    );
    let reordered = LogicalInvocation::new(
        run,
        "step-7",
        "workspace.apply",
        1,
        json!({ "b": 2, "a": 1 }),
    );
    let changed_arguments = LogicalInvocation::new(
        run,
        "step-7",
        "workspace.apply",
        1,
        json!({ "a": 2, "b": 2 }),
    );
    let changed_version = LogicalInvocation::new(
        run,
        "step-7",
        "workspace.apply",
        2,
        json!({ "a": 1, "b": 2 }),
    );

    assert_eq!(first.id, reordered.id);
    assert_eq!(first.idempotency_key, reordered.idempotency_key);
    assert_eq!(first.id.get_version_num(), 5);
    assert_ne!(first.id, changed_arguments.id);
    assert_ne!(first.id, changed_version.id);
}

#[test]
fn attempts_are_append_only_history_and_do_not_change_logical_identity_or_key() {
    let invocation = LogicalInvocation::new(
        Uuid::nil(),
        "step-7",
        "workspace.apply",
        1,
        json!({ "query": "hello" }),
    );
    let first = CapabilityAttempt::new(&invocation, 1);
    let retry = CapabilityAttempt::new(&invocation, 2);
    let same_logical_retry = LogicalInvocation::new(
        Uuid::nil(),
        "step-7",
        "workspace.apply",
        1,
        json!({ "query": "hello" }),
    );

    assert_ne!(first.id, retry.id);
    assert_eq!(invocation.id, same_logical_retry.id);
    assert_eq!(
        invocation.idempotency_key,
        same_logical_retry.idempotency_key
    );
    assert_eq!(first.logical_invocation_id, retry.logical_invocation_id);
}

#[tokio::test]
async fn recovery_decides_retry_reconcile_or_manual_without_automatic_execution() {
    let modes = [
        (
            RecoveryMode::InherentlyIdempotent,
            false,
            RecoveryActionKind::RetrySameKey,
        ),
        (
            RecoveryMode::KeyedIdempotent,
            false,
            RecoveryActionKind::RetrySameKey,
        ),
        (
            RecoveryMode::Reconcilable,
            true,
            RecoveryActionKind::Pending,
        ),
        (
            RecoveryMode::NonRetryable,
            false,
            RecoveryActionKind::RecoveryRequired,
        ),
    ];

    for (mode, calls_reconciler, expected) in modes {
        let manifest = manifest("workspace.apply", 1, mode);
        let executor = RecordingExecutor::new(manifest.clone());
        let reconciliations = executor.reconciliations.clone();
        let mut registry = registry(vec![manifest.clone()]);
        registry.register_executor(Arc::new(executor)).unwrap();

        let action = registry
            .recover(context(&manifest, json!({ "query": "hello" })))
            .await
            .unwrap();
        assert_eq!(action.kind(), expected);
        assert_eq!(reconciliations.load(Ordering::SeqCst) > 0, calls_reconciler);
    }
}

#[tokio::test]
async fn reconcilers_return_all_portable_recovery_outcomes() {
    let manifest = manifest("workspace.apply", 1, RecoveryMode::Reconcilable);
    for (outcome, expected) in [
        (
            ReconcileOutcome::Completed(CapabilityResult::new(json!({ "ok": true }))),
            RecoveryActionKind::Completed,
        ),
        (ReconcileOutcome::Pending, RecoveryActionKind::Pending),
        (
            ReconcileOutcome::AuthoritativeAbsence,
            RecoveryActionKind::AuthoritativeAbsence,
        ),
        (
            ReconcileOutcome::RecoveryRequired,
            RecoveryActionKind::RecoveryRequired,
        ),
    ] {
        let mut executor = RecordingExecutor::new(manifest.clone());
        executor.reconcile_result = outcome;
        let mut registry = registry(vec![manifest.clone()]);
        registry.register_executor(Arc::new(executor)).unwrap();
        let action = registry
            .recover(context(&manifest, json!({ "query": "hello" })))
            .await
            .unwrap();
        assert_eq!(action.kind(), expected);
    }
}

#[test]
fn capability_errors_have_stable_safe_codes_without_upstream_diagnostics() {
    for (code, retryable) in [
        (CapabilityErrorCode::Validation, false),
        (CapabilityErrorCode::Unavailable, false),
        (CapabilityErrorCode::Timeout, true),
        (CapabilityErrorCode::Cancelled, false),
        (CapabilityErrorCode::Execution, true),
        (CapabilityErrorCode::OutputValidation, false),
        (CapabilityErrorCode::Reconciliation, true),
    ] {
        let error = CapabilityError::new(code, "safe message", retryable);
        let serialized = serde_json::to_string(&error).unwrap();
        assert_eq!(error.code, code);
        assert_eq!(error.retryable, retryable);
        assert!(!serialized.contains("upstream-body"));
    }
}

#[test]
fn execution_context_is_serde_safe_and_never_contains_raw_credentials() {
    let manifest = manifest("workspace.apply", 1, RecoveryMode::KeyedIdempotent);
    let mut context = context(&manifest, json!({ "query": "hello" }));
    context.secret_references = vec!["github-token".into()];
    context.owner_reference = Some("owner:42".into());
    context.agent_reference = Some("agent:7".into());
    context.session_reference = Some("session:9".into());
    context.workspace_reference = Some("workspace:11".into());
    context.deadline_reference = Some("deadline:soon".into());
    context.cancellation_reference = Some("cancel:request-2".into());

    let serialized = serde_json::to_string(&context).unwrap();
    let debug = format!("{context:?}");
    for representation in [&serialized, &debug] {
        assert!(representation.contains("github-token"));
        assert!(!representation.contains("super-secret-access-token"));
        assert!(!representation.contains("credentials"));
    }
}
