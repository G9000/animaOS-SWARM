use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use futures::channel::oneshot;
use futures::future::{select, Either};
use futures::lock::Mutex;
use futures_timer::Delay;
use jsonschema::JSONSchema;
use uuid::Uuid;

use super::lineage::InMemoryCapabilityLineageStore;
use super::*;

/// A host-agnostic pairing of the exact portable manifest catalog and host executors.
pub struct CapabilityRegistry {
    catalog: ManifestCatalog,
    executors: BTreeMap<(String, u32), RegisteredExecutor>,
    lineage: Arc<dyn CapabilityLineageStore>,
    reference_validator: Arc<dyn CapabilityReferenceValidator>,
    result_recorder: Arc<dyn CapabilityResultRecorder>,
}

struct InvocationBoundReferenceValidator;
struct InMemoryCapabilityResultRecorder;

#[async_trait]
impl CapabilityReferenceValidator for InvocationBoundReferenceValidator {
    async fn validate(
        &self,
        context: &CapabilityExecutionContext,
        _manifest: &CapabilityManifest,
    ) -> Result<(), CapabilityError> {
        if context.references().is_bound_to(context.invocation())
            && context.references().is_run_only()
        {
            Ok(())
        } else {
            Err(CapabilityError::validation())
        }
    }
}

#[async_trait]
impl CapabilityResultRecorder for InMemoryCapabilityResultRecorder {
    async fn record(
        &self,
        _context: &CapabilityExecutionContext,
        _manifest: &CapabilityManifest,
        _result: &CapabilityResult,
        _durable: &DurableCapabilityResult,
    ) -> Result<(), CapabilityError> {
        Ok(())
    }
}

struct RegisteredExecutor {
    executor: Arc<dyn CapabilityExecutor>,
    input_validator: Arc<JSONSchema>,
    output_validator: Arc<JSONSchema>,
}

impl CapabilityRegistry {
    pub fn new(catalog: ManifestCatalog) -> Self {
        Self::with_lineage_store(catalog, Arc::new(InMemoryCapabilityLineageStore::default()))
    }

    pub fn with_lineage_store(
        catalog: ManifestCatalog,
        lineage: Arc<dyn CapabilityLineageStore>,
    ) -> Self {
        Self {
            catalog,
            executors: BTreeMap::new(),
            lineage,
            reference_validator: Arc::new(InvocationBoundReferenceValidator),
            result_recorder: Arc::new(InMemoryCapabilityResultRecorder),
        }
    }

    pub fn with_reference_validator(
        mut self,
        validator: Arc<dyn CapabilityReferenceValidator>,
    ) -> Self {
        self.reference_validator = validator;
        self
    }

    pub fn with_result_recorder(mut self, recorder: Arc<dyn CapabilityResultRecorder>) -> Self {
        self.result_recorder = recorder;
        self
    }

    pub fn manifest(&self, id: &str, version: u32) -> Option<&CapabilityManifest> {
        self.catalog.manifest(id, version)
    }

    pub fn executor(&self, id: &str, version: u32) -> Option<Arc<dyn CapabilityExecutor>> {
        self.executors
            .get(&(id.to_owned(), version))
            .map(|entry| entry.executor.clone())
    }

    pub fn register_executor(
        &mut self,
        executor: Arc<dyn CapabilityExecutor>,
    ) -> Result<(), CapabilityRegistryError> {
        let executor_manifest = executor.manifest();
        let key = (executor_manifest.id.clone(), executor_manifest.version);
        let Some(registered_manifest) = self.catalog.manifest(&key.0, key.1) else {
            return Err(CapabilityRegistryError::ManifestExecutorMismatch {
                id: key.0,
                version: key.1,
            });
        };
        if registered_manifest != executor_manifest {
            return Err(CapabilityRegistryError::ManifestExecutorMismatch {
                id: executor_manifest.id.clone(),
                version: executor_manifest.version,
            });
        }
        if self.executors.contains_key(&key) {
            return Err(CapabilityRegistryError::DuplicateExecutor {
                id: key.0,
                version: key.1,
            });
        }
        let input_validator = compile_schema(&registered_manifest.input_schema).map_err(|_| {
            CapabilityRegistryError::InvalidInputSchema {
                id: registered_manifest.id.clone(),
                version: registered_manifest.version,
            }
        })?;
        let output_validator =
            compile_schema(&registered_manifest.output_schema).map_err(|_| {
                CapabilityRegistryError::InvalidOutputSchema {
                    id: registered_manifest.id.clone(),
                    version: registered_manifest.version,
                }
            })?;

        self.executors.insert(
            key,
            RegisteredExecutor {
                executor,
                input_validator: Arc::new(input_validator),
                output_validator: Arc::new(output_validator),
            },
        );
        Ok(())
    }

