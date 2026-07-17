use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use anima_core::{
    CapabilityAttempt, CapabilityError, CapabilityErrorCode, CapabilityExecutionContext,
    CapabilityExecutor, CapabilityKind, CapabilityManifest, CapabilityReferenceId,
    CapabilityRegistry, CapabilityRegistryError, CapabilityResult, CapabilitySecretReferenceId,
    LogicalInvocation, ManifestCatalog, ReconcileOutcome, RecoveryActionKind, RecoveryMode,
    RiskLevel, RuntimeCompatibility,
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
    )
    .unwrap();
    CapabilityExecutionContext::for_attempt(
        invocation.clone(),
        CapabilityAttempt::new(&invocation, 1).unwrap(),
    )
    .unwrap()
}

fn context_for_attempt(
    manifest: &CapabilityManifest,
    arguments: Value,
    number: u32,
) -> CapabilityExecutionContext {
    let invocation = LogicalInvocation::new(
        Uuid::parse_str("e4d94bfa-7e8f-4874-a5c3-8f473ef71772").unwrap(),
        "step-7",
        manifest.id.clone(),
        manifest.version,
        arguments,
    )
    .unwrap();
    CapabilityExecutionContext::for_attempt(
        invocation.clone(),
        CapabilityAttempt::new(&invocation, number).unwrap(),
    )
    .unwrap()
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

struct SecretLeakingExecutor {
    manifest: CapabilityManifest,
    upstream_diagnostic: String,
}

#[async_trait]
impl CapabilityExecutor for SecretLeakingExecutor {
    fn manifest(&self) -> &CapabilityManifest {
        &self.manifest
    }

    async fn execute(
        &self,
        _context: CapabilityExecutionContext,
    ) -> Result<CapabilityResult, CapabilityError> {
        assert!(!self.upstream_diagnostic.is_empty());
        Err(CapabilityError::execution())
    }

    async fn reconcile(
        &self,
        _context: CapabilityExecutionContext,
    ) -> Result<ReconcileOutcome, CapabilityError> {
        assert!(!self.upstream_diagnostic.is_empty());
        Err(CapabilityError::reconciliation())
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
    assert_eq!(error.code(), CapabilityErrorCode::Unavailable);
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

    let mut contract_mismatch = manifest.clone();
    contract_mismatch.label = "different immutable contract".into();
    assert!(matches!(
        registry.register_executor(Arc::new(RecordingExecutor::new(contract_mismatch))),
        Err(CapabilityRegistryError::ManifestExecutorMismatch { .. })
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
    assert_eq!(error.code(), CapabilityErrorCode::Validation);
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
    assert_eq!(error.code(), CapabilityErrorCode::OutputValidation);
}

#[tokio::test]
async fn unavailable_exact_pinned_manifest_is_reported_safely() {
    let manifest = manifest("workspace.apply", 1, RecoveryMode::KeyedIdempotent);
    let registry = registry(vec![manifest.clone()]);

    let error = registry
        .execute(context(&manifest, json!({ "query": "hello" })))
        .await
        .unwrap_err();
    assert_eq!(error.code(), CapabilityErrorCode::Unavailable);
    assert!(!error.retryable());
    assert!(!error.message().contains("token"));
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
    )
    .unwrap();
    let reordered = LogicalInvocation::new(
        run,
        "step-7",
        "workspace.apply",
        1,
        json!({ "b": 2, "a": 1 }),
    )
    .unwrap();
    let changed_arguments = LogicalInvocation::new(
        run,
        "step-7",
        "workspace.apply",
        1,
        json!({ "a": 2, "b": 2 }),
    )
    .unwrap();
    let changed_version = LogicalInvocation::new(
        run,
        "step-7",
        "workspace.apply",
        2,
        json!({ "a": 1, "b": 2 }),
    )
    .unwrap();

    assert_eq!(first.id(), reordered.id());
    assert_eq!(first.idempotency_key(), reordered.idempotency_key());
    assert_eq!(first.id().get_version_num(), 5);
    assert_ne!(first.id(), changed_arguments.id());
    assert_ne!(first.id(), changed_version.id());
}

#[test]
fn attempts_are_append_only_history_and_do_not_change_logical_identity_or_key() {
    let invocation = LogicalInvocation::new(
        Uuid::nil(),
        "step-7",
        "workspace.apply",
        1,
        json!({ "query": "hello" }),
    )
    .unwrap();
    let first = CapabilityAttempt::new(&invocation, 1).unwrap();
    let retry = CapabilityAttempt::new(&invocation, 2).unwrap();
    let same_logical_retry = LogicalInvocation::new(
        Uuid::nil(),
        "step-7",
        "workspace.apply",
        1,
        json!({ "query": "hello" }),
    )
    .unwrap();

    assert_ne!(first.id(), retry.id());
    assert_eq!(invocation.id(), same_logical_retry.id());
    assert_eq!(
        invocation.idempotency_key(),
        same_logical_retry.idempotency_key()
    );
    assert_eq!(first.logical_invocation_id(), retry.logical_invocation_id());
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
            RecoveryActionKind::RetrySameKey,
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
    for (error, code, retryable) in [
        (
            CapabilityError::validation(),
            CapabilityErrorCode::Validation,
            false,
        ),
        (
            CapabilityError::unavailable(),
            CapabilityErrorCode::Unavailable,
            false,
        ),
        (
            CapabilityError::timeout(),
            CapabilityErrorCode::Timeout,
            true,
        ),
        (
            CapabilityError::cancelled(),
            CapabilityErrorCode::Cancelled,
            false,
        ),
        (
            CapabilityError::execution(),
            CapabilityErrorCode::Execution,
            true,
        ),
        (
            CapabilityError::output_validation(),
            CapabilityErrorCode::OutputValidation,
            false,
        ),
        (
            CapabilityError::reconciliation(),
            CapabilityErrorCode::Reconciliation,
            true,
        ),
    ] {
        let serialized = serde_json::to_string(&error).unwrap();
        assert_eq!(error.code(), code);
        assert_eq!(error.retryable(), retryable);
        assert!(!serialized.contains("upstream-body"));
    }
}

#[test]
fn execution_context_is_serde_safe_and_never_contains_raw_credentials() {
    let manifest = manifest("workspace.apply", 1, RecoveryMode::KeyedIdempotent);
    let baseline = context(&manifest, json!({ "query": "hello" }));
    let references = baseline
        .references()
        .clone()
        .with_owner(CapabilityReferenceId::parse("owner:42").unwrap())
        .with_agent(CapabilityReferenceId::parse("agent:7").unwrap())
        .with_session(CapabilityReferenceId::parse("session:9").unwrap())
        .with_workspace(CapabilityReferenceId::parse("workspace:11").unwrap())
        .with_deadline(CapabilityReferenceId::parse("deadline:soon").unwrap())
        .with_cancellation(CapabilityReferenceId::parse("cancel:request-2").unwrap())
        .with_secrets(vec![CapabilitySecretReferenceId::parse(
            "secret:github-token",
        )
        .unwrap()]);
    let context = baseline.with_references(references);

    let serialized = serde_json::to_string(&context).unwrap();
    let debug = format!("{context:?}");
    assert!(serialized.contains("secret:github-token"));
    assert!(!debug.contains("github-token"));
    for representation in [&serialized, &debug] {
        assert!(!representation.contains("super-secret-access-token"));
        assert!(!representation.contains("credentials"));
    }
}

#[tokio::test]
async fn tampered_serialized_context_cannot_reuse_an_existing_logical_identity() {
    let manifest = manifest("workspace.apply", 1, RecoveryMode::KeyedIdempotent);
    let mut registry = registry(vec![manifest.clone()]);
    registry
        .register_executor(Arc::new(RecordingExecutor::new(manifest.clone())))
        .unwrap();
    let original = context(&manifest, json!({ "query": "safe" }));
    let mut serialized = serde_json::to_value(&original).unwrap();
    serialized["invocation"]["normalized_arguments"] = json!({ "query": "tampered" });

    assert!(serde_json::from_value::<CapabilityExecutionContext>(serialized).is_err());
    let mut tampered_reference = serde_json::to_value(&original).unwrap();
    tampered_reference["references"]["run"] = json!("run:not-the-logical-run");
    assert!(serde_json::from_value::<CapabilityExecutionContext>(tampered_reference).is_err());
    assert!(registry.execute(original).await.is_ok());
}

#[test]
fn logical_identity_seed_cannot_collide_when_fields_contain_old_delimiters() {
    let run = Uuid::nil();
    let first = LogicalInvocation::new(
        run,
        "step\u{1f}capability=other",
        "workspace.apply",
        1,
        json!({ "query": "hello" }),
    )
    .unwrap();
    let second = LogicalInvocation::new(
        run,
        "step",
        "other\u{1f}capability=workspace.apply",
        1,
        json!({ "query": "hello" }),
    )
    .unwrap();

    assert_ne!(first.id(), second.id());
}

#[tokio::test]
async fn portable_errors_never_serialize_or_propagate_raw_upstream_secrets() {
    const SECRET: &str = "super-secret-upstream-body";
    let manifest = manifest("workspace.apply", 1, RecoveryMode::Reconcilable);
    let mut registry = registry(vec![manifest.clone()]);
    registry
        .register_executor(Arc::new(SecretLeakingExecutor {
            manifest: manifest.clone(),
            upstream_diagnostic: SECRET.into(),
        }))
        .unwrap();
    assert!(serde_json::from_value::<CapabilityError>(json!({
        "code": "execution",
        "message": SECRET,
    }))
    .is_err());

    for error in [
        CapabilityError::validation(),
        CapabilityError::unavailable(),
        CapabilityError::timeout(),
        CapabilityError::cancelled(),
        CapabilityError::execution(),
        CapabilityError::output_validation(),
        CapabilityError::reconciliation(),
        registry
            .execute(context(&manifest, json!({ "query": "hello" })))
            .await
            .unwrap_err(),
        registry
            .recover(context(&manifest, json!({ "query": "hello" })))
            .await
            .unwrap_err(),
    ] {
        for representation in [
            serde_json::to_string(&error).unwrap(),
            format!("{error:?}"),
            error.to_string(),
        ] {
            assert!(!representation.contains(SECRET));
        }
    }
}

#[tokio::test]
async fn initial_execution_rejects_retry_attempts_before_the_executor() {
    let manifest = manifest("workspace.apply", 1, RecoveryMode::KeyedIdempotent);
    let executor = RecordingExecutor::new(manifest.clone());
    let calls = executor.executions.clone();
    let mut registry = registry(vec![manifest.clone()]);
    registry.register_executor(Arc::new(executor)).unwrap();
    let invocation = LogicalInvocation::new(
        Uuid::nil(),
        "step-7",
        manifest.id.clone(),
        manifest.version,
        json!({ "query": "hello" }),
    )
    .unwrap();
    let retry_context = CapabilityExecutionContext::for_attempt(
        invocation.clone(),
        CapabilityAttempt::new(&invocation, 2).unwrap(),
    )
    .unwrap();

    let error = registry.execute(retry_context).await.unwrap_err();
    assert_eq!(error.code(), CapabilityErrorCode::Validation);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn recovery_authorizations_are_exact_one_time_and_bounded() {
    let mut manifest = manifest("workspace.apply", 1, RecoveryMode::KeyedIdempotent);
    manifest.max_retries = 1;
    let mut registry = registry(vec![manifest.clone()]);
    registry
        .register_executor(Arc::new(RecordingExecutor::new(manifest.clone())))
        .unwrap();
    let initial = context(&manifest, json!({ "query": "hello" }));
    registry.execute(initial.clone()).await.unwrap();

    let action = registry.recover(initial).await.unwrap();
    let authorization = action.retry_authorization().unwrap().clone();
    let retry = context_for_attempt(&manifest, json!({ "query": "hello" }), 2);
    registry
        .execute_retry(retry.clone(), authorization.clone())
        .await
        .unwrap();
    assert_eq!(
        registry
            .execute_retry(retry.clone(), authorization)
            .await
            .unwrap_err()
            .code(),
        CapabilityErrorCode::Validation
    );
    assert_eq!(
        registry.recover(retry).await.unwrap().kind(),
        RecoveryActionKind::RecoveryRequired
    );
}

#[tokio::test]
async fn recovery_authorizations_cannot_be_used_for_another_invocation() {
    let manifest = manifest("workspace.apply", 1, RecoveryMode::KeyedIdempotent);
    let mut registry = registry(vec![manifest.clone()]);
    registry
        .register_executor(Arc::new(RecordingExecutor::new(manifest.clone())))
        .unwrap();
    let initial = context(&manifest, json!({ "query": "one" }));
    let authorization = registry
        .recover(initial)
        .await
        .unwrap()
        .retry_authorization()
        .unwrap()
        .clone();
    let other = context_for_attempt(&manifest, json!({ "query": "two" }), 2);

    assert_eq!(
        registry
            .execute_retry(other, authorization)
            .await
            .unwrap_err()
            .code(),
        CapabilityErrorCode::Validation
    );
}

#[tokio::test]
async fn injected_reference_values_are_rejected_or_redacted_before_executor_entry() {
    let manifest = manifest("workspace.apply", 1, RecoveryMode::KeyedIdempotent);
    let executor = RecordingExecutor::new(manifest.clone());
    let calls = executor.executions.clone();
    let mut registry = registry(vec![manifest.clone()]);
    registry.register_executor(Arc::new(executor)).unwrap();
    let context = context(&manifest, json!({ "query": "hello" }));
    let mut serialized = serde_json::to_value(&context).unwrap();
    serialized["references"]["secrets"] = json!(["super-secret-upstream-body"]);
    assert!(serde_json::from_value::<CapabilityExecutionContext>(serialized).is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(CapabilityReferenceId::parse("https://token.example/path").is_err());
    assert!(CapabilitySecretReferenceId::parse("super-secret-upstream-body").is_err());
    assert!(!format!("{context:?}").contains("super-secret-upstream-body"));
}

#[test]
fn attempts_reject_standalone_serialized_tampering() {
    let manifest = manifest("workspace.apply", 1, RecoveryMode::KeyedIdempotent);
    let context = context(&manifest, json!({ "query": "hello" }));
    let mut attempt = serde_json::to_value(context.attempt()).unwrap();
    attempt["number"] = json!(2);
    assert!(serde_json::from_value::<CapabilityAttempt>(attempt).is_err());
}

#[test]
fn invocation_uses_jcs_numeric_equivalence_and_rejects_unbounded_arguments() {
    let first =
        LogicalInvocation::new(Uuid::nil(), "step", "capability", 1, json!({ "n": 1 })).unwrap();
    let equivalent =
        LogicalInvocation::new(Uuid::nil(), "step", "capability", 1, json!({ "n": 1.0 })).unwrap();
    assert_eq!(first.id(), equivalent.id());

    let oversized = Value::String("x".repeat(anima_core::MAX_CAPABILITY_ARGUMENT_BYTES + 1));
    assert!(LogicalInvocation::new(Uuid::nil(), "step", "capability", 1, oversized).is_err());

    let mut deep = json!(null);
    for _ in 0..=anima_core::MAX_CAPABILITY_ARGUMENT_DEPTH {
        deep = json!([deep]);
    }
    assert!(LogicalInvocation::new(Uuid::nil(), "step", "capability", 1, deep).is_err());
    let nodes = Value::Array(vec![Value::Null; anima_core::MAX_CAPABILITY_ARGUMENT_NODES]);
    assert!(LogicalInvocation::new(Uuid::nil(), "step", "capability", 1, nodes).is_err());
}

#[test]
fn registration_accepts_only_draft7_or_2020_12_schemas() {
    let mut unsupported = manifest("workspace.apply", 1, RecoveryMode::KeyedIdempotent);
    unsupported.input_schema["$schema"] = json!("http://json-schema.org/draft-06/schema#");
    let mut unsupported_registry = registry(vec![unsupported.clone()]);
    assert!(matches!(
        unsupported_registry.register_executor(Arc::new(RecordingExecutor::new(unsupported))),
        Err(CapabilityRegistryError::InvalidInputSchema { .. })
    ));

    let mut modern = manifest("workspace.apply", 1, RecoveryMode::KeyedIdempotent);
    modern.output_schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "array",
        "prefixItems": [{ "type": "string" }],
        "items": false
    });
    let mut modern_registry = registry(vec![modern.clone()]);
    modern_registry
        .register_executor(Arc::new(RecordingExecutor::new(modern)))
        .unwrap();
}

#[test]
fn capability_registry_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<CapabilityRegistry>();
}

#[tokio::test]
async fn registry_supports_concurrent_initial_execution() {
    let manifest = manifest("workspace.apply", 1, RecoveryMode::KeyedIdempotent);
    let mut mutable_registry = registry(vec![manifest.clone()]);
    mutable_registry
        .register_executor(Arc::new(RecordingExecutor::new(manifest.clone())))
        .unwrap();
    let registry = Arc::new(mutable_registry);
    let first = registry.execute(context(&manifest, json!({ "query": "one" })));
    let second = registry.execute(
        CapabilityExecutionContext::for_attempt(
            LogicalInvocation::new(
                Uuid::parse_str("58b3dfdc-6713-4f65-a6f0-88c3584d13d9").unwrap(),
                "step-8",
                manifest.id.clone(),
                manifest.version,
                json!({ "query": "two" }),
            )
            .unwrap(),
            CapabilityAttempt::new(
                &LogicalInvocation::new(
                    Uuid::parse_str("58b3dfdc-6713-4f65-a6f0-88c3584d13d9").unwrap(),
                    "step-8",
                    manifest.id.clone(),
                    manifest.version,
                    json!({ "query": "two" }),
                )
                .unwrap(),
                1,
            )
            .unwrap(),
        )
        .unwrap(),
    );
    let (first, second) = futures::join!(first, second);
    assert!(first.is_ok());
    assert!(second.is_ok());
}
