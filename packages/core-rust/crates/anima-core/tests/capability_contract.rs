use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicU64, AtomicUsize, Ordering},
    Arc, Mutex,
};

use anima_core::{
    AttemptRecordState, Budget, CapabilityAttempt, CapabilityAttemptLineageState,
    CapabilityContextError, CapabilityError, CapabilityErrorCode, CapabilityExecutionContext,
    CapabilityExecutionReferences, CapabilityExecutor, CapabilityKind, CapabilityLeaseKind,
    CapabilityLineageStore, CapabilityManifest, CapabilityReferenceId,
    CapabilityReferenceValidator, CapabilityRegistry, CapabilityRegistryError, CapabilityResult,
    CapabilityResultRecorder, CapabilitySecretReferenceId, CheckpointV1, CheckpointV1Builder,
    DefinitionPin, DurableCapabilityResult, ExecutionFence, InvocationAttemptRecord,
    LogicalInvocation, ManifestCatalog, ManifestCatalogError, ManifestPin, ReconcileOutcome,
    RecoveryActionKind, RecoveryMode, RecoveryPauseReason, RecoveryPauseRecord, RiskLevel, Run,
    RunPauseReason, RunState, RuntimeCommand, RuntimeCompatibility, UncertainInvocationRecord,
    Usage,
};
use async_trait::async_trait;
use futures::channel::oneshot;
use futures_timer::Delay;
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

struct RejectingResultRecorder;

#[async_trait]
impl CapabilityResultRecorder for RejectingResultRecorder {
    async fn record(
        &self,
        _context: &CapabilityExecutionContext,
        _manifest: &CapabilityManifest,
        _result: &CapabilityResult,
        _durable: &DurableCapabilityResult,
    ) -> Result<(), CapabilityError> {
        Err(CapabilityError::output_validation())
    }
}

#[derive(Default)]
struct StableRepeatedResultRecorder {
    records: Mutex<BTreeMap<Uuid, (DurableCapabilityResult, CapabilityResult)>>,
    calls: AtomicUsize,
}

impl StableRepeatedResultRecorder {
    fn resolve(&self, result_ref: &CapabilityReferenceId) -> Option<CapabilityResult> {
        self.records
            .lock()
            .unwrap()
            .get(&result_ref.handle())
            .map(|(_, result)| result.clone())
    }
}

#[async_trait]
impl CapabilityResultRecorder for StableRepeatedResultRecorder {
    async fn record(
        &self,
        _context: &CapabilityExecutionContext,
        _manifest: &CapabilityManifest,
        result: &CapabilityResult,
        durable: &DurableCapabilityResult,
    ) -> Result<(), CapabilityError> {
        let key = durable.result_ref().handle();
        let mut records = self.records.lock().unwrap();
        if let Some((recorded, recorded_result)) = records.get(&key) {
            if recorded != durable || recorded_result != result {
                return Err(CapabilityError::validation());
            }
        } else {
            records.insert(key, (durable.clone(), result.clone()));
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn registry_with_test_stores(
    catalog: ManifestCatalog,
    lineage: Arc<dyn CapabilityLineageStore>,
) -> CapabilityRegistry {
    CapabilityRegistry::with_stores(
        catalog,
        lineage,
        Arc::new(StableRepeatedResultRecorder::default()),
    )
}

#[derive(Default)]
struct TestLineageStore {
    states: Mutex<BTreeMap<(Uuid, u32), CapabilityAttemptLineageState>>,
    authoritative_now_ms: AtomicU64,
    renewals: AtomicUsize,
    fail_compensation_upgrade_once: AtomicUsize,
    fail_completion_cas_once: AtomicUsize,
}

#[async_trait]
impl CapabilityLineageStore for TestLineageStore {
    async fn load(
        &self,
        invocation_id: Uuid,
        attempt_number: u32,
    ) -> Result<Option<CapabilityAttemptLineageState>, CapabilityError> {
        Ok(self
            .states
            .lock()
            .unwrap()
            .get(&(invocation_id, attempt_number))
            .cloned())
    }

    async fn compare_exchange(
        &self,
        invocation_id: Uuid,
        attempt_number: u32,
        current: Option<CapabilityAttemptLineageState>,
        new: CapabilityAttemptLineageState,
    ) -> Result<bool, CapabilityError> {
        let mut states = self.states.lock().unwrap();
        if states.get(&(invocation_id, attempt_number)) != current.as_ref() {
            return Ok(false);
        }
        if current == Some(CapabilityAttemptLineageState::RecoveryRequired)
            && new == CapabilityAttemptLineageState::CompensationRequired
            && self
                .fail_compensation_upgrade_once
                .swap(0, Ordering::SeqCst)
                == 1
        {
            states.insert((invocation_id, attempt_number), new);
            return Ok(false);
        }
        if matches!(new, CapabilityAttemptLineageState::Completed(_))
            && self.fail_completion_cas_once.swap(0, Ordering::SeqCst) == 1
        {
            states.insert(
                (invocation_id, attempt_number),
                CapabilityAttemptLineageState::Uncertain,
            );
            return Ok(false);
        }
        states.insert((invocation_id, attempt_number), new);
        Ok(true)
    }

    async fn acquire_lease(
        &self,
        invocation_id: Uuid,
        attempt_number: u32,
        current: Option<CapabilityAttemptLineageState>,
        kind: CapabilityLeaseKind,
        lease_duration_ms: u64,
    ) -> Result<Option<CapabilityAttemptLineageState>, CapabilityError> {
        let mut states = self.states.lock().unwrap();
        if states.get(&(invocation_id, attempt_number)) != current.as_ref() {
            return Ok(None);
        }
        let fence = Uuid::new_v4();
        let lease_expires_at_ms = self
            .authoritative_now_ms
            .load(Ordering::SeqCst)
            .saturating_add(lease_duration_ms);
        let state = match kind {
            CapabilityLeaseKind::Executing => CapabilityAttemptLineageState::Executing {
                fence,
                lease_expires_at_ms,
            },
            CapabilityLeaseKind::RetryExecuting => CapabilityAttemptLineageState::RetryExecuting {
                fence,
                lease_expires_at_ms,
            },
            CapabilityLeaseKind::Reconciling => CapabilityAttemptLineageState::Reconciling {
                fence,
                lease_expires_at_ms,
            },
        };
        states.insert((invocation_id, attempt_number), state.clone());
        Ok(Some(state))
    }

    async fn renew_lease(
        &self,
        invocation_id: Uuid,
        attempt_number: u32,
        current: CapabilityAttemptLineageState,
        lease_duration_ms: u64,
    ) -> Result<Option<CapabilityAttemptLineageState>, CapabilityError> {
        let mut states = self.states.lock().unwrap();
        if states.get(&(invocation_id, attempt_number)) != Some(&current) {
            return Ok(None);
        }
        let now_ms = self.authoritative_now_ms.load(Ordering::SeqCst);
        let renewed = match current {
            CapabilityAttemptLineageState::Executing {
                fence,
                lease_expires_at_ms,
            } if lease_expires_at_ms > now_ms => CapabilityAttemptLineageState::Executing {
                fence,
                lease_expires_at_ms: now_ms.saturating_add(lease_duration_ms),
            },
            CapabilityAttemptLineageState::RetryExecuting {
                fence,
                lease_expires_at_ms,
            } if lease_expires_at_ms > now_ms => CapabilityAttemptLineageState::RetryExecuting {
                fence,
                lease_expires_at_ms: now_ms.saturating_add(lease_duration_ms),
            },
            CapabilityAttemptLineageState::Reconciling {
                fence,
                lease_expires_at_ms,
            } if lease_expires_at_ms > now_ms => CapabilityAttemptLineageState::Reconciling {
                fence,
                lease_expires_at_ms: now_ms.saturating_add(lease_duration_ms),
            },
            _ => return Ok(None),
        };
        states.insert((invocation_id, attempt_number), renewed.clone());
        self.renewals.fetch_add(1, Ordering::SeqCst);
        Ok(Some(renewed))
    }

    async fn expire_lease(
        &self,
        invocation_id: Uuid,
        attempt_number: u32,
        current: CapabilityAttemptLineageState,
        new: CapabilityAttemptLineageState,
    ) -> Result<bool, CapabilityError> {
        let mut states = self.states.lock().unwrap();
        if states.get(&(invocation_id, attempt_number)) != Some(&current) {
            return Ok(false);
        }
        let lease_expires_at_ms = match current {
            CapabilityAttemptLineageState::Executing {
                lease_expires_at_ms,
                ..
            }
            | CapabilityAttemptLineageState::RetryExecuting {
                lease_expires_at_ms,
                ..
            }
            | CapabilityAttemptLineageState::Reconciling {
                lease_expires_at_ms,
                ..
            } => lease_expires_at_ms,
            _ => return Ok(false),
        };
        if lease_expires_at_ms > self.authoritative_now_ms.load(Ordering::SeqCst) {
            return Ok(false);
        }
        states.insert((invocation_id, attempt_number), new);
        Ok(true)
    }

    async fn validate_effect_fence(
        &self,
        invocation_id: Uuid,
        attempt_number: u32,
        expected_kind: CapabilityLeaseKind,
        fence: Uuid,
    ) -> Result<bool, CapabilityError> {
        let states = self.states.lock().unwrap();
        let Some(state) = states.get(&(invocation_id, attempt_number)) else {
            return Ok(false);
        };
        let (active_kind, active_fence, lease_expires_at_ms) = match state {
            CapabilityAttemptLineageState::Executing {
                fence,
                lease_expires_at_ms,
            } => (CapabilityLeaseKind::Executing, *fence, *lease_expires_at_ms),
            CapabilityAttemptLineageState::RetryExecuting {
                fence,
                lease_expires_at_ms,
            } => (
                CapabilityLeaseKind::RetryExecuting,
                *fence,
                *lease_expires_at_ms,
            ),
            CapabilityAttemptLineageState::Reconciling {
                fence,
                lease_expires_at_ms,
            } => (
                CapabilityLeaseKind::Reconciling,
                *fence,
                *lease_expires_at_ms,
            ),
            _ => return Ok(false),
        };
        Ok(active_kind == expected_kind
            && active_fence == fence
            && lease_expires_at_ms > self.authoritative_now_ms.load(Ordering::SeqCst))
    }
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

async fn ensure_effect_fence(context: &CapabilityExecutionContext) -> Result<(), CapabilityError> {
    context
        .execution_fence()
        .ok_or_else(CapabilityError::execution)?
        .ensure_valid()
        .await
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
    failures_remaining: Arc<AtomicUsize>,
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
            failures_remaining: Arc::new(AtomicUsize::new(0)),
            executions: Arc::new(AtomicUsize::new(0)),
            reconciliations: Arc::new(AtomicUsize::new(0)),
            contexts: Arc::new(Mutex::new(vec![])),
        }
    }

    fn failing_once(manifest: CapabilityManifest) -> Self {
        let mut executor = Self::new(manifest);
        executor.failures_remaining.store(1, Ordering::SeqCst);
        executor.reconcile_result = ReconcileOutcome::AuthoritativeAbsence;
        executor
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
        ensure_effect_fence(&context).await?;
        self.executions.fetch_add(1, Ordering::SeqCst);
        self.contexts.lock().unwrap().push(context);
        if self
            .failures_remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            return Err(CapabilityError::execution());
        }
        Ok(self.execute_result.clone())
    }

    async fn reconcile(
        &self,
        context: CapabilityExecutionContext,
    ) -> Result<ReconcileOutcome, CapabilityError> {
        ensure_effect_fence(&context).await?;
        self.reconciliations.fetch_add(1, Ordering::SeqCst);
        Ok(self.reconcile_result.clone())
    }
}

struct SecretLeakingExecutor {
    manifest: CapabilityManifest,
    upstream_diagnostic: String,
}

struct BarrierExecutor {
    manifest: CapabilityManifest,
    execute_entered: Mutex<Option<oneshot::Sender<()>>>,
    execute_release: Mutex<Option<oneshot::Receiver<()>>>,
    reconcile_result: ReconcileOutcome,
    reconciliations: Arc<AtomicUsize>,
}

#[async_trait]
impl CapabilityExecutor for BarrierExecutor {
    fn manifest(&self) -> &CapabilityManifest {
        &self.manifest
    }