    pub async fn execute(
        &self,
        context: CapabilityExecutionContext,
    ) -> Result<CapabilityResult, CapabilityError> {
        if context.attempt().number() != 1 {
            return Err(CapabilityError::validation());
        }
        let manifest = self.manifest_for_context(&context)?;
        self.validate_context(&context, manifest).await?;
        let entry = self.entry_for_context(&context)?;
        validate_instance(
            &entry.input_validator,
            context.normalized_arguments(),
            false,
        )?;
        let lineage_key = (context.invocation().id(), 1);
        let lease_duration_ms = execution_lease_duration_ms(manifest);
        let Some(executing) = self
            .lineage
            .acquire_lease(
                lineage_key.0,
                lineage_key.1,
                None,
                CapabilityLeaseKind::Executing,
                lease_duration_ms,
            )
            .await?
        else {
            return Err(CapabilityError::validation());
        };
        let fence_token = lease_fence(&executing).ok_or_else(CapabilityError::execution)?;
        let executor = entry.executor.clone();
        let execution_fence = ExecutionFence::new(
            fence_token,
            lineage_key.0,
            lineage_key.1,
            CapabilityLeaseKind::Executing,
            context.invocation().idempotency_key(),
            self.lineage.clone(),
        );
        let recording_context = context.clone();
        let execution = executor.execute(context.with_execution_fence(execution_fence.clone()));
        let (execution, executing, heartbeat_error) = self
            .await_with_heartbeat(
                lineage_key,
                executing,
                lease_duration_ms,
                execution_fence,
                execution,
            )
            .await;
        if let Some(error) = heartbeat_error {
            self.transition_to_uncertain(lineage_key, executing).await?;
            return Err(error);
        }
        self.finish_execution(
            lineage_key,
            executing,
            execution,
            entry,
            &recording_context,
            manifest,
        )
        .await
    }

    /// Executes a retry only when it presents a one-time authorization issued by `recover`.
    pub async fn execute_retry(
        &self,
        context: CapabilityExecutionContext,
        authorization: CapabilityRetryAuthorization,
    ) -> Result<CapabilityResult, CapabilityError> {
        let manifest = self.manifest_for_context(&context)?;
        self.validate_context(&context, manifest).await?;
        if !authorization.matches_resume(&context, manifest, &authorization.resume_binding) {
            return Err(CapabilityError::validation());
        }
        let entry = self.entry_for_context(&context)?;
        validate_instance(
            &entry.input_validator,
            context.normalized_arguments(),
            false,
        )?;
        let lineage_key = (context.invocation().id(), context.attempt().number());
        let authorized = CapabilityAttemptLineageState::RetryAuthorized {
            authorization_id: authorization.nonce,
        };
        let lease_duration_ms = execution_lease_duration_ms(manifest);
        let Some(executing) = self
            .lineage
            .acquire_lease(
                lineage_key.0,
                lineage_key.1,
                Some(authorized),
                CapabilityLeaseKind::RetryExecuting,
                lease_duration_ms,
            )
            .await?
        else {
            return Err(CapabilityError::validation());
        };
        let fence_token = lease_fence(&executing).ok_or_else(CapabilityError::execution)?;
        let execution_fence = ExecutionFence::new(
            fence_token,
            lineage_key.0,
            lineage_key.1,
            CapabilityLeaseKind::RetryExecuting,
            context.invocation().idempotency_key(),
            self.lineage.clone(),
        );
        let recording_context = context.clone();
        let execution = entry
            .executor
            .execute(context.with_execution_fence(execution_fence.clone()));
        let (execution, executing, heartbeat_error) = self
            .await_with_heartbeat(
                lineage_key,
                executing,
                lease_duration_ms,
                execution_fence,
                execution,
            )
            .await;
        if let Some(error) = heartbeat_error {
            self.transition_to_uncertain(lineage_key, executing).await?;
            return Err(error);
        }
        self.finish_execution(
            lineage_key,
            executing,
            execution,
            entry,
            &recording_context,
            manifest,
        )
        .await
    }

