use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use jsonschema::JSONSchema;
use uuid::Uuid;

use super::lineage::InMemoryCapabilityLineageStore;
use super::*;

/// A host-agnostic pairing of the exact portable manifest catalog and host executors.
pub struct CapabilityRegistry {
    catalog: ManifestCatalog,
    executors: BTreeMap<(String, u32), RegisteredExecutor>,
    lineage: Arc<dyn CapabilityLineageStore>,
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
        }
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
        self.validate_context(&context, manifest)?;
        let entry = self.entry_for_context(&context)?;
        validate_instance(
            &entry.input_validator,
            context.normalized_arguments(),
            false,
        )?;
        let lineage_key = (context.invocation().id(), 1);
        let executing = CapabilityAttemptLineageState::Executing {
            fence: Uuid::new_v4(),
            lease_expires_at_ms: lease_expires_at_ms(),
        };
        if !self
            .lineage
            .compare_exchange(lineage_key.0, lineage_key.1, None, executing.clone())
            .await?
        {
            return Err(CapabilityError::validation());
        }
        let executor = entry.executor.clone();
        let execution = executor.execute(context).await;
        self.finish_execution(lineage_key, executing, execution, entry)
            .await
    }

    /// Executes a retry only when it presents a one-time authorization issued by `recover`.
    pub async fn execute_retry(
        &self,
        context: CapabilityExecutionContext,
        authorization: CapabilityRetryAuthorization,
    ) -> Result<CapabilityResult, CapabilityError> {
        let manifest = self.manifest_for_context(&context)?;
        self.validate_context(&context, manifest)?;
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
        let executing = CapabilityAttemptLineageState::RetryExecuting {
            fence: Uuid::new_v4(),
            lease_expires_at_ms: lease_expires_at_ms(),
        };
        if !self
            .lineage
            .compare_exchange(
                lineage_key.0,
                lineage_key.1,
                Some(authorized),
                executing.clone(),
            )
            .await?
        {
            return Err(CapabilityError::validation());
        }
        let execution = entry.executor.execute(context).await;
        self.finish_execution(lineage_key, executing, execution, entry)
            .await
    }

    pub async fn recover(
        &self,
        context: CapabilityExecutionContext,
    ) -> Result<RecoveryAction, CapabilityError> {
        let manifest = self.manifest_for_context(&context)?;
        self.validate_context(&context, manifest)?;
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
                    return Ok(RecoveryAction::RecoveryRequired);
                }
                CapabilityAttemptLineageState::RetryAuthorized { .. } => {
                    return Err(CapabilityError::validation());
                }
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
                } if !lease_is_expired(lease_expires_at_ms) => {
                    return Ok(RecoveryAction::Pending);
                }
                CapabilityAttemptLineageState::Executing { .. }
                | CapabilityAttemptLineageState::RetryExecuting { .. }
                | CapabilityAttemptLineageState::Reconciling { .. } => {
                    if self
                        .lineage
                        .compare_exchange(
                            key.0,
                            key.1,
                            Some(state),
                            CapabilityAttemptLineageState::Uncertain,
                        )
                        .await?
                    {
                        continue;
                    }
                }
                CapabilityAttemptLineageState::AuthoritativeAbsence { .. } => {
                    return self.authorize_retry(&context, manifest, state).await;
                }
                CapabilityAttemptLineageState::Uncertain => match manifest.recovery_mode {
                    RecoveryMode::InherentlyIdempotent
                    | RecoveryMode::KeyedIdempotent
                    | RecoveryMode::Retry => {
                        let absence = CapabilityAttemptLineageState::AuthoritativeAbsence {
                            fence: Uuid::new_v4(),
                        };
                        if self
                            .lineage
                            .compare_exchange(key.0, key.1, Some(state), absence.clone())
                            .await?
                        {
                            return self.authorize_retry(&context, manifest, absence).await;
                        }
                    }
                    RecoveryMode::Reconcilable | RecoveryMode::Compensate => {
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

    fn validate_context(
        &self,
        context: &CapabilityExecutionContext,
        manifest: &CapabilityManifest,
    ) -> Result<(), CapabilityError> {
        if context.references().run().handle() != context.invocation().run_id()
            || !context.references().secrets_are_declared_by(manifest)
        {
            return Err(CapabilityError::validation());
        }
        Ok(())
    }

    async fn finish_execution(
        &self,
        key: (Uuid, u32),
        executing: CapabilityAttemptLineageState,
        execution: Result<CapabilityResult, CapabilityError>,
        entry: &RegisteredExecutor,
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
        if self
            .lineage
            .compare_exchange(
                key.0,
                key.1,
                Some(executing),
                CapabilityAttemptLineageState::Completed(result.clone()),
            )
            .await?
        {
            return Ok(result);
        }
        match self.lineage.load(key.0, key.1).await? {
            Some(CapabilityAttemptLineageState::Completed(cached)) => Ok(cached),
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
        let fence = Uuid::new_v4();
        let reconciling = CapabilityAttemptLineageState::Reconciling {
            fence,
            lease_expires_at_ms: lease_expires_at_ms(),
        };
        if !self
            .lineage
            .compare_exchange(key.0, key.1, Some(uncertain), reconciling.clone())
            .await?
        {
            return Ok(RecoveryAction::Pending);
        }
        let outcome = match entry.executor.reconcile(context.clone()).await {
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
                if self
                    .lineage
                    .compare_exchange(
                        key.0,
                        key.1,
                        Some(reconciling),
                        CapabilityAttemptLineageState::Completed(result.clone()),
                    )
                    .await?
                {
                    Ok(RecoveryAction::Completed(result))
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
                if self
                    .lineage
                    .compare_exchange(
                        key.0,
                        key.1,
                        Some(reconciling),
                        CapabilityAttemptLineageState::RecoveryRequired,
                    )
                    .await?
                {
                    Ok(RecoveryAction::RecoveryRequired)
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
                authorization: CapabilityRetryAuthorization {
                    nonce: authorization_id,
                },
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
                    authorization: CapabilityRetryAuthorization {
                        nonce: authorization_id,
                    },
                });
            }
            return Ok(RecoveryAction::RecoveryRequired);
        }
        Ok(RecoveryAction::RetrySameKey {
            idempotency_key,
            authorization: CapabilityRetryAuthorization { nonce },
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

fn lease_expires_at_ms() -> u64 {
    current_time_ms().saturating_add(CAPABILITY_EXECUTION_LEASE_MS)
}

fn lease_is_expired(lease_expires_at_ms: u64) -> bool {
    lease_expires_at_ms <= current_time_ms()
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}