    async fn execute(
        &self,
        context: CapabilityExecutionContext,
    ) -> Result<CapabilityResult, CapabilityError> {
        ensure_effect_fence(&context).await?;
        if let Some(entered) = self.execute_entered.lock().unwrap().take() {
            let _ = entered.send(());
        }
        let release = self.execute_release.lock().unwrap().take();
        if let Some(release) = release {
            let _ = release.await;
        }
        Ok(CapabilityResult::new(json!({ "ok": true })))
    }

    async fn reconcile(
        &self,
        context: CapabilityExecutionContext,
    ) -> Result<ReconcileOutcome, CapabilityError> {
        ensure_effect_fence(&context).await?;
        self.reconciliations.fetch_add(1, Ordering::SeqCst);
        Ok(self.reconcile_result.clone())
    }
}

struct ConflictingReconcileExecutor {
    manifest: CapabilityManifest,
    reconcile_entered: Mutex<Option<oneshot::Sender<()>>>,
    reconcile_release: Mutex<Option<oneshot::Receiver<()>>>,
    reconciliations: Arc<AtomicUsize>,
}

struct RetryBarrierExecutor {
    manifest: CapabilityManifest,
    executions: AtomicUsize,
    retry_entered: Mutex<Option<oneshot::Sender<()>>>,
    retry_release: Mutex<Option<oneshot::Receiver<()>>>,
    reconciliations: Arc<AtomicUsize>,
}

struct FenceAwareBarrierExecutor {
    manifest: CapabilityManifest,
    entered: Mutex<Option<oneshot::Sender<ExecutionFence>>>,
    release: Mutex<Option<oneshot::Receiver<()>>>,
    external_calls: Arc<AtomicUsize>,
    reconcile_calls: AtomicUsize,
}

#[async_trait]
impl CapabilityExecutor for FenceAwareBarrierExecutor {
    fn manifest(&self) -> &CapabilityManifest {
        &self.manifest
    }

    async fn execute(
        &self,
        context: CapabilityExecutionContext,
    ) -> Result<CapabilityResult, CapabilityError> {
        let fence = context.execution_fence().unwrap().clone();
        if let Some(entered) = self.entered.lock().unwrap().take() {
            let _ = entered.send(fence.clone());
        }
        let release = self.release.lock().unwrap().take();
        if let Some(release) = release {
            let _ = release.await;
        }
        fence.ensure_valid().await?;
        self.external_calls.fetch_add(1, Ordering::SeqCst);
        Ok(CapabilityResult::new(json!({ "ok": true })))
    }

    async fn reconcile(
        &self,
        context: CapabilityExecutionContext,
    ) -> Result<ReconcileOutcome, CapabilityError> {
        ensure_effect_fence(&context).await?;
        if self.reconcile_calls.fetch_add(1, Ordering::SeqCst) == 0 {
            Ok(ReconcileOutcome::Pending)
        } else {
            Ok(ReconcileOutcome::AuthoritativeAbsence)
        }
    }
}

#[async_trait]
impl CapabilityExecutor for RetryBarrierExecutor {
    fn manifest(&self) -> &CapabilityManifest {
        &self.manifest
    }