    /// Validates a durable resume binding against both the live bearer authorization and the
    /// current durable lineage state. This does not consume or recreate retry authority.
    pub async fn validate_recovery_resume(
        &self,
        retry_context: &CapabilityExecutionContext,
        authorization: &CapabilityRetryAuthorization,
        binding: &RecoveryResumeBinding,
    ) -> Result<ValidatedRecoveryResume, CapabilityError> {
        let manifest = self.manifest_for_context(retry_context)?;
        self.validate_context(retry_context, manifest).await?;
        if !authorization.matches_resume(retry_context, manifest, binding) {
            return Err(CapabilityError::validation());
        }
        match self
            .lineage
            .load(binding.logical_invocation_id, binding.retry_attempt_number)
            .await?
        {
            Some(CapabilityAttemptLineageState::RetryAuthorized { authorization_id })
                if authorization_id == authorization.nonce =>
            {
                Ok(ValidatedRecoveryResume {
                    binding: binding.clone(),
                })
            }
            _ => Err(CapabilityError::validation()),
        }
    }

    pub async fn recover(
        &self,
        context: CapabilityExecutionContext,
    ) -> Result<RecoveryAction, CapabilityError> {
        let manifest = self.manifest_for_context(&context)?;
        self.validate_context(&context, manifest).await?;
        let key = (context.invocation().id(), context.attempt().number());
        loop {
            let Some(state) = self.lineage.load(key.0, key.1).await? else {
                return Err(CapabilityError::validation());
            };
            match state.clone() {
                CapabilityAttemptLineageState::Completed(result) => {
                    return Ok(RecoveryAction::Completed(result));
                }
                CapabilityAttemptLineageState::RecoveryRequired => {
                    if manifest.recovery_mode == RecoveryMode::Compensate {
                        if self
                            .lineage
                            .compare_exchange(
                                key.0,
                                key.1,
                                Some(state),
                                CapabilityAttemptLineageState::CompensationRequired,
                            )
                            .await?
                        {
                            return Ok(RecoveryAction::CompensationRequired);
                        }
                        continue;
                    }
                    return Ok(RecoveryAction::RecoveryRequired);
                }
                CapabilityAttemptLineageState::CompensationRequired => {
                    return Ok(RecoveryAction::CompensationRequired);
                }
                CapabilityAttemptLineageState::RetryAuthorized { .. } => {
                    return Err(CapabilityError::validation());
                }
                CapabilityAttemptLineageState::Executing { .. }
                | CapabilityAttemptLineageState::RetryExecuting { .. }
                | CapabilityAttemptLineageState::Reconciling { .. } => {
                    if self
                        .lineage
                        .expire_lease(
                            key.0,
                            key.1,
                            state.clone(),
                            CapabilityAttemptLineageState::Uncertain,
                        )
                        .await?
                    {
                        continue;
                    }
                    if self.lineage.load(key.0, key.1).await? == Some(state) {
                        return Ok(RecoveryAction::Pending);
                    }
                }
                CapabilityAttemptLineageState::AuthoritativeAbsence { .. } => {
                    if manifest.recovery_mode == RecoveryMode::Compensate {
                        if self
                            .lineage
                            .compare_exchange(
                                key.0,
                                key.1,
                                Some(state),
                                CapabilityAttemptLineageState::CompensationRequired,
                            )
                            .await?
                        {
                            return Ok(RecoveryAction::CompensationRequired);
                        }
                        continue;
                    }
                    return self.authorize_retry(&context, manifest, state).await;
                }
                CapabilityAttemptLineageState::Uncertain => match manifest.recovery_mode {
                    RecoveryMode::InherentlyIdempotent
                    | RecoveryMode::KeyedIdempotent
                    | RecoveryMode::Retry
                    | RecoveryMode::Reconcilable
                    | RecoveryMode::Compensate => {
                        return self.reconcile_uncertain(&context, manifest, state).await;
                    }
                    RecoveryMode::NonRetryable | RecoveryMode::None | RecoveryMode::Manual => {
                        if self
                            .lineage
                            .compare_exchange(
                                key.0,
                                key.1,
                                Some(state),
                                CapabilityAttemptLineageState::RecoveryRequired,
                            )
                            .await?
                        {
                            return Ok(RecoveryAction::RecoveryRequired);
                        }
                    }
                },
            }
        }
    }