    async fn execute(
        &self,
        context: CapabilityExecutionContext,
    ) -> Result<CapabilityResult, CapabilityError> {
        ensure_effect_fence(&context).await?;
        if self.executions.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(CapabilityError::execution());
        }
        if let Some(entered) = self.retry_entered.lock().unwrap().take() {
            let _ = entered.send(());
        }
        let release = self.retry_release.lock().unwrap().take();
        if let Some(release) = release {
            let _ = release.await;
        }
        Ok(CapabilityResult::new(json!({ "ok": true })))
    }

    async fn reconcile(
        &self,
        context: CapabilityExecutionContext,
    ) -> Result<ReconcileOutcome, CapabilityError> {
        ensure_effect_fence(&context).await?;
        self.reconciliations.fetch_add(1, Ordering::SeqCst);
        Ok(ReconcileOutcome::AuthoritativeAbsence)
    }
}

#[async_trait]
impl CapabilityExecutor for ConflictingReconcileExecutor {
    fn manifest(&self) -> &CapabilityManifest {
        &self.manifest
    }

    async fn execute(
        &self,
        context: CapabilityExecutionContext,
    ) -> Result<CapabilityResult, CapabilityError> {
        ensure_effect_fence(&context).await?;
        Err(CapabilityError::execution())
    }

    async fn reconcile(
        &self,
        context: CapabilityExecutionContext,
    ) -> Result<ReconcileOutcome, CapabilityError> {
        ensure_effect_fence(&context).await?;
        let call = self.reconciliations.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            if let Some(entered) = self.reconcile_entered.lock().unwrap().take() {
                let _ = entered.send(());
            }
            let release = self.reconcile_release.lock().unwrap().take();
            if let Some(release) = release {
                let _ = release.await;
            }
            Ok(ReconcileOutcome::Completed(CapabilityResult::new(
                json!({ "ok": true }),
            )))
        } else {
            Ok(ReconcileOutcome::RecoveryRequired)
        }
    }
}

#[async_trait]
impl CapabilityExecutor for SecretLeakingExecutor {
    fn manifest(&self) -> &CapabilityManifest {
        &self.manifest
    }

    async fn execute(
        &self,
        context: CapabilityExecutionContext,
    ) -> Result<CapabilityResult, CapabilityError> {
        ensure_effect_fence(&context).await?;
        assert!(!self.upstream_diagnostic.is_empty());
        Err(CapabilityError::execution())
    }

    async fn reconcile(
        &self,
        context: CapabilityExecutionContext,
    ) -> Result<ReconcileOutcome, CapabilityError> {
        ensure_effect_fence(&context).await?;
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
async fn output_preflight_bounds_execution_and_reconciliation_without_panics() {
    let oversized = Value::String("x".repeat(anima_core::MAX_CAPABILITY_ARGUMENT_BYTES + 1));
    let mut execution_manifest = manifest("workspace.apply", 1, RecoveryMode::KeyedIdempotent);
    execution_manifest.output_schema = json!({});
    let mut execution_executor = RecordingExecutor::new(execution_manifest.clone());
    execution_executor.execute_result = CapabilityResult::new(oversized.clone());
    let mut execution_registry = registry(vec![execution_manifest.clone()]);
    execution_registry
        .register_executor(Arc::new(execution_executor))
        .unwrap();
    assert_eq!(
        execution_registry
            .execute(context(&execution_manifest, json!({ "query": "hello" })))
            .await
            .unwrap_err()
            .code(),
        CapabilityErrorCode::OutputValidation
    );

    let mut reconcile_manifest = manifest("workspace.reconcile", 1, RecoveryMode::Reconcilable);
    reconcile_manifest.output_schema = json!({});
    let mut reconcile_executor = RecordingExecutor::failing_once(reconcile_manifest.clone());
    reconcile_executor.reconcile_result =
        ReconcileOutcome::Completed(CapabilityResult::new(oversized));
    let mut reconcile_registry = registry(vec![reconcile_manifest.clone()]);
    reconcile_registry
        .register_executor(Arc::new(reconcile_executor))
        .unwrap();
    let initial = context(&reconcile_manifest, json!({ "query": "hello" }));
    reconcile_registry
        .execute(initial.clone())
        .await
        .unwrap_err();
    assert_eq!(
        reconcile_registry
            .recover(initial)
            .await
            .unwrap_err()
            .code(),
        CapabilityErrorCode::OutputValidation
    );
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
            true,
            RecoveryActionKind::RetrySameKey,
        ),
        (
            RecoveryMode::KeyedIdempotent,
            true,
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
        let mut executor = RecordingExecutor::failing_once(manifest.clone());
        if mode == RecoveryMode::Reconcilable {
            executor.reconcile_result = ReconcileOutcome::Pending;
        }
        let reconciliations = executor.reconciliations.clone();
        let mut registry = registry(vec![manifest.clone()]);
        registry.register_executor(Arc::new(executor)).unwrap();

        let initial = context(&manifest, json!({ "query": "hello" }));
        registry.execute(initial.clone()).await.unwrap_err();
        let action = registry.recover(initial).await.unwrap();
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
        let mut executor = RecordingExecutor::failing_once(manifest.clone());
        executor.reconcile_result = outcome;
        let mut registry = registry(vec![manifest.clone()]);
        registry.register_executor(Arc::new(executor)).unwrap();
        let initial = context(&manifest, json!({ "query": "hello" }));
        registry.execute(initial.clone()).await.unwrap_err();
        let action = registry.recover(initial).await.unwrap();
        assert_eq!(action.kind(), expected);
    }
}

#[tokio::test]
async fn compensate_recovery_never_authorizes_a_retry_for_any_reconcile_outcome() {
    let manifest = manifest("workspace.compensate", 1, RecoveryMode::Compensate);
    for (outcome, expected) in [
        (
            ReconcileOutcome::Completed(CapabilityResult::new(json!({ "ok": true }))),
            RecoveryActionKind::Completed,
        ),
        (ReconcileOutcome::Pending, RecoveryActionKind::Pending),
        (
            ReconcileOutcome::AuthoritativeAbsence,
            RecoveryActionKind::CompensationRequired,
        ),
        (
            ReconcileOutcome::RecoveryRequired,
            RecoveryActionKind::CompensationRequired,
        ),
    ] {
        let store = Arc::new(TestLineageStore::default());
        let mut catalog = ManifestCatalog::default();
        catalog.register_manifest(manifest.clone()).unwrap();
        let mut registry = registry_with_test_stores(catalog, store.clone());
        let mut executor = RecordingExecutor::failing_once(manifest.clone());
        executor.reconcile_result = outcome;
        registry.register_executor(Arc::new(executor)).unwrap();

        let initial = context(&manifest, json!({ "query": "hello" }));
        registry.execute(initial.clone()).await.unwrap_err();
        let action = registry.recover(initial.clone()).await.unwrap();

        assert_eq!(action.kind(), expected);
        assert!(action.retry_authorization().is_none());
        assert!(store
            .load(initial.invocation().id(), initial.attempt().number() + 1)
            .await
            .unwrap()
            .is_none());
    }
}

#[tokio::test]
async fn compensate_recovery_reloads_after_losing_the_terminal_upgrade_cas() {
    let manifest = manifest("workspace.compensate", 1, RecoveryMode::Compensate);
    let context = context(&manifest, json!({ "query": "hello" }));
    let store = Arc::new(TestLineageStore::default());
    store.states.lock().unwrap().insert(
        (context.invocation().id(), context.attempt().number()),
        CapabilityAttemptLineageState::RecoveryRequired,
    );
    store
        .fail_compensation_upgrade_once
        .store(1, Ordering::SeqCst);
    let mut catalog = ManifestCatalog::default();
    catalog.register_manifest(manifest).unwrap();
    let registry = registry_with_test_stores(catalog, store);

    assert_eq!(
        registry.recover(context).await.unwrap().kind(),
        RecoveryActionKind::CompensationRequired
    );
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
        .with_owner(CapabilityReferenceId::new(Uuid::from_u128(1)))
        .with_agent(CapabilityReferenceId::new(Uuid::from_u128(2)))
        .with_session(CapabilityReferenceId::new(Uuid::from_u128(3)))
        .with_workspace(CapabilityReferenceId::new(Uuid::from_u128(4)))
        .with_deadline(CapabilityReferenceId::new(Uuid::from_u128(5)))
        .with_cancellation(CapabilityReferenceId::new(Uuid::from_u128(6)))
        .with_secrets(vec![CapabilitySecretReferenceId::from_manifest_index(0)]);
    let context = baseline.with_references(references).unwrap();

    let serialized = serde_json::to_string(&context).unwrap();
    let debug = format!("{context:?}");
    assert!(serialized.contains("\"secrets\":[0]"));
    assert!(!serialized.contains("github-token"));
    assert!(!debug.contains("github-token"));
    for representation in [&serialized, &debug] {
        assert!(!representation.contains("super-secret-access-token"));
        assert!(!representation.contains("credentials"));
    }
    assert_eq!(
        context.references().deadline().unwrap().handle(),
        Uuid::from_u128(5)
    );
    assert_eq!(
        context.references().cancellation().unwrap().handle(),
        Uuid::from_u128(6)
    );
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
        .register_executor(Arc::new(RecordingExecutor::failing_once(manifest.clone())))
        .unwrap();
    let initial = context(&manifest, json!({ "query": "hello" }));
    registry.execute(initial.clone()).await.unwrap_err();

    let action = registry.recover(initial).await.unwrap();
    let authorization = action.retry_authorization().unwrap().clone();
    let retry = context_for_attempt(&manifest, json!({ "query": "hello" }), 2);
    let binding = authorization.resume_binding();
    assert_eq!(binding.logical_invocation_id(), retry.invocation().id());
    assert_eq!(binding.completed_attempt_number(), 1);
    assert_eq!(binding.retry_attempt_number(), 2);
    assert_eq!(binding.manifest_id(), manifest.id);
    assert_eq!(binding.manifest_version(), manifest.version);
    assert_eq!(binding.manifest_digest(), manifest.schema_digest);
    assert_eq!(binding.recovery_mode(), RecoveryMode::KeyedIdempotent);
    assert_eq!(
        binding.idempotency_key(),
        retry.invocation().idempotency_key()
    );
    let live_claim = registry
        .validate_recovery_resume(&retry, &authorization, &binding)
        .await
        .unwrap();
    let manifest_pin = ManifestPin::new_with_recovery_mode(
        &manifest.id,
        manifest.version,
        &manifest.schema_digest,
        manifest.recovery_mode,
    )
    .unwrap();
    let recovery_pause = RecoveryPauseRecord::new(
        retry.invocation().binding(),
        1,
        manifest_pin.clone(),
        RecoveryPauseReason::AuthoritativeAbsence,
    )
    .unwrap();
    let paused = Run::queued(binding.run_id(), Uuid::from_u128(500), "writer", 1)
        .unwrap()
        .transition(RunState::Running, None)
        .unwrap()
        .pause_for_recovery(recovery_pause)
        .unwrap();
    let resume = RuntimeCommand::resume_with_recovery_binding(
        Uuid::from_u128(501),
        Uuid::from_u128(500),
        binding.run_id(),
        binding.clone(),
    )
    .unwrap();
    assert!(paused.apply_resume_command(&resume, None, None).is_err());
    assert!(paused
        .apply_resume_command(&resume, None, Some(&live_claim))
        .is_ok());
    let uncertain_attempt = InvocationAttemptRecord::new(
        retry.invocation().binding(),
        1,
        AttemptRecordState::Uncertain,
        manifest_pin.clone(),
        manifest.recovery_mode,
    )
    .unwrap();
    let uncertain = UncertainInvocationRecord::new(
        retry.invocation().binding(),
        1,
        manifest_pin.clone(),
        manifest.recovery_mode,
        Some(binding.clone()),
    )
    .unwrap();
    let checkpoint = CheckpointV1Builder::new(
        Uuid::from_u128(500),
        binding.run_id(),
        DefinitionPin::new(1, "writer", 1).unwrap(),
        1,
        vec![manifest_pin],
        Budget::default(),
        Usage::default(),
    )
    .state(RunState::Paused, Some(RunPauseReason::RecoveryRequired))
    .cursor_step_id(Some("step-7".into()))
    .attempts(vec![uncertain_attempt])
    .uncertain_invocations(vec![uncertain])
    .build()
    .unwrap();
    assert!(
        serde_json::from_value::<CheckpointV1>(serde_json::to_value(checkpoint).unwrap()).is_ok()
    );
    let mut forged = serde_json::to_value(&binding).unwrap();
    forged["completed_attempt_number"] = json!(2);
    assert!(serde_json::from_value::<anima_core::RecoveryResumeBinding>(forged).is_err());

    registry
        .execute_retry(retry.clone(), authorization.clone())
        .await
        .unwrap();
    assert!(registry
        .validate_recovery_resume(&retry, &authorization, &binding)
        .await
        .is_err());
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
        RecoveryActionKind::Completed
    );
}

#[tokio::test]
async fn retry_budget_blocks_authorization_after_an_uncertain_final_attempt() {
    let mut manifest = manifest("workspace.apply", 1, RecoveryMode::KeyedIdempotent);
    manifest.max_retries = 1;
    let mut executor = RecordingExecutor::new(manifest.clone());
    executor.failures_remaining.store(2, Ordering::SeqCst);
    executor.reconcile_result = ReconcileOutcome::AuthoritativeAbsence;
    let mut registry = registry(vec![manifest.clone()]);
    registry.register_executor(Arc::new(executor)).unwrap();
    let initial = context(&manifest, json!({ "query": "hello" }));
    registry.execute(initial.clone()).await.unwrap_err();
    let authorization = registry
        .recover(initial)
        .await
        .unwrap()
        .retry_authorization()
        .unwrap()
        .clone();
    let retry = context_for_attempt(&manifest, json!({ "query": "hello" }), 2);
    registry
        .execute_retry(retry.clone(), authorization)
        .await
        .unwrap_err();

    assert_eq!(
        registry.recover(retry).await.unwrap().kind(),
        RecoveryActionKind::RecoveryRequired
    );
}

#[tokio::test]
async fn recovery_requires_a_known_executed_attempt() {
    let manifest = manifest("workspace.apply", 1, RecoveryMode::KeyedIdempotent);
    let mut registry = registry(vec![manifest.clone()]);
    registry
        .register_executor(Arc::new(RecordingExecutor::new(manifest.clone())))
        .unwrap();

    let unknown = context(&manifest, json!({ "query": "hello" }));
    assert_eq!(
        registry.recover(unknown).await.unwrap_err().code(),
        CapabilityErrorCode::Validation
    );

    let initial = context(&manifest, json!({ "query": "hello" }));
    registry.execute(initial.clone()).await.unwrap();
    let fabricated = context_for_attempt(&manifest, json!({ "query": "hello" }), 3);
    assert_eq!(
        registry.recover(fabricated).await.unwrap_err().code(),
        CapabilityErrorCode::Validation
    );
}