    async fn validate_context(
        &self,
        context: &CapabilityExecutionContext,
        manifest: &CapabilityManifest,
    ) -> Result<(), CapabilityError> {
        if context.references().run().handle() != context.invocation().run_id()
            || !context.references().secrets_are_declared_by(manifest)
        {
            return Err(CapabilityError::validation());
        }
        self.reference_validator.validate(context, manifest).await?;
        Ok(())
    }

    async fn await_with_heartbeat<F, T>(
        &self,
        key: (Uuid, u32),
        initial_state: CapabilityAttemptLineageState,
        lease_duration_ms: u64,
        execution_fence: ExecutionFence,
        operation: F,
    ) -> (T, CapabilityAttemptLineageState, Option<CapabilityError>)
    where
        F: Future<Output = T>,
    {
        let state = Arc::new(Mutex::new(initial_state));
        let (stop_heartbeat, heartbeat_stopped) = oneshot::channel();
        let heartbeat = self.heartbeat(
            key,
            state.clone(),
            lease_duration_ms,
            execution_fence,
            heartbeat_stopped,
        );
        let (output, heartbeat_error) = match select(Box::pin(operation), Box::pin(heartbeat)).await
        {
            Either::Left((output, heartbeat)) => {
                let _ = stop_heartbeat.send(());
                (output, heartbeat.await.err())
            }
            Either::Right((heartbeat_result, operation)) => {
                drop(stop_heartbeat);
                (operation.await, heartbeat_result.err())
            }
        };
        let latest_state = state.lock().await.clone();
        (output, latest_state, heartbeat_error)
    }

    async fn heartbeat(
        &self,
        key: (Uuid, u32),
        state: Arc<Mutex<CapabilityAttemptLineageState>>,
        lease_duration_ms: u64,
        execution_fence: ExecutionFence,
        heartbeat_stopped: oneshot::Receiver<()>,
    ) -> Result<(), CapabilityError> {
        let interval_ms = heartbeat_interval_ms(lease_duration_ms);
        let mut heartbeat_stopped = Box::pin(heartbeat_stopped);
        loop {
            heartbeat_stopped = match select(
                Box::pin(Delay::new(Duration::from_millis(interval_ms))),
                heartbeat_stopped,
            )
            .await
            {
                Either::Left((_, heartbeat_stopped)) => heartbeat_stopped,
                Either::Right((_, _delay)) => return Ok(()),
            };
            let expected = state.lock().await.clone();
            match self
                .lineage
                .renew_lease(key.0, key.1, expected, lease_duration_ms)
                .await
            {
                Ok(Some(renewed)) => *state.lock().await = renewed,
                Ok(None) => {
                    execution_fence.cancel();
                    return Ok(());
                }
                Err(error) => {
                    execution_fence.cancel();
                    return Err(error);
                }
            }
        }
    }

    async fn finish_execution(
        &self,
        key: (Uuid, u32),
        executing: CapabilityAttemptLineageState,
        execution: Result<CapabilityResult, CapabilityError>,
        entry: &RegisteredExecutor,
        context: &CapabilityExecutionContext,
        manifest: &CapabilityManifest,
    ) -> Result<CapabilityResult, CapabilityError> {
        let mut result = match execution {
            Ok(result) => result,
            Err(error) => {
                self.transition_to_uncertain(key, executing).await?;
                return Err(error);
            }
        };
        if let Err(error) = validate_result(entry, &mut result) {
            self.transition_to_uncertain(key, executing).await?;
            return Err(error);
        }
        let durable = durable_result(context, manifest, &result)?;
        match self
            .result_recorder
            .record(context, manifest, &result, &durable)
            .await
        {
            Ok(()) => {}
            Err(error) => {
                self.transition_to_uncertain(key, executing).await?;
                return Err(error);
            }
        }
        if self
            .lineage
            .compare_exchange(
                key.0,
                key.1,
                Some(executing),
                CapabilityAttemptLineageState::Completed(durable.clone()),
            )
            .await?
        {
            return Ok(result);
        }
        match self.lineage.load(key.0, key.1).await? {
            Some(CapabilityAttemptLineageState::Completed(cached)) if cached == durable => {
                Ok(result)
            }
            _ => Err(CapabilityError::execution()),
        }
    }