#[tokio::test]
async fn recovery_racing_active_execution_returns_pending_without_reconciliation() {
    let manifest = manifest("workspace.apply", 1, RecoveryMode::Reconcilable);
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let reconciliations = Arc::new(AtomicUsize::new(0));
    let executor = BarrierExecutor {
        manifest: manifest.clone(),
        execute_entered: Mutex::new(Some(entered_tx)),
        execute_release: Mutex::new(Some(release_rx)),
        reconcile_result: ReconcileOutcome::RecoveryRequired,
        reconciliations: reconciliations.clone(),
    };
    let mut mutable_registry = registry(vec![manifest.clone()]);
    mutable_registry
        .register_executor(Arc::new(executor))
        .unwrap();
    let registry = Arc::new(mutable_registry);
    let initial = context(&manifest, json!({ "query": "hello" }));
    let execution_registry = registry.clone();
    let execution_context = initial.clone();
    let execution =
        tokio::spawn(async move { execution_registry.execute(execution_context).await });
    entered_rx.await.unwrap();

    assert_eq!(
        registry.recover(initial.clone()).await.unwrap().kind(),
        RecoveryActionKind::Pending
    );
    assert_eq!(reconciliations.load(Ordering::SeqCst), 0);
    release_tx.send(()).unwrap();
    execution.await.unwrap().unwrap();
    assert_eq!(
        registry.recover(initial).await.unwrap().kind(),
        RecoveryActionKind::Completed
    );
}

#[tokio::test]
async fn recovery_racing_active_retry_returns_pending_without_reconciliation() {
    let manifest = manifest("workspace.apply", 1, RecoveryMode::KeyedIdempotent);
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let reconciliations = Arc::new(AtomicUsize::new(0));
    let executor = RetryBarrierExecutor {
        manifest: manifest.clone(),
        executions: AtomicUsize::new(0),
        retry_entered: Mutex::new(Some(entered_tx)),
        retry_release: Mutex::new(Some(release_rx)),
        reconciliations: reconciliations.clone(),
    };
    let mut mutable_registry = registry(vec![manifest.clone()]);
    mutable_registry
        .register_executor(Arc::new(executor))
        .unwrap();
    let registry = Arc::new(mutable_registry);
    let initial = context(&manifest, json!({ "query": "hello" }));
    registry.execute(initial.clone()).await.unwrap_err();
    let authorization = registry
        .recover(initial)
        .await
        .unwrap()
        .retry_authorization()
        .unwrap()
        .clone();
    let retry = context_for_attempt(&manifest, json!({ "query": "hello" }), 2);
    let retry_registry = registry.clone();
    let retry_context = retry.clone();
    let execution = tokio::spawn(async move {
        retry_registry
            .execute_retry(retry_context, authorization)
            .await
    });
    entered_rx.await.unwrap();

    assert_eq!(
        registry.recover(retry.clone()).await.unwrap().kind(),
        RecoveryActionKind::Pending
    );
    assert_eq!(reconciliations.load(Ordering::SeqCst), 1);
    release_tx.send(()).unwrap();
    execution.await.unwrap().unwrap();
    assert_eq!(
        registry.recover(retry).await.unwrap().kind(),
        RecoveryActionKind::Completed
    );
}

#[tokio::test]
async fn store_authoritative_heartbeat_keeps_long_running_execution_fenced() {
    let mut manifest = manifest("workspace.apply", 1, RecoveryMode::Reconcilable);
    manifest.timeout_ms = 40_000;
    let store = Arc::new(TestLineageStore::default());
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let external_calls = Arc::new(AtomicUsize::new(0));
    let executor = FenceAwareBarrierExecutor {
        manifest: manifest.clone(),
        entered: Mutex::new(Some(entered_tx)),
        release: Mutex::new(Some(release_rx)),
        external_calls: external_calls.clone(),
        reconcile_calls: AtomicUsize::new(0),
    };
    let mut catalog = ManifestCatalog::default();
    catalog.register_manifest(manifest.clone()).unwrap();
    let mut mutable_registry = registry_with_test_stores(catalog, store.clone());
    mutable_registry
        .register_executor(Arc::new(executor))
        .unwrap();
    let registry = Arc::new(mutable_registry);
    let initial = context(&manifest, json!({ "query": "hello" }));
    let execution_registry = registry.clone();
    let execution_context = initial.clone();
    let execution =
        tokio::spawn(async move { execution_registry.execute(execution_context).await });
    let fence = entered_rx.await.unwrap();

    for _ in 0..150 {
        if store.renewals.load(Ordering::SeqCst) > 0 {
            break;
        }
        Delay::new(std::time::Duration::from_millis(10)).await;
    }
    assert!(store.renewals.load(Ordering::SeqCst) > 0);
    store.authoritative_now_ms.store(31_000, Ordering::SeqCst);
    assert!(fence.is_valid());
    assert_eq!(
        registry.recover(initial).await.unwrap().kind(),
        RecoveryActionKind::Pending
    );
    release_tx.send(()).unwrap();
    execution.await.unwrap().unwrap();
    assert_eq!(external_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn lost_execution_fence_cancels_original_and_requires_strong_absence_for_retry() {
    let mut manifest = manifest("workspace.apply", 1, RecoveryMode::Reconcilable);
    manifest.timeout_ms = 30;
    let store = Arc::new(TestLineageStore::default());
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let external_calls = Arc::new(AtomicUsize::new(0));
    let executor = FenceAwareBarrierExecutor {
        manifest: manifest.clone(),
        entered: Mutex::new(Some(entered_tx)),
        release: Mutex::new(Some(release_rx)),
        external_calls: external_calls.clone(),
        reconcile_calls: AtomicUsize::new(0),
    };
    let mut catalog = ManifestCatalog::default();
    catalog.register_manifest(manifest.clone()).unwrap();
    let mut mutable_registry = registry_with_test_stores(catalog, store.clone());
    mutable_registry
        .register_executor(Arc::new(executor))
        .unwrap();
    let registry = Arc::new(mutable_registry);
    let initial = context(&manifest, json!({ "query": "hello" }));
    let execution_registry = registry.clone();
    let execution_context = initial.clone();
    let execution =
        tokio::spawn(async move { execution_registry.execute(execution_context).await });
    let fence = entered_rx.await.unwrap();
    store.states.lock().unwrap().insert(
        (initial.invocation().id(), initial.attempt().number()),
        CapabilityAttemptLineageState::Uncertain,
    );

    for _ in 0..30 {
        if fence.is_cancelled() {
            break;
        }
        Delay::new(std::time::Duration::from_millis(10)).await;
    }
    assert!(fence.is_cancelled());
    let pending = registry.recover(initial.clone()).await.unwrap();
    assert_eq!(pending.kind(), RecoveryActionKind::Pending);
    assert!(pending.retry_authorization().is_none());
    release_tx.send(()).unwrap();
    assert_eq!(
        execution.await.unwrap().unwrap_err().code(),
        CapabilityErrorCode::Cancelled
    );
    assert_eq!(external_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        registry.recover(initial).await.unwrap().kind(),
        RecoveryActionKind::RetrySameKey
    );
}

#[tokio::test]
async fn authoritative_effect_check_rejects_takeover_before_heartbeat_observes_loss() {
    let mut manifest = manifest("workspace.apply", 1, RecoveryMode::Reconcilable);
    manifest.timeout_ms = 40_000;
    let store = Arc::new(TestLineageStore::default());
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let external_calls = Arc::new(AtomicUsize::new(0));
    let executor = FenceAwareBarrierExecutor {
        manifest: manifest.clone(),
        entered: Mutex::new(Some(entered_tx)),
        release: Mutex::new(Some(release_rx)),
        external_calls: external_calls.clone(),
        reconcile_calls: AtomicUsize::new(0),
    };
    let mut catalog = ManifestCatalog::default();
    catalog.register_manifest(manifest.clone()).unwrap();
    let mut mutable_registry = registry_with_test_stores(catalog, store.clone());
    mutable_registry
        .register_executor(Arc::new(executor))
        .unwrap();
    let registry = Arc::new(mutable_registry);
    let initial = context(&manifest, json!({ "query": "hello" }));
    let execution_registry = registry.clone();
    let execution_context = initial.clone();
    let execution =
        tokio::spawn(async move { execution_registry.execute(execution_context).await });
    let fence = entered_rx.await.unwrap();
    assert_eq!(
        fence.idempotency_key(),
        initial.invocation().idempotency_key()
    );
    let destination_fencing_token = fence.fencing_token().destination_value();
    assert!(!destination_fencing_token.is_empty());
    assert!(!format!("{:?}", fence.fencing_token()).contains(&destination_fencing_token));
    let key = (initial.invocation().id(), initial.attempt().number());
    let active = store.load(key.0, key.1).await.unwrap().unwrap();
    let active_fence = match &active {
        CapabilityAttemptLineageState::Executing { fence, .. } => *fence,
        _ => panic!("initial execution must hold an executing lease"),
    };
    assert!(store
        .validate_effect_fence(key.0, key.1, CapabilityLeaseKind::Executing, active_fence,)
        .await
        .unwrap());
    assert!(!store
        .validate_effect_fence(key.0, key.1, CapabilityLeaseKind::Reconciling, active_fence,)
        .await
        .unwrap());
    store.authoritative_now_ms.store(50_000, Ordering::SeqCst);
    assert!(store
        .expire_lease(
            key.0,
            key.1,
            active,
            CapabilityAttemptLineageState::Uncertain,
        )
        .await
        .unwrap());
    assert!(store
        .acquire_lease(
            key.0,
            key.1,
            Some(CapabilityAttemptLineageState::Uncertain),
            CapabilityLeaseKind::Reconciling,
            40_000,
        )
        .await
        .unwrap()
        .is_some());
    assert_eq!(store.renewals.load(Ordering::SeqCst), 0);
    assert!(fence.is_valid());

    release_tx.send(()).unwrap();

    assert_eq!(
        execution.await.unwrap().unwrap_err().code(),
        CapabilityErrorCode::Cancelled
    );
    assert_eq!(external_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn concurrent_reconciliation_is_single_winner_and_terminal_monotonic() {
    let manifest = manifest("workspace.apply", 1, RecoveryMode::Reconcilable);
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let reconciliations = Arc::new(AtomicUsize::new(0));
    let executor = ConflictingReconcileExecutor {
        manifest: manifest.clone(),
        reconcile_entered: Mutex::new(Some(entered_tx)),
        reconcile_release: Mutex::new(Some(release_rx)),
        reconciliations: reconciliations.clone(),
    };
    let mut mutable_registry = registry(vec![manifest.clone()]);
    mutable_registry
        .register_executor(Arc::new(executor))
        .unwrap();
    let registry = Arc::new(mutable_registry);
    let initial = context(&manifest, json!({ "query": "hello" }));
    assert_eq!(
        registry.execute(initial.clone()).await.unwrap_err().code(),
        CapabilityErrorCode::Execution
    );
    let recovery_registry = registry.clone();
    let recovery_context = initial.clone();
    let first = tokio::spawn(async move { recovery_registry.recover(recovery_context).await });
    entered_rx.await.unwrap();

    assert_eq!(
        registry.recover(initial.clone()).await.unwrap().kind(),
        RecoveryActionKind::Pending
    );
    assert_eq!(reconciliations.load(Ordering::SeqCst), 1);
    release_tx.send(()).unwrap();
    assert_eq!(
        first.await.unwrap().unwrap().kind(),
        RecoveryActionKind::Completed
    );
    assert_eq!(
        registry.recover(initial).await.unwrap().kind(),
        RecoveryActionKind::Completed
    );
    assert_eq!(reconciliations.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn concurrent_recovery_returns_one_redacted_authorization() {
    let manifest = manifest("workspace.apply", 1, RecoveryMode::KeyedIdempotent);
    let mut mutable_registry = registry(vec![manifest.clone()]);
    mutable_registry
        .register_executor(Arc::new(RecordingExecutor::failing_once(manifest.clone())))
        .unwrap();
    let registry = Arc::new(mutable_registry);
    let initial = context(&manifest, json!({ "query": "hello" }));
    registry.execute(initial.clone()).await.unwrap_err();

    let (first, second) =
        futures::join!(registry.recover(initial.clone()), registry.recover(initial));
    let first = first.unwrap();
    let second = second.unwrap();
    assert_eq!(first, second);
    assert_eq!(
        format!("{:?}", first.retry_authorization().unwrap()),
        "CapabilityRetryAuthorization(REDACTED)"
    );
}

#[tokio::test]
async fn duplicate_authoritative_absence_recovery_reuses_the_same_token() {
    let manifest = manifest("workspace.apply", 1, RecoveryMode::Reconcilable);
    let mut executor = RecordingExecutor::failing_once(manifest.clone());
    executor.reconcile_result = ReconcileOutcome::AuthoritativeAbsence;
    let reconciliations = executor.reconciliations.clone();
    let mut registry = registry(vec![manifest.clone()]);
    registry.register_executor(Arc::new(executor)).unwrap();
    let initial = context(&manifest, json!({ "query": "hello" }));
    registry.execute(initial.clone()).await.unwrap_err();

    let first = registry.recover(initial.clone()).await.unwrap();
    let second = registry.recover(initial).await.unwrap();
    assert_eq!(first, second);
    assert_eq!(reconciliations.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn durable_lineage_store_preserves_retry_authorization_across_registries() {
    let manifest = manifest("workspace.apply", 1, RecoveryMode::KeyedIdempotent);
    let store = Arc::new(TestLineageStore::default());
    let mut first_catalog = ManifestCatalog::default();
    first_catalog.register_manifest(manifest.clone()).unwrap();
    let mut first_registry = registry_with_test_stores(first_catalog, store.clone());
    first_registry
        .register_executor(Arc::new(RecordingExecutor::failing_once(manifest.clone())))
        .unwrap();
    let initial = context(&manifest, json!({ "query": "hello" }));
    first_registry.execute(initial.clone()).await.unwrap_err();
    let first_action = first_registry.recover(initial.clone()).await.unwrap();

    let mut second_catalog = ManifestCatalog::default();
    second_catalog.register_manifest(manifest.clone()).unwrap();
    let mut second_registry = registry_with_test_stores(second_catalog, store);
    second_registry
        .register_executor(Arc::new(RecordingExecutor::new(manifest.clone())))
        .unwrap();
    let restored_action = second_registry.recover(initial).await.unwrap();
    assert_eq!(first_action, restored_action);
    second_registry
        .execute_retry(
            context_for_attempt(&manifest, json!({ "query": "hello" }), 2),
            restored_action.retry_authorization().unwrap().clone(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn durable_lineage_store_returns_cached_completion_after_registry_restart() {
    let manifest = manifest("workspace.apply", 1, RecoveryMode::Reconcilable);
    let store = Arc::new(TestLineageStore::default());
    let results = Arc::new(StableRepeatedResultRecorder::default());
    let mut first_catalog = ManifestCatalog::default();
    first_catalog.register_manifest(manifest.clone()).unwrap();
    let mut first_registry =
        CapabilityRegistry::with_stores(first_catalog, store.clone(), results.clone());
    first_registry
        .register_executor(Arc::new(RecordingExecutor::new(manifest.clone())))
        .unwrap();
    let initial = context(&manifest, json!({ "query": "hello" }));
    first_registry.execute(initial.clone()).await.unwrap();
    let completed = match store
        .load(initial.invocation().id(), initial.attempt().number())
        .await
        .unwrap()
        .unwrap()
    {
        CapabilityAttemptLineageState::Completed(completed) => completed,
        _ => panic!("completion must be durable metadata"),
    };

    let mut second_catalog = ManifestCatalog::default();
    second_catalog.register_manifest(manifest.clone()).unwrap();
    let second_executor = RecordingExecutor::new(manifest.clone());
    let reconciliations = second_executor.reconciliations.clone();
    let mut second_registry =
        CapabilityRegistry::with_stores(second_catalog, store, results.clone());
    second_registry
        .register_executor(Arc::new(second_executor))
        .unwrap();

    assert_eq!(
        second_registry.recover(initial.clone()).await.unwrap(),
        anima_core::RecoveryAction::Completed(completed.clone())
    );
    assert_eq!(reconciliations.load(Ordering::SeqCst), 0);
    assert_eq!(
        results.resolve(completed.result_ref()),
        Some(CapabilityResult::new(json!({ "ok": true })))
    );
}

#[tokio::test]
async fn expired_execution_lease_can_be_fenced_and_reconciled() {
    let manifest = manifest("workspace.apply", 1, RecoveryMode::Reconcilable);
    let store = Arc::new(TestLineageStore::default());
    let initial = context(&manifest, json!({ "query": "hello" }));
    store.states.lock().unwrap().insert(
        (initial.invocation().id(), initial.attempt().number()),
        CapabilityAttemptLineageState::Executing {
            fence: Uuid::new_v4(),
            lease_expires_at_ms: 0,
        },
    );
    let mut catalog = ManifestCatalog::default();
    catalog.register_manifest(manifest.clone()).unwrap();
    let mut executor = RecordingExecutor::new(manifest.clone());
    executor.reconcile_result =
        ReconcileOutcome::Completed(CapabilityResult::new(json!({ "ok": true })));
    let reconciliations = executor.reconciliations.clone();
    let mut registry = registry_with_test_stores(catalog, store);
    registry.register_executor(Arc::new(executor)).unwrap();

    assert_eq!(
        registry.recover(initial).await.unwrap().kind(),
        RecoveryActionKind::Completed
    );
    assert_eq!(reconciliations.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn concurrent_retry_consumption_has_exactly_one_winner() {
    let manifest = manifest("workspace.apply", 1, RecoveryMode::KeyedIdempotent);
    let executor = RecordingExecutor::failing_once(manifest.clone());
    let executions = executor.executions.clone();
    let mut mutable_registry = registry(vec![manifest.clone()]);
    mutable_registry
        .register_executor(Arc::new(executor))
        .unwrap();
    let registry = Arc::new(mutable_registry);
    let initial = context(&manifest, json!({ "query": "hello" }));
    registry.execute(initial.clone()).await.unwrap_err();
    let authorization = registry
        .recover(initial)
        .await
        .unwrap()
        .retry_authorization()
        .unwrap()
        .clone();
    let retry = context_for_attempt(&manifest, json!({ "query": "hello" }), 2);

    let (first, second) = futures::join!(
        registry.execute_retry(retry.clone(), authorization.clone()),
        registry.execute_retry(retry, authorization)
    );
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    assert_eq!(executions.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn recovery_authorizations_cannot_be_used_for_another_invocation() {
    let manifest = manifest("workspace.apply", 1, RecoveryMode::KeyedIdempotent);
    let mut registry = registry(vec![manifest.clone()]);
    registry
        .register_executor(Arc::new(RecordingExecutor::failing_once(manifest.clone())))
        .unwrap();
    let initial = context(&manifest, json!({ "query": "one" }));
    registry.execute(initial.clone()).await.unwrap_err();
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
    assert!(
        serde_json::from_value::<CapabilityReferenceId>(json!("https://token.example/path"))
            .is_err()
    );
    assert!(serde_json::from_value::<CapabilitySecretReferenceId>(json!(
        "super-secret-upstream-body"
    ))
    .is_err());
    assert!(!format!("{context:?}").contains("super-secret-upstream-body"));
}

#[tokio::test]
async fn undeclared_secret_handle_is_rejected_before_executor_entry() {
    let manifest = manifest("workspace.apply", 1, RecoveryMode::KeyedIdempotent);
    let executor = RecordingExecutor::new(manifest.clone());
    let calls = executor.executions.clone();
    let mut registry = registry(vec![manifest.clone()]);
    registry.register_executor(Arc::new(executor)).unwrap();
    let baseline = context(&manifest, json!({ "query": "hello" }));
    let references = baseline
        .references()
        .clone()
        .with_secrets(vec![CapabilitySecretReferenceId::from_manifest_index(1)]);
    let context = baseline.with_references(references).unwrap();

    assert_eq!(
        registry.execute(context).await.unwrap_err().code(),
        CapabilityErrorCode::Validation
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn references_cannot_be_grafted_across_runs() {
    let manifest = manifest("workspace.apply", 1, RecoveryMode::KeyedIdempotent);
    let first = context(&manifest, json!({ "query": "hello" }));
    let second_invocation = LogicalInvocation::new(
        Uuid::from_u128(99),
        "step-7",
        manifest.id.clone(),
        manifest.version,
        json!({ "query": "hello" }),
    )
    .unwrap();
    let second = CapabilityExecutionContext::for_attempt(
        second_invocation.clone(),
        CapabilityAttempt::new(&second_invocation, 1).unwrap(),
    )
    .unwrap();

    assert_eq!(
        second
            .with_references(first.references().clone())
            .unwrap_err(),
        CapabilityContextError::RunReferenceMismatch
    );
}

#[derive(Clone)]
struct ExactScopeValidator {
    invocation_id: Uuid,
    references: CapabilityExecutionReferences,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl CapabilityReferenceValidator for ExactScopeValidator {
    async fn validate(
        &self,
        context: &CapabilityExecutionContext,
        _manifest: &CapabilityManifest,
    ) -> Result<(), CapabilityError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if context.invocation().id() == self.invocation_id
            && context.references() == &self.references
        {
            Ok(())
        } else {
            Err(CapabilityError::validation())
        }
    }
}

#[tokio::test]
async fn expanded_reference_scopes_require_host_attestation_and_reject_same_run_grafts() {
    let manifest = manifest("workspace.apply", 1, RecoveryMode::KeyedIdempotent);
    let executor = RecordingExecutor::new(manifest.clone());
    let executions = executor.executions.clone();
    let base = context(&manifest, json!({ "query": "hello" }));
    let expected = base
        .references()
        .clone()
        .with_owner(CapabilityReferenceId::new(Uuid::from_u128(10)))
        .with_agent(CapabilityReferenceId::new(Uuid::from_u128(11)))
        .with_session(CapabilityReferenceId::new(Uuid::from_u128(12)))
        .with_workspace(CapabilityReferenceId::new(Uuid::from_u128(13)));
    let scoped = base.clone().with_references(expected.clone()).unwrap();
    let expected = scoped.references().clone();

    let mut default_registry = registry(vec![manifest.clone()]);
    default_registry
        .register_executor(Arc::new(executor.clone()))
        .unwrap();
    assert_eq!(
        default_registry
            .execute(scoped.clone())
            .await
            .unwrap_err()
            .code(),
        CapabilityErrorCode::Validation
    );
    assert_eq!(executions.load(Ordering::SeqCst), 0);

    let calls = Arc::new(AtomicUsize::new(0));
    let validator = ExactScopeValidator {
        invocation_id: base.invocation().id(),
        references: expected.clone(),
        calls: calls.clone(),
    };
    let mut attested_registry =
        registry(vec![manifest.clone()]).with_reference_validator(Arc::new(validator));
    attested_registry
        .register_executor(Arc::new(executor))
        .unwrap();
    attested_registry.execute(scoped).await.unwrap();

    for grafted in [
        expected
            .clone()
            .with_owner(CapabilityReferenceId::new(Uuid::from_u128(20))),
        expected
            .clone()
            .with_agent(CapabilityReferenceId::new(Uuid::from_u128(21))),
        expected
            .clone()
            .with_session(CapabilityReferenceId::new(Uuid::from_u128(22))),
        expected
            .clone()
            .with_workspace(CapabilityReferenceId::new(Uuid::from_u128(23))),
    ] {
        let grafted = base.clone().with_references(grafted).unwrap();
        assert_eq!(
            attested_registry.execute(grafted).await.unwrap_err().code(),
            CapabilityErrorCode::Validation
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 5);
    assert_eq!(executions.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn durable_lineage_records_only_opaque_result_metadata() {
    let mut manifest = manifest("workspace.apply", 1, RecoveryMode::Reconcilable);
    manifest.output_schema = json!({});
    let secret = "sk-secret-output-must-not-persist";
    let mut executor = RecordingExecutor::new(manifest.clone());
    executor.execute_result = CapabilityResult::new(json!({ "ok": true, "secret": secret }));
    let store = Arc::new(TestLineageStore::default());
    let mut catalog = ManifestCatalog::default();
    catalog.register_manifest(manifest.clone()).unwrap();
    let mut registry = registry_with_test_stores(catalog, store.clone());
    registry.register_executor(Arc::new(executor)).unwrap();
    let context = context(&manifest, json!({ "query": "hello" }));

    let live = registry.execute(context.clone()).await.unwrap();
    assert_eq!(live.output["secret"], json!(secret));
    let state = store
        .load(context.invocation().id(), 1)
        .await
        .unwrap()
        .unwrap();
    let durable = match &state {
        CapabilityAttemptLineageState::Completed(record) => record,
        _ => panic!("execution must store a durable completion record"),
    };
    let _: &DurableCapabilityResult = durable;
    assert!(!serde_json::to_string(&state).unwrap().contains(secret));
    assert!(!format!("{state:?}").contains(secret));
    assert_eq!(
        registry.recover(context).await.unwrap().result_ref(),
        Some(durable.result_ref())
    );
}

#[tokio::test]
async fn result_recording_is_at_least_once_with_stable_attempt_identity() {
    let manifest = manifest("workspace.apply", 1, RecoveryMode::Reconcilable);
    let context = context(&manifest, json!({ "query": "hello" }));
    let recorder = Arc::new(StableRepeatedResultRecorder::default());
    let store = Arc::new(TestLineageStore::default());
    store.fail_completion_cas_once.store(1, Ordering::SeqCst);
    let mut catalog = ManifestCatalog::default();
    catalog.register_manifest(manifest.clone()).unwrap();
    let mut executor = RecordingExecutor::new(manifest.clone());
    executor.reconcile_result = ReconcileOutcome::Completed(CapabilityResult::new(json!({
        "ok": true
    })));
    let mut registry = CapabilityRegistry::with_stores(catalog, store.clone(), recorder.clone());
    registry.register_executor(Arc::new(executor)).unwrap();

    registry.execute(context.clone()).await.unwrap_err();
    assert_eq!(
        registry.recover(context.clone()).await.unwrap().kind(),
        RecoveryActionKind::Completed
    );
    let state = store
        .load(context.invocation().id(), context.attempt().number())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(state, CapabilityAttemptLineageState::Completed(_)));

    assert_eq!(recorder.calls.load(Ordering::SeqCst), 2);
    assert_eq!(recorder.records.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn result_recorder_failure_leaves_the_attempt_uncertain() {
    let manifest = manifest("workspace.apply", 1, RecoveryMode::KeyedIdempotent);
    let store = Arc::new(TestLineageStore::default());
    let mut catalog = ManifestCatalog::default();
    catalog.register_manifest(manifest.clone()).unwrap();
    let mut registry =
        CapabilityRegistry::with_stores(catalog, store.clone(), Arc::new(RejectingResultRecorder));
    registry
        .register_executor(Arc::new(RecordingExecutor::new(manifest.clone())))
        .unwrap();
    let context = context(&manifest, json!({ "query": "hello" }));

    assert_eq!(
        registry.execute(context.clone()).await.unwrap_err().code(),
        CapabilityErrorCode::OutputValidation
    );
    assert_eq!(
        store
            .load(context.invocation().id(), context.attempt().number())
            .await
            .unwrap(),
        Some(CapabilityAttemptLineageState::Uncertain)
    );
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
    let escaped_oversized =
        Value::String("\"".repeat(anima_core::MAX_CAPABILITY_ARGUMENT_BYTES / 2));
    assert!(
        LogicalInvocation::new(Uuid::nil(), "step", "capability", 1, escaped_oversized).is_err()
    );

    let mut deep = json!(null);
    for _ in 0..=anima_core::MAX_CAPABILITY_ARGUMENT_DEPTH {
        deep = json!([deep]);
    }
    assert!(LogicalInvocation::new(Uuid::nil(), "step", "capability", 1, deep).is_err());
    let nodes = Value::Array(vec![Value::Null; anima_core::MAX_CAPABILITY_ARGUMENT_NODES]);
    assert!(LogicalInvocation::new(Uuid::nil(), "step", "capability", 1, nodes).is_err());
    let bounded_nodes = Value::Array(vec![
        Value::Null;
        anima_core::MAX_CAPABILITY_ARGUMENT_NODES - 1
    ]);
    assert!(LogicalInvocation::new(
        Uuid::nil(),
        "s".repeat(anima_core::MAX_CAPABILITY_ID_BYTES),
        "capability",
        1,
        bounded_nodes
    )
    .is_ok());
    assert!(LogicalInvocation::new(
        Uuid::nil(),
        "s".repeat(anima_core::MAX_CAPABILITY_ID_BYTES + 1),
        "capability",
        1,
        json!(null)
    )
    .is_err());
}

#[test]
fn catalog_rejects_invalid_or_duplicate_secret_declaration_names() {
    for names in [
        vec!["UPPERCASE".to_owned()],
        vec!["token/value".to_owned()],
        vec!["duplicate".to_owned(), "duplicate".to_owned()],
    ] {
        let mut candidate = manifest("workspace.apply", 1, RecoveryMode::KeyedIdempotent);
        candidate.secret_references = names;
        let mut catalog = ManifestCatalog::default();
        assert_eq!(
            catalog.register_manifest(candidate).unwrap_err(),
            ManifestCatalogError::InvalidSecretReferenceName
        );
    }
}

#[test]
fn catalog_caps_manifest_ids_and_index_addressable_secret_declarations() {
    let mut oversized_id = manifest("workspace.apply", 1, RecoveryMode::KeyedIdempotent);
    oversized_id.id = "x".repeat(anima_core::MAX_CAPABILITY_ID_BYTES + 1);
    let mut catalog = ManifestCatalog::default();
    assert_eq!(
        catalog.register_manifest(oversized_id.clone()).unwrap_err(),
        ManifestCatalogError::InvalidManifestId
    );
    assert!(serde_json::from_value::<ManifestCatalog>(json!({
        "manifests": [oversized_id],
        "profiles": []
    }))
    .is_err());

    let mut too_many_secrets = manifest("workspace.secrets", 1, RecoveryMode::KeyedIdempotent);
    too_many_secrets.secret_references = (0..=anima_core::MAX_CAPABILITY_SECRET_REFERENCES)
        .map(|index| format!("secret-{index}"))
        .collect();
    assert_eq!(
        catalog.register_manifest(too_many_secrets).unwrap_err(),
        ManifestCatalogError::TooManySecretReferences
    );
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