    async fn transition_to_uncertain(
        &self,
        key: (Uuid, u32),
        executing: CapabilityAttemptLineageState,
    ) -> Result<(), CapabilityError> {
        if self
            .lineage
            .compare_exchange(
                key.0,
                key.1,
                Some(executing),
                CapabilityAttemptLineageState::Uncertain,
            )
            .await?
        {
            return Ok(());
        }
        match self.lineage.load(key.0, key.1).await? {
            Some(CapabilityAttemptLineageState::Completed(_))
            | Some(CapabilityAttemptLineageState::Uncertain)
            | Some(CapabilityAttemptLineageState::CompensationRequired)
            | Some(CapabilityAttemptLineageState::RecoveryRequired)
            | Some(CapabilityAttemptLineageState::Reconciling { .. })
            | Some(CapabilityAttemptLineageState::AuthoritativeAbsence { .. }) => Ok(()),
            _ => Err(CapabilityError::execution()),
        }
    }

    async fn reconcile_uncertain(
        &self,
        context: &CapabilityExecutionContext,
        manifest: &CapabilityManifest,
        uncertain: CapabilityAttemptLineageState,
    ) -> Result<RecoveryAction, CapabilityError> {
        let entry = self.entry_for_context(context)?;
        validate_instance(
            &entry.input_validator,
            context.normalized_arguments(),
            false,
        )?;
        let key = (context.invocation().id(), context.attempt().number());
        let lease_duration_ms = execution_lease_duration_ms(manifest);
        let Some(reconciling) = self
            .lineage
            .acquire_lease(
                key.0,
                key.1,
                Some(uncertain),
                CapabilityLeaseKind::Reconciling,
                lease_duration_ms,
            )
            .await?
        else {
            return Ok(RecoveryAction::Pending);
        };
        let fence = lease_fence(&reconciling).ok_or_else(CapabilityError::execution)?;
        let execution_fence = ExecutionFence::new(
            fence,
            key.0,
            key.1,
            CapabilityLeaseKind::Reconciling,
            context.invocation().idempotency_key(),
            self.lineage.clone(),
        );
        let reconciliation = entry.executor.reconcile(
            context
                .clone()
                .with_execution_fence(execution_fence.clone()),
        );
        let (outcome, reconciling, heartbeat_error) = self
            .await_with_heartbeat(
                key,
                reconciling,
                lease_duration_ms,
                execution_fence,
                reconciliation,
            )
            .await;
        if let Some(error) = heartbeat_error {
            self.transition_to_uncertain(key, reconciling).await?;
            return Err(error);
        }
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                self.lineage
                    .compare_exchange(
                        key.0,
                        key.1,
                        Some(reconciling),
                        CapabilityAttemptLineageState::Uncertain,
                    )
                    .await?;
                return Err(error);
            }
        };
        match outcome {
            ReconcileOutcome::Completed(mut result) => {
                if let Err(error) = validate_result(entry, &mut result) {
                    self.lineage
                        .compare_exchange(
                            key.0,
                            key.1,
                            Some(reconciling),
                            CapabilityAttemptLineageState::Uncertain,
                        )
                        .await?;
                    return Err(error);
                }
                let durable = durable_result(context, manifest, &result)?;
                match self
                    .result_recorder
                    .record(context, manifest, &result, &durable)
                    .await
                {
                    Ok(()) => {}
                    Err(error) => {
                        self.transition_to_uncertain(key, reconciling).await?;
                        return Err(error);
                    }
                }
                if self
                    .lineage
                    .compare_exchange(
                        key.0,
                        key.1,
                        Some(reconciling),
                        CapabilityAttemptLineageState::Completed(durable.clone()),
                    )
                    .await?
                {
                    Ok(RecoveryAction::Completed(durable))
                } else {
                    self.current_recovery_action(key).await
                }
            }
            ReconcileOutcome::Pending => {
                self.lineage
                    .compare_exchange(
                        key.0,
                        key.1,
                        Some(reconciling),
                        CapabilityAttemptLineageState::Uncertain,
                    )
                    .await?;
                Ok(RecoveryAction::Pending)
            }
            ReconcileOutcome::AuthoritativeAbsence => {
                if manifest.recovery_mode == RecoveryMode::Compensate {
                    if self
                        .lineage
                        .compare_exchange(
                            key.0,
                            key.1,
                            Some(reconciling),
                            CapabilityAttemptLineageState::CompensationRequired,
                        )
                        .await?
                    {
                        return Ok(RecoveryAction::CompensationRequired);
                    }
                    return self.current_recovery_action(key).await;
                }
                let absence = CapabilityAttemptLineageState::AuthoritativeAbsence { fence };
                if self
                    .lineage
                    .compare_exchange(key.0, key.1, Some(reconciling), absence.clone())
                    .await?
                {
                    self.authorize_retry(context, manifest, absence).await
                } else {
                    self.current_recovery_action(key).await
                }
            }
            ReconcileOutcome::RecoveryRequired => {
                let (lineage_state, action) = if manifest.recovery_mode == RecoveryMode::Compensate
                {
                    (
                        CapabilityAttemptLineageState::CompensationRequired,
                        RecoveryAction::CompensationRequired,
                    )
                } else {
                    (
                        CapabilityAttemptLineageState::RecoveryRequired,
                        RecoveryAction::RecoveryRequired,
                    )
                };
                if self
                    .lineage
                    .compare_exchange(key.0, key.1, Some(reconciling), lineage_state)
                    .await?
                {
                    Ok(action)
                } else {
                    self.current_recovery_action(key).await
                }
            }
        }
    }

    async fn current_recovery_action(
        &self,
        key: (Uuid, u32),
    ) -> Result<RecoveryAction, CapabilityError> {
        match self.lineage.load(key.0, key.1).await? {
            Some(CapabilityAttemptLineageState::Completed(result)) => {
                Ok(RecoveryAction::Completed(result))
            }
            Some(CapabilityAttemptLineageState::RecoveryRequired) => {
                Ok(RecoveryAction::RecoveryRequired)
            }
            Some(CapabilityAttemptLineageState::CompensationRequired) => {
                Ok(RecoveryAction::CompensationRequired)
            }
            _ => Ok(RecoveryAction::Pending),
        }
    }

    fn manifest_for_context(
        &self,
        context: &CapabilityExecutionContext,
    ) -> Result<&CapabilityManifest, CapabilityError> {
        self.manifest(
            context.invocation().capability_id(),
            context.invocation().manifest_version(),
        )
        .ok_or_else(CapabilityError::unavailable)
    }

    fn entry_for_context(
        &self,
        context: &CapabilityExecutionContext,
    ) -> Result<&RegisteredExecutor, CapabilityError> {
        self.executors
            .get(&(
                context.invocation().capability_id().to_owned(),
                context.invocation().manifest_version(),
            ))
            .ok_or_else(CapabilityError::unavailable)
    }

    async fn authorize_retry(
        &self,
        context: &CapabilityExecutionContext,
        manifest: &CapabilityManifest,
        absence: CapabilityAttemptLineageState,
    ) -> Result<RecoveryAction, CapabilityError> {
        let current_key = (context.invocation().id(), context.attempt().number());
        if !matches!(
            absence,
            CapabilityAttemptLineageState::AuthoritativeAbsence { .. }
        ) {
            return Err(CapabilityError::validation());
        }
        if self.lineage.load(current_key.0, current_key.1).await? != Some(absence.clone()) {
            return self.current_recovery_action(current_key).await;
        }
        let next_attempt = context
            .attempt()
            .number()
            .checked_add(1)
            .ok_or_else(CapabilityError::validation)?;
        if next_attempt > manifest.max_retries.saturating_add(1) {
            if self
                .lineage
                .compare_exchange(
                    current_key.0,
                    current_key.1,
                    Some(absence),
                    CapabilityAttemptLineageState::RecoveryRequired,
                )
                .await?
            {
                return Ok(RecoveryAction::RecoveryRequired);
            }
            return self.current_recovery_action(current_key).await;
        }
        let next_key = (context.invocation().id(), next_attempt);
        if let Some(CapabilityAttemptLineageState::RetryAuthorized { authorization_id }) =
            self.lineage.load(next_key.0, next_key.1).await?
        {
            return Ok(RecoveryAction::RetrySameKey {
                idempotency_key: context.invocation().idempotency_key(),
                authorization: CapabilityRetryAuthorization::new(
                    authorization_id,
                    context,
                    manifest,
                )?,
            });
        }
        if self.lineage.load(next_key.0, next_key.1).await?.is_some() {
            return Ok(RecoveryAction::RecoveryRequired);
        }
        let nonce = Uuid::new_v4();
        let idempotency_key = context.invocation().idempotency_key();
        if !self
            .lineage
            .compare_exchange(
                next_key.0,
                next_key.1,
                None,
                CapabilityAttemptLineageState::RetryAuthorized {
                    authorization_id: nonce,
                },
            )
            .await?
        {
            if let Some(CapabilityAttemptLineageState::RetryAuthorized { authorization_id }) =
                self.lineage.load(next_key.0, next_key.1).await?
            {
                return Ok(RecoveryAction::RetrySameKey {
                    idempotency_key,
                    authorization: CapabilityRetryAuthorization::new(
                        authorization_id,
                        context,
                        manifest,
                    )?,
                });
            }
            return Ok(RecoveryAction::RecoveryRequired);
        }
        Ok(RecoveryAction::RetrySameKey {
            idempotency_key,
            authorization: CapabilityRetryAuthorization::new(nonce, context, manifest)?,
        })
    }
}

fn validate_result(
    entry: &RegisteredExecutor,
    result: &mut CapabilityResult,
) -> Result<(), CapabilityError> {
    validate_argument_bounds(&result.output).map_err(|_| CapabilityError::output_validation())?;
    validate_instance(&entry.output_validator, &result.output, true)?;
    result.output = canonicalize_json(std::mem::take(&mut result.output))?;
    Ok(())
}

fn durable_result(
    context: &CapabilityExecutionContext,
    manifest: &CapabilityManifest,
    result: &CapabilityResult,
) -> Result<DurableCapabilityResult, CapabilityError> {
    let bytes =
        serde_jcs::to_vec(&result.output).map_err(|_| CapabilityError::output_validation())?;
    let digest = Uuid::new_v5(&CAPABILITY_RESULT_NAMESPACE, &bytes);
    let result_ref = Uuid::new_v5(
        &CAPABILITY_RESULT_NAMESPACE,
        format!(
            "{}:{}",
            context.invocation().id(),
            context.attempt().number()
        )
        .as_bytes(),
    );
    let size_bytes =
        u64::try_from(bytes.len()).map_err(|_| CapabilityError::output_validation())?;
    DurableCapabilityResult::new(
        CapabilityReferenceId::new(result_ref),
        format!("jcs-v1:{digest}"),
        manifest.schema_digest.clone(),
        size_bytes,
        DurableCapabilityStatus::Completed,
    )
}

fn execution_lease_duration_ms(manifest: &CapabilityManifest) -> u64 {
    let grace_ms = (manifest.timeout_ms / 10).clamp(25, 5_000);
    manifest.timeout_ms.saturating_add(grace_ms).max(30)
}

fn heartbeat_interval_ms(lease_duration_ms: u64) -> u64 {
    (lease_duration_ms / 3).clamp(5, 1_000)
}

fn lease_fence(state: &CapabilityAttemptLineageState) -> Option<Uuid> {
    match state {
        CapabilityAttemptLineageState::Executing { fence, .. }
        | CapabilityAttemptLineageState::RetryExecuting { fence, .. }
        | CapabilityAttemptLineageState::Reconciling { fence, .. } => Some(*fence),
        _ => None,
    }
}
