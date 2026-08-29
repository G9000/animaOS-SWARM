use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anima_core::{Content, DataValue, MessageRole, TaskResult, TaskStatus};
use async_trait::async_trait;
use tokio::sync::{oneshot, watch, Mutex};
use tokio::task::JoinHandle;

use super::credentials::{ConnectorCredentialStore, CredentialStoreError, TelegramBotToken};
use super::telegram::{
    TelegramClient, TelegramSentMessage, TelegramTransportError, TelegramUpdateBatch,
};
use super::{
    TelegramBotIdentity, TelegramConnectorRecord, TelegramCredentialCleanupIntent,
    TelegramInboundRecord, TelegramPendingPairing,
};
use crate::agent_runs::{AgentRunCoordinator, AgentRunRequest, RunRoom};
use crate::app::SharedDaemonState;
use crate::connectors::{InboundProcessingState, OutboundDeliveryState, TelegramOutboundRecord};
use crate::routes::{AgentRunEnvelope, AgentRuntimeSnapshotResponse, ApiError, TaskResultResponse};
use crate::schedules::{ScheduleOutcomeStatus, ScheduleSafeOutcome, ScheduleTarget};
use crate::state::DaemonState;

static CONNECTOR_ID_SEQUENCE: AtomicU64 = AtomicU64::new(1);
const DELIVERED_RETENTION_MS: u64 = 7 * 24 * 60 * 60 * 1000;
const MAX_RETAINED_DELIVERED: usize = 1000;
const POLL_RETRY_INITIAL: Duration = Duration::from_millis(100);
const POLL_RETRY_MAX: Duration = Duration::from_secs(30);
const PAIRING_CANDIDATE_TTL_MS: u64 = 10 * 60 * 1000;
const MAX_RETAINED_TERMINAL_INBOUND: usize = 1000;
const MAX_LIVE_INBOUND: usize = 100;
const MAX_UNDELIVERED_OUTBOUND: usize = 100;

#[async_trait]
pub(crate) trait TelegramTransport: Send + Sync {
    async fn get_me(
        &self,
        token: &TelegramBotToken,
    ) -> Result<TelegramBotIdentity, TelegramTransportError>;
    async fn get_updates(
        &self,
        token: &TelegramBotToken,
        offset: i64,
    ) -> Result<TelegramUpdateBatch, TelegramTransportError>;
    async fn send_message(
        &self,
        token: &TelegramBotToken,
        chat_id: &str,
        text: &str,
    ) -> Result<Vec<TelegramSentMessage>, TelegramTransportError>;
}

#[async_trait]
impl TelegramTransport for TelegramClient {
    async fn get_me(
        &self,
        token: &TelegramBotToken,
    ) -> Result<TelegramBotIdentity, TelegramTransportError> {
        TelegramClient::get_me(self, token).await
    }

    async fn get_updates(
        &self,
        token: &TelegramBotToken,
        offset: i64,
    ) -> Result<TelegramUpdateBatch, TelegramTransportError> {
        TelegramClient::get_updates(self, token, offset).await
    }

    async fn send_message(
        &self,
        token: &TelegramBotToken,
        chat_id: &str,
        text: &str,
    ) -> Result<Vec<TelegramSentMessage>, TelegramTransportError> {
        TelegramClient::send_message(self, token, chat_id, text).await
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConnectorRuntimeStatus {
    Ready,
    Pairing,
    CredentialRequired,
    Error,
    Degraded,
    Reconciling,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConnectorManagerError {
    AgentNotFound,
    ConnectorNotFound,
    AgentAlreadyConnected,
    PendingPairingNotFound,
    InvalidToken,
    Transport,
    Credential,
    CredentialStateUncertain,
    Persistence,
    Backpressure,
    ConflictingUpdate,
    IdempotencyConflict,
    WorkerStopped,
}

impl fmt::Display for ConnectorManagerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::AgentNotFound => "agent not found",
            Self::ConnectorNotFound => "connector not found",
            Self::AgentAlreadyConnected => "agent already has an active Telegram connector",
            Self::PendingPairingNotFound => "Telegram pairing candidate was not found",
            Self::InvalidToken => "Telegram bot token is invalid",
            Self::Transport => "Telegram transport failed",
            Self::Credential => "credential vault operation failed",
            Self::CredentialStateUncertain => "credential vault state requires reconciliation",
            Self::Persistence => "connector state persistence failed",
            Self::Backpressure => "connector inbound capacity is exhausted",
            Self::ConflictingUpdate => "Telegram update conflicts with durable history",
            Self::IdempotencyConflict => "connector idempotency key conflicts with committed text",
            Self::WorkerStopped => "connector worker stopped unexpectedly",
        })
    }
}

impl std::error::Error for ConnectorManagerError {}

#[derive(Clone)]
pub(crate) struct ConnectorManager {
    state: SharedDaemonState,
    runs: AgentRunCoordinator,
    credentials: Arc<dyn ConnectorCredentialStore>,
    transport: Arc<dyn TelegramTransport>,
    lifecycle_lock: Arc<Mutex<()>>,
    mutation_lock: Arc<Mutex<()>>,
    statuses: Arc<Mutex<HashMap<String, ConnectorRuntimeStatus>>>,
    workers: Arc<Mutex<HashMap<String, WorkerHandle>>>,
    owner_send_locks: Arc<StdMutex<HashMap<(String, String), Weak<Mutex<()>>>>>,
    worker_generation: Arc<AtomicU64>,
    closing: Arc<AtomicBool>,
    owner_close: Option<watch::Sender<bool>>,
}

struct WorkerHandle {
    generation: u64,
    cancel: watch::Sender<bool>,
    join: JoinHandle<()>,
}

#[derive(Default)]
struct ConnectorRunRollbackDelta {
    committed_target: Option<TelegramInboundRecord>,
    inserted_outbound: Option<TelegramOutboundRecord>,
    removed_terminal: Vec<((String, i64), TelegramInboundRecord)>,
}

impl ConnectorManager {
    pub(crate) fn new(
        state: SharedDaemonState,
        runs: AgentRunCoordinator,
        credentials: Arc<dyn ConnectorCredentialStore>,
        transport: Arc<dyn TelegramTransport>,
    ) -> Self {
        let mutation_lock = runs.control_plane_transactions();
        let (owner_close, _) = watch::channel(false);
        Self {
            state,
            runs,
            credentials,
            transport,
            lifecycle_lock: Arc::new(Mutex::new(())),
            mutation_lock,
            statuses: Arc::new(Mutex::new(HashMap::new())),
            workers: Arc::new(Mutex::new(HashMap::new())),
            owner_send_locks: Arc::new(StdMutex::new(HashMap::new())),
            worker_generation: Arc::new(AtomicU64::new(1)),
            closing: Arc::new(AtomicBool::new(false)),
            owner_close: Some(owner_close),
        }
    }

    fn ensure_open(&self) -> Result<(), ConnectorManagerError> {
        if self.closing.load(Ordering::SeqCst) {
            Err(ConnectorManagerError::WorkerStopped)
        } else {
            Ok(())
        }
    }

    fn owner_send_lock(&self, connector_id: &str, idempotency_key: &str) -> Arc<Mutex<()>> {
        let mut locks = self
            .owner_send_locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        locks.retain(|_, lock| lock.strong_count() > 0);
        let key = (connector_id.to_string(), idempotency_key.to_string());
        if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(Mutex::new(()));
        locks.insert(key, Arc::downgrade(&lock));
        lock
    }

    pub(crate) async fn create(
        &self,
        agent_id: String,
        token: TelegramBotToken,
    ) -> Result<TelegramConnectorRecord, ConnectorManagerError> {
        let manager = self.clone();
        tokio::spawn(async move { manager.create_owned(agent_id, token).await })
            .await
            .map_err(|_| ConnectorManagerError::WorkerStopped)?
    }

    async fn create_owned(
        &self,
        agent_id: String,
        token: TelegramBotToken,
    ) -> Result<TelegramConnectorRecord, ConnectorManagerError> {
        let _lifecycle = self.lifecycle_lock.lock().await;
        self.ensure_open()?;
        {
            let state = self.state.read().await;
            if state.get_agent(&agent_id).is_none() {
                return Err(ConnectorManagerError::AgentNotFound);
            }
            if state.connectors.values().any(|connector| {
                connector.agent_id == agent_id && connector.deleted_at_ms.is_none()
            }) {
                return Err(ConnectorManagerError::AgentAlreadyConnected);
            }
        }

        let bot = self
            .transport
            .get_me(&token)
            .await
            .map_err(map_token_validation_error)?;
        let now = now_ms();
        let id = next_connector_id(now);
        let connector = TelegramConnectorRecord {
            id: id.clone(),
            agent_id,
            room_id: format!("telegram:{id}"),
            bot,
            approved_chat: None,
            pending_pairing: None,
            next_update_id: 0,
            enabled: true,
            deleted_at_ms: None,
            created_at_ms: now,
            updated_at_ms: now,
        };

        let _mutation = self.mutation_lock.lock().await;
        let intent = TelegramCredentialCleanupIntent {
            connector_id: id.clone(),
            created_at_ms: now,
        };
        let intent_persist = {
            let mut state = self.state.write().await;
            if state.get_agent(&connector.agent_id).is_none() {
                return Err(ConnectorManagerError::AgentNotFound);
            }
            if state.connectors.values().any(|candidate| {
                candidate.agent_id == connector.agent_id && candidate.deleted_at_ms.is_none()
            }) {
                return Err(ConnectorManagerError::AgentAlreadyConnected);
            }
            state.credential_cleanup.insert(id.clone(), intent.clone());
            state.control_plane_persist_request()
        };
        if intent_persist.save().await.is_err() {
            self.state.write().await.credential_cleanup.remove(&id);
            return Err(ConnectorManagerError::Persistence);
        }
        drop(_mutation);

        let worker_token = token.clone();
        if let Err(error) = self.credentials.put(&id, token).await {
            let mapped = map_credential_error(error);
            return self.cleanup_failed_create_vault(&id, mapped).await;
        }

        let _mutation = self.mutation_lock.lock().await;
        let persist = {
            let mut state = self.state.write().await;
            if state.get_agent(&connector.agent_id).is_none() {
                drop(state);
                drop(_mutation);
                return self
                    .cleanup_failed_create_vault(&id, ConnectorManagerError::AgentNotFound)
                    .await;
            }
            if state.connectors.values().any(|candidate| {
                candidate.agent_id == connector.agent_id && candidate.deleted_at_ms.is_none()
            }) {
                drop(state);
                drop(_mutation);
                return self
                    .cleanup_failed_create_vault(&id, ConnectorManagerError::AgentAlreadyConnected)
                    .await;
            }
            state.connectors.insert(id.clone(), connector.clone());
            state.credential_cleanup.remove(&id);
            state.control_plane_persist_request()
        };
        if persist.save().await.is_err() {
            let mut state = self.state.write().await;
            state.connectors.remove(&id);
            state.credential_cleanup.insert(id.clone(), intent);
            drop(state);
            drop(_mutation);
            return self
                .cleanup_failed_create_vault(&id, ConnectorManagerError::Persistence)
                .await;
        }
        self.statuses
            .lock()
            .await
            .insert(id.clone(), ConnectorRuntimeStatus::Pairing);
        drop(_mutation);
        self.start_worker(id, worker_token).await?;
        Ok(connector)
    }

    async fn cleanup_failed_create_vault(
        &self,
        connector_id: &str,
        original_error: ConnectorManagerError,
    ) -> Result<TelegramConnectorRecord, ConnectorManagerError> {
        if self.credentials.delete(connector_id).await.is_err() {
            self.statuses.lock().await.insert(
                connector_id.to_string(),
                ConnectorRuntimeStatus::Reconciling,
            );
            return Err(ConnectorManagerError::CredentialStateUncertain);
        }
        let _mutation = self.mutation_lock.lock().await;
        let (intent, persist) = {
            let mut state = self.state.write().await;
            let intent = state.credential_cleanup.remove(connector_id);
            (intent, state.control_plane_persist_request())
        };
        if persist.save().await.is_err() {
            if let Some(intent) = intent {
                self.state
                    .write()
                    .await
                    .credential_cleanup
                    .insert(connector_id.to_string(), intent);
            }
            self.statuses.lock().await.insert(
                connector_id.to_string(),
                ConnectorRuntimeStatus::Reconciling,
            );
            return Err(ConnectorManagerError::Persistence);
        }
        self.statuses.lock().await.remove(connector_id);
        Err(original_error)
    }

    pub(crate) async fn replace_token(
        &self,
        connector_id: String,
        token: TelegramBotToken,
    ) -> Result<TelegramConnectorRecord, ConnectorManagerError> {
        let manager = self.clone();
        tokio::spawn(async move { manager.replace_token_owned(connector_id, token).await })
            .await
            .map_err(|_| ConnectorManagerError::WorkerStopped)?
    }

    async fn replace_token_owned(
        &self,
        connector_id: String,
        token: TelegramBotToken,
    ) -> Result<TelegramConnectorRecord, ConnectorManagerError> {
        let _lifecycle = self.lifecycle_lock.lock().await;
        self.ensure_open()?;
        let preflight_connector = {
            let state = self.state.read().await;
            state
                .connectors
                .get(&connector_id)
                .filter(|connector| connector.deleted_at_ms.is_none())
                .cloned()
                .ok_or(ConnectorManagerError::ConnectorNotFound)?
        };
        let previous_status = self
            .statuses
            .lock()
            .await
            .get(&connector_id)
            .copied()
            .unwrap_or_else(|| connector_operational_status(&preflight_connector));
        let bot = self
            .transport
            .get_me(&token)
            .await
            .map_err(map_token_validation_error)?;
        let previous_token = match self.credentials.load(&connector_id).await {
            Ok(token) => token,
            Err(error) => {
                let mapped = map_credential_error(error);
                if mapped == ConnectorManagerError::CredentialStateUncertain {
                    self.stop_worker(&connector_id).await?;
                    self.statuses
                        .lock()
                        .await
                        .insert(connector_id, ConnectorRuntimeStatus::Reconciling);
                }
                return Err(mapped);
            }
        };
        let worker_token = token.clone();
        if let Err(error) = self.credentials.put(&connector_id, token).await {
            let mapped = map_credential_error(error);
            if mapped == ConnectorManagerError::CredentialStateUncertain {
                self.stop_worker(&connector_id).await?;
                self.statuses
                    .lock()
                    .await
                    .insert(connector_id, ConnectorRuntimeStatus::Reconciling);
            }
            return Err(mapped);
        }

        if let Err(error) = self.stop_worker(&connector_id).await {
            return match self
                .restore_previous_worker(connector_id, previous_token, previous_status)
                .await
            {
                Ok(()) => Err(error),
                Err(compensation_error) => Err(compensation_error),
            };
        }
        let _mutation = self.mutation_lock.lock().await;

        let transaction = {
            let mut state = self.state.write().await;
            state
                .connectors
                .get(&connector_id)
                .cloned()
                .map(|previous| {
                    let connector = state
                        .connectors
                        .get_mut(&connector_id)
                        .expect("connector was just found");
                    connector.bot = bot;
                    connector.enabled = true;
                    connector.updated_at_ms = now_ms();
                    let updated = connector.clone();
                    let persist = state.control_plane_persist_request();
                    (previous, updated, persist)
                })
        };
        let Some((previous, updated, persist)) = transaction else {
            let rollback = {
                let mut state = self.state.write().await;
                state
                    .connectors
                    .insert(connector_id.clone(), preflight_connector);
                state.control_plane_persist_request()
            };
            if rollback.save().await.is_err() {
                drop(_mutation);
                let _ = self
                    .restore_previous_worker(connector_id.clone(), previous_token, previous_status)
                    .await;
                self.statuses
                    .lock()
                    .await
                    .insert(connector_id, ConnectorRuntimeStatus::Reconciling);
                return Err(ConnectorManagerError::Persistence);
            }
            drop(_mutation);
            self.restore_previous_worker(connector_id, previous_token, previous_status)
                .await?;
            return Err(ConnectorManagerError::ConnectorNotFound);
        };
        if persist.save().await.is_err() {
            self.state
                .write()
                .await
                .connectors
                .insert(connector_id.clone(), previous.clone());
            drop(_mutation);
            self.restore_previous_worker(connector_id, previous_token, previous_status)
                .await?;
            return Err(ConnectorManagerError::Persistence);
        }
        drop(_mutation);
        if let Err(error) = self.start_worker(connector_id.clone(), worker_token).await {
            let _mutation = self.mutation_lock.lock().await;
            let rollback = {
                let mut state = self.state.write().await;
                state
                    .connectors
                    .insert(connector_id.clone(), previous.clone());
                state.control_plane_persist_request()
            };
            if rollback.save().await.is_err() {
                drop(_mutation);
                let _ = self
                    .restore_previous_worker(connector_id.clone(), previous_token, previous_status)
                    .await;
                self.statuses
                    .lock()
                    .await
                    .insert(connector_id, ConnectorRuntimeStatus::Reconciling);
                return Err(ConnectorManagerError::Persistence);
            }
            drop(_mutation);
            return match self
                .restore_previous_worker(connector_id, previous_token, previous_status)
                .await
            {
                Ok(()) => Err(error),
                Err(compensation_error) => Err(compensation_error),
            };
        }
        self.statuses
            .lock()
            .await
            .insert(connector_id, connector_operational_status(&updated));
        Ok(updated)
    }

    async fn restore_previous_worker(
        &self,
        connector_id: String,
        previous_token: Option<TelegramBotToken>,
        previous_status: ConnectorRuntimeStatus,
    ) -> Result<(), ConnectorManagerError> {
        let Some(previous_token) = previous_token else {
            if self.credentials.delete(&connector_id).await.is_err() {
                self.statuses
                    .lock()
                    .await
                    .insert(connector_id, ConnectorRuntimeStatus::Reconciling);
                return Err(ConnectorManagerError::CredentialStateUncertain);
            }
            self.statuses
                .lock()
                .await
                .insert(connector_id, ConnectorRuntimeStatus::CredentialRequired);
            return Ok(());
        };
        if self
            .credentials
            .put(&connector_id, previous_token.clone())
            .await
            .is_err()
        {
            self.statuses
                .lock()
                .await
                .insert(connector_id, ConnectorRuntimeStatus::Reconciling);
            return Err(ConnectorManagerError::CredentialStateUncertain);
        }
        if self
            .start_worker(connector_id.clone(), previous_token)
            .await
            .is_err()
        {
            self.statuses
                .lock()
                .await
                .insert(connector_id, ConnectorRuntimeStatus::Reconciling);
            return Err(ConnectorManagerError::WorkerStopped);
        }
        self.statuses
            .lock()
            .await
            .insert(connector_id, previous_status);
        Ok(())
    }

    pub(crate) async fn accept_batch(
        &self,
        connector_id: String,
        batch: TelegramUpdateBatch,
    ) -> Result<(), ConnectorManagerError> {
        let manager = self.clone();
        tokio::spawn(async move { manager.accept_batch_owned(connector_id, batch).await })
            .await
            .map_err(|_| ConnectorManagerError::WorkerStopped)?
    }

    async fn accept_batch_owned(
        &self,
        connector_id: String,
        batch: TelegramUpdateBatch,
    ) -> Result<(), ConnectorManagerError> {
        let _mutation = self.mutation_lock.lock().await;
        let (previous_connector, previous_inbound, persist) = {
            let mut state = self.state.write().await;
            let previous_connector = state
                .connectors
                .get(&connector_id)
                .cloned()
                .ok_or(ConnectorManagerError::ConnectorNotFound)?;
            if !previous_connector.is_active() {
                return Err(ConnectorManagerError::ConnectorNotFound);
            }
            if batch.next_update_id < previous_connector.next_update_id {
                return Err(ConnectorManagerError::ConflictingUpdate);
            }
            let previous_inbound = state
                .inbound
                .iter()
                .filter(|((record_connector_id, _), _)| record_connector_id == &connector_id)
                .map(|(key, record)| (key.clone(), record.clone()))
                .collect::<Vec<_>>();
            let approved_chat_id = previous_connector
                .approved_chat
                .as_ref()
                .map(|chat| chat.id.clone());
            let now = now_ms();

            if let Some(approved_chat_id) = approved_chat_id {
                let mut new_records = Vec::new();
                for update in &batch.updates {
                    if update.chat.id != approved_chat_id {
                        continue;
                    }
                    let key = (connector_id.clone(), update.update_id);
                    let candidate = TelegramInboundRecord {
                        connector_id: connector_id.clone(),
                        update_id: update.update_id,
                        agent_id: previous_connector.agent_id.clone(),
                        room_id: previous_connector.room_id.clone(),
                        normalized_text: update.text.clone(),
                        sender: update.sender.clone(),
                        chat: update.chat.clone(),
                        received_at_ms: now,
                        processing_state: InboundProcessingState::Received,
                        run_idempotency_key: format!(
                            "telegram:{connector_id}:update:{}",
                            update.update_id
                        ),
                    };
                    if let Some(existing) = state.inbound.get(&key) {
                        if !same_inbound_identity(existing, &candidate) {
                            return Err(ConnectorManagerError::ConflictingUpdate);
                        }
                    } else if let Some((_, existing)) = new_records
                        .iter()
                        .find(|(candidate_key, _)| candidate_key == &key)
                    {
                        if !same_inbound_identity(existing, &candidate) {
                            return Err(ConnectorManagerError::ConflictingUpdate);
                        }
                    } else {
                        new_records.push((key, candidate));
                    }
                }
                let live_records = state
                    .inbound
                    .values()
                    .filter(|record| {
                        record.connector_id == connector_id
                            && matches!(
                                record.processing_state,
                                InboundProcessingState::Received
                                    | InboundProcessingState::Processing
                            )
                    })
                    .count();
                if live_records.saturating_add(new_records.len()) > MAX_LIVE_INBOUND {
                    drop(state);
                    self.statuses
                        .lock()
                        .await
                        .insert(connector_id, ConnectorRuntimeStatus::Degraded);
                    return Err(ConnectorManagerError::Backpressure);
                }
                for (key, record) in new_records {
                    state.inbound.insert(key, record);
                }
            } else if let Some(update) = batch.updates.last() {
                let connector = state
                    .connectors
                    .get_mut(&connector_id)
                    .expect("connector was prevalidated");
                connector.pending_pairing = Some(TelegramPendingPairing {
                    chat: update.chat.clone(),
                    requested_at_ms: now,
                });
            }

            let connector = state
                .connectors
                .get_mut(&connector_id)
                .expect("connector was prevalidated");
            connector.next_update_id = connector.next_update_id.max(batch.next_update_id);
            connector.updated_at_ms = now;
            let _ = compact_terminal_inbound(&mut state.inbound, &connector_id);
            let persist = state.control_plane_persist_request();
            (previous_connector, previous_inbound, persist)
        };

        if persist.save().await.is_err() {
            let mut state = self.state.write().await;
            state
                .connectors
                .insert(connector_id.clone(), previous_connector);
            state
                .inbound
                .retain(|(record_connector_id, _), _| record_connector_id != &connector_id);
            state.inbound.extend(previous_inbound);
            drop(state);
            self.statuses
                .lock()
                .await
                .insert(connector_id, ConnectorRuntimeStatus::Error);
            return Err(ConnectorManagerError::Persistence);
        }
        Ok(())
    }

    pub(crate) async fn approve_pending(
        &self,
        connector_id: String,
    ) -> Result<TelegramConnectorRecord, ConnectorManagerError> {
        self.approve_pending_chat(connector_id, None).await
    }

    pub(crate) async fn approve_pending_chat(
        &self,
        connector_id: String,
        expected_chat_id: Option<String>,
    ) -> Result<TelegramConnectorRecord, ConnectorManagerError> {
        let manager = self.clone();
        tokio::spawn(async move {
            manager
                .approve_pending_owned(connector_id, expected_chat_id)
                .await
        })
        .await
        .map_err(|_| ConnectorManagerError::WorkerStopped)?
    }

    async fn approve_pending_owned(
        &self,
        connector_id: String,
        expected_chat_id: Option<String>,
    ) -> Result<TelegramConnectorRecord, ConnectorManagerError> {
        let _mutation = self.mutation_lock.lock().await;
        let stale = {
            let state = self.state.read().await;
            state
                .connectors
                .get(&connector_id)
                .and_then(|connector| connector.pending_pairing.as_ref())
                .is_some_and(|pending| {
                    now_ms().saturating_sub(pending.requested_at_ms) > PAIRING_CANDIDATE_TTL_MS
                })
        };
        if stale {
            let (previous, persist) = {
                let mut state = self.state.write().await;
                let connector = state
                    .connectors
                    .get_mut(&connector_id)
                    .ok_or(ConnectorManagerError::ConnectorNotFound)?;
                let previous = connector.clone();
                connector.pending_pairing = None;
                connector.updated_at_ms = now_ms();
                let persist = state.control_plane_persist_request();
                (previous, persist)
            };
            if persist.save().await.is_err() {
                self.state
                    .write()
                    .await
                    .connectors
                    .insert(connector_id, previous);
                return Err(ConnectorManagerError::Persistence);
            }
            return Err(ConnectorManagerError::PendingPairingNotFound);
        }
        let (previous, updated, persist) = {
            let mut state = self.state.write().await;
            let previous = state
                .connectors
                .get(&connector_id)
                .cloned()
                .ok_or(ConnectorManagerError::ConnectorNotFound)?;
            if !previous.is_active() {
                return Err(ConnectorManagerError::ConnectorNotFound);
            }
            let pending = previous
                .pending_pairing
                .clone()
                .ok_or(ConnectorManagerError::PendingPairingNotFound)?;
            if expected_chat_id
                .as_deref()
                .is_some_and(|expected| expected != pending.chat.id)
            {
                return Err(ConnectorManagerError::PendingPairingNotFound);
            }
            let connector = state
                .connectors
                .get_mut(&connector_id)
                .expect("connector was prevalidated");
            connector.approved_chat = Some(pending.chat);
            connector.pending_pairing = None;
            connector.updated_at_ms = now_ms();
            let updated = connector.clone();
            let persist = state.control_plane_persist_request();
            (previous, updated, persist)
        };
        if persist.save().await.is_err() {
            self.state
                .write()
                .await
                .connectors
                .insert(connector_id.clone(), previous);
            self.statuses
                .lock()
                .await
                .insert(connector_id, ConnectorRuntimeStatus::Error);
            return Err(ConnectorManagerError::Persistence);
        }
        self.statuses
            .lock()
            .await
            .insert(connector_id, ConnectorRuntimeStatus::Ready);
        Ok(updated)
    }

    pub(crate) async fn send_from_owner(
        &self,
        agent_id: String,
        connector_id: String,
        text: String,
        idempotency_key: String,
    ) -> Result<(crate::routes::AgentRunEnvelope, bool), ConnectorManagerError> {
        let manager = self.clone();
        tokio::spawn(async move {
            manager
                .send_from_owner_owned(agent_id, connector_id, text, idempotency_key)
                .await
        })
        .await
        .map_err(|_| ConnectorManagerError::WorkerStopped)?
    }

    async fn send_from_owner_owned(
        &self,
        agent_id: String,
        connector_id: String,
        text: String,
        idempotency_key: String,
    ) -> Result<(crate::routes::AgentRunEnvelope, bool), ConnectorManagerError> {
        let owner_send_lock = self.owner_send_lock(&connector_id, &idempotency_key);
        let _owner_send_guard = owner_send_lock.lock().await;
        let (connector, connector_worker_generation, replay) = {
            let _lifecycle = self.lifecycle_lock.lock().await;
            self.ensure_open()?;
            let state = self.state.read().await;
            let connector = state
                .connectors
                .get(&connector_id)
                .filter(|connector| connector.is_active() && connector.agent_id == agent_id)
                .cloned()
                .ok_or(ConnectorManagerError::ConnectorNotFound)?;
            if connector.approved_chat.is_some()
                && state
                    .outbound
                    .values()
                    .filter(|record| {
                        record.connector_id == connector_id
                            && record.delivery_state != OutboundDeliveryState::Delivered
                    })
                    .count()
                    >= MAX_UNDELIVERED_OUTBOUND
            {
                return Err(ConnectorManagerError::Backpressure);
            }
            let worker_generation = self
                .workers
                .lock()
                .await
                .get(&connector_id)
                .map(|worker| worker.generation);
            let replay = owner_send_replay(&state, &connector, &text, &idempotency_key)?;
            (connector, worker_generation, replay)
        };
        if let Some(replay) = replay {
            return Ok(replay);
        }
        let permit = self
            .runs
            .try_admit()
            .map_err(|_| ConnectorManagerError::Backpressure)?;
        let commit_connector_id = connector.id.clone();
        let commit_agent_id = connector.agent_id.clone();
        let commit_room_id = connector.room_id.clone();
        let commit_chat_id = connector.approved_chat.as_ref().map(|chat| chat.id.clone());
        let delivery_queued = commit_chat_id.is_some();
        let rollback_outbound = Arc::new(std::sync::Mutex::new(None::<TelegramOutboundRecord>));
        let commit_rollback_outbound = Arc::clone(&rollback_outbound);
        let commit_lifecycle_lock = Arc::clone(&self.lifecycle_lock);
        let commit_workers = Arc::clone(&self.workers);
        let request = AgentRunRequest {
            agent_id: connector.agent_id.clone(),
            content: Content {
                text,
                metadata: Some(BTreeMap::from([
                    ("source".into(), DataValue::String("telegramThread".into())),
                    (
                        "connectorId".into(),
                        DataValue::String(connector.id.clone()),
                    ),
                ])),
                attachments: None,
            },
            room: RunRoom::Stable(connector.room_id.clone()),
            idempotency_key: Some(idempotency_key),
        };
        let run = self
            .runs
            .run_with_commit_admitted_and_rollback(
                request,
                permit,
                move |state, snapshot, result| {
                    let _lifecycle = commit_lifecycle_lock.try_lock().map_err(|_| {
                        ApiError::service_unavailable("connector lifecycle changed during run")
                    })?;
                    let workers = commit_workers.try_lock().map_err(|_| {
                        ApiError::service_unavailable("connector worker changed during run")
                    })?;
                    if workers
                        .get(&commit_connector_id)
                        .map(|worker| worker.generation)
                        != connector_worker_generation
                    {
                        return Err(ApiError::service_unavailable(
                            "connector worker changed during run",
                        ));
                    }
                    let current = state
                        .connectors
                        .get(&commit_connector_id)
                        .filter(|current| {
                            current.is_active()
                                && current.agent_id == commit_agent_id
                                && current.room_id == commit_room_id
                                && current.approved_chat.as_ref().map(|chat| &chat.id)
                                    == commit_chat_id.as_ref()
                        })
                        .ok_or_else(|| ApiError::not_found())?;
                    if result.status == TaskStatus::Error || commit_chat_id.is_none() {
                        return Ok(());
                    }
                    if state
                        .outbound
                        .values()
                        .filter(|record| {
                            record.connector_id == commit_connector_id
                                && record.delivery_state != OutboundDeliveryState::Delivered
                        })
                        .count()
                        >= MAX_UNDELIVERED_OUTBOUND
                    {
                        return Err(ApiError::service_unavailable(
                            "connector outbound capacity is exhausted",
                        ));
                    }
                    let assistant = snapshot
                        .messages
                        .iter()
                        .rev()
                        .find(|message| {
                            message.room_id == current.room_id
                                && message.role == MessageRole::Assistant
                        })
                        .cloned()
                        .ok_or_else(|| {
                            ApiError::bad_request("agent produced no assistant message")
                        })?;
                    let outbound_id = format!(
                        "telegram:{}:web:{}:outbound",
                        commit_connector_id, assistant.id
                    );
                    let outbound = TelegramOutboundRecord {
                        id: outbound_id.clone(),
                        connector_id: commit_connector_id.clone(),
                        agent_id: commit_agent_id.clone(),
                        room_id: commit_room_id.clone(),
                        assistant_message_id: assistant.id,
                        text: assistant.content.text,
                        created_at_ms: now_ms(),
                        delivered_at_ms: None,
                        attempts: 0,
                        delivery_state: OutboundDeliveryState::Pending,
                    };
                    if let Some(existing) = state.outbound.get(&outbound_id) {
                        if existing != &outbound {
                            return Err(ApiError::bad_request(
                                "connector outbound conflicts with run",
                            ));
                        }
                    } else {
                        state.outbound.insert(outbound_id, outbound.clone());
                        *commit_rollback_outbound
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(outbound);
                    }
                    Ok(())
                },
                move |state, baseline| {
                    if let Some(inserted) = rollback_outbound
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .as_ref()
                    {
                        if state.outbound.get(&inserted.id) == Some(inserted) {
                            state.outbound.remove(&inserted.id);
                        }
                    }
                    state
                        .rollback_agent_runtime(baseline)
                        .map(|_| ())
                        .map_err(ApiError::service_unavailable)
                },
            )
            .await
            .map_err(|_| ConnectorManagerError::Persistence)?;
        let delivery_queued = delivery_queued && run.result.status == "success";
        Ok((run, delivery_queued))
    }

    async fn expire_pending_pairing_at(
        &self,
        connector_id: &str,
        now: u64,
    ) -> Result<bool, ConnectorManagerError> {
        let stale = {
            let state = self.state.read().await;
            let connector = state
                .connectors
                .get(connector_id)
                .filter(|connector| connector.is_active())
                .ok_or(ConnectorManagerError::ConnectorNotFound)?;
            connector.pending_pairing.as_ref().is_some_and(|pending| {
                now.saturating_sub(pending.requested_at_ms) > PAIRING_CANDIDATE_TTL_MS
            })
        };
        if !stale {
            return Ok(false);
        }

        let _mutation = self.mutation_lock.lock().await;
        let transaction = {
            let mut state = self.state.write().await;
            let connector = state
                .connectors
                .get(connector_id)
                .filter(|connector| connector.is_active())
                .cloned()
                .ok_or(ConnectorManagerError::ConnectorNotFound)?;
            if !connector.pending_pairing.as_ref().is_some_and(|pending| {
                now.saturating_sub(pending.requested_at_ms) > PAIRING_CANDIDATE_TTL_MS
            }) {
                return Ok(false);
            }
            let target = state
                .connectors
                .get_mut(connector_id)
                .expect("connector was prevalidated");
            target.pending_pairing = None;
            target.updated_at_ms = now;
            Some((connector, state.control_plane_persist_request()))
        };
        let Some((previous, persist)) = transaction else {
            return Ok(false);
        };
        if persist.save().await.is_err() {
            self.state
                .write()
                .await
                .connectors
                .insert(connector_id.to_string(), previous);
            self.statuses
                .lock()
                .await
                .insert(connector_id.to_string(), ConnectorRuntimeStatus::Error);
            return Err(ConnectorManagerError::Persistence);
        }
        self.refresh_operational_status(connector_id).await;
        Ok(true)
    }

    pub(crate) async fn process_pending_once(
        &self,
        connector_id: String,
    ) -> Result<bool, ConnectorManagerError> {
        let manager = self.clone();
        tokio::spawn(async move { manager.process_pending_once_owned(connector_id).await })
            .await
            .map_err(|_| ConnectorManagerError::WorkerStopped)?
    }

    async fn process_pending_once_owned(
        &self,
        connector_id: String,
    ) -> Result<bool, ConnectorManagerError> {
        let inbound = {
            let _mutation = self.mutation_lock.lock().await;
            let (key, previous, persist) = {
                let mut state = self.state.write().await;
                let connector = state
                    .connectors
                    .get(&connector_id)
                    .filter(|connector| connector.is_active())
                    .ok_or(ConnectorManagerError::ConnectorNotFound)?;
                if connector.approved_chat.is_none() {
                    return Ok(false);
                }
                if state
                    .outbound
                    .values()
                    .filter(|record| {
                        record.connector_id == connector_id
                            && record.delivery_state != OutboundDeliveryState::Delivered
                    })
                    .count()
                    >= MAX_UNDELIVERED_OUTBOUND
                {
                    drop(state);
                    self.statuses
                        .lock()
                        .await
                        .insert(connector_id, ConnectorRuntimeStatus::Error);
                    return Ok(false);
                }
                let key = state
                    .inbound
                    .iter()
                    .filter(|((record_connector_id, _), record)| {
                        record_connector_id == &connector_id
                            && matches!(
                                record.processing_state,
                                InboundProcessingState::Received
                                    | InboundProcessingState::Processing
                            )
                    })
                    .min_by_key(|(_, record)| (record.received_at_ms, record.update_id))
                    .map(|(key, _)| key.clone());
                let Some(key) = key else {
                    return Ok(false);
                };
                let previous = state.inbound.get(&key).cloned().expect("key came from map");
                state
                    .inbound
                    .get_mut(&key)
                    .expect("key came from map")
                    .processing_state = InboundProcessingState::Processing;
                let persist = state.control_plane_persist_request();
                (key, previous, persist)
            };
            if persist.save().await.is_err() {
                self.state.write().await.inbound.insert(key, previous);
                self.statuses
                    .lock()
                    .await
                    .insert(connector_id, ConnectorRuntimeStatus::Error);
                return Err(ConnectorManagerError::Persistence);
            }
            self.state
                .read()
                .await
                .inbound
                .get(&key)
                .cloned()
                .expect("processing transition was persisted")
        };

        let key = (inbound.connector_id.clone(), inbound.update_id);
        let commit_connector_id = inbound.connector_id.clone();
        let commit_agent_id = inbound.agent_id.clone();
        let commit_room_id = inbound.room_id.clone();
        let commit_key = key.clone();
        let outbound_id = format!(
            "telegram:{}:update:{}:outbound",
            inbound.connector_id, inbound.update_id
        );
        let previous_outbound = {
            let state = self.state.read().await;
            state.outbound.get(&outbound_id).cloned()
        };
        let rollback_delta = Arc::new(std::sync::Mutex::new(ConnectorRunRollbackDelta::default()));
        let commit_rollback_delta = Arc::clone(&rollback_delta);
        let commit_outbound_id = outbound_id.clone();
        let rollback_target = inbound.clone();
        let rollback_key = key.clone();
        let rollback_outbound_id = outbound_id.clone();
        let request = AgentRunRequest {
            agent_id: inbound.agent_id.clone(),
            content: Content {
                text: inbound.normalized_text.clone(),
                metadata: Some(BTreeMap::from([
                    ("source".into(), DataValue::String("telegram".into())),
                    (
                        "connectorId".into(),
                        DataValue::String(inbound.connector_id.clone()),
                    ),
                    (
                        "updateId".into(),
                        DataValue::Number(inbound.update_id as f64),
                    ),
                ])),
                attachments: None,
            },
            room: RunRoom::Stable(inbound.room_id.clone()),
            idempotency_key: Some(inbound.run_idempotency_key.clone()),
        };

        let run = self
            .runs
            .run_with_commit_waiting(
                request,
                move |state, snapshot, result| {
                    let current = state
                        .inbound
                        .get(&commit_key)
                        .ok_or_else(|| ApiError::bad_request("durable inbound disappeared"))?;
                    if current.processing_state != InboundProcessingState::Processing
                        || current.agent_id != commit_agent_id
                        || current.room_id != commit_room_id
                    {
                        return Err(ApiError::bad_request("durable inbound changed during run"));
                    }

                    if result.status == TaskStatus::Error {
                        let target = state
                            .inbound
                            .get_mut(&commit_key)
                            .expect("inbound was prevalidated");
                        target.processing_state = InboundProcessingState::Rejected;
                        commit_rollback_delta
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .committed_target = Some(target.clone());
                        return Ok(());
                    }

                    let assistant = snapshot
                        .messages
                        .iter()
                        .rev()
                        .find(|message| {
                            message.room_id == commit_room_id
                                && message.role == MessageRole::Assistant
                        })
                        .cloned()
                        .ok_or_else(|| {
                            ApiError::bad_request("agent produced no assistant message")
                        })?;
                    let candidate = TelegramOutboundRecord {
                        id: commit_outbound_id.clone(),
                        connector_id: commit_connector_id.clone(),
                        agent_id: commit_agent_id.clone(),
                        room_id: commit_room_id.clone(),
                        assistant_message_id: assistant.id,
                        text: assistant.content.text,
                        created_at_ms: now_ms(),
                        delivered_at_ms: None,
                        attempts: 0,
                        delivery_state: OutboundDeliveryState::Pending,
                    };
                    if let Some(existing) = state.outbound.get(&commit_outbound_id) {
                        if existing.connector_id != candidate.connector_id
                            || existing.agent_id != candidate.agent_id
                            || existing.room_id != candidate.room_id
                            || existing.assistant_message_id != candidate.assistant_message_id
                            || existing.text != candidate.text
                        {
                            return Err(ApiError::bad_request(
                                "durable outbound conflicts with run",
                            ));
                        }
                    }

                    let target = state
                        .inbound
                        .get_mut(&commit_key)
                        .expect("inbound was prevalidated");
                    target.processing_state = InboundProcessingState::Processed;
                    let committed_target = target.clone();
                    let inserted_outbound = if state.outbound.contains_key(&commit_outbound_id) {
                        None
                    } else {
                        state
                            .outbound
                            .insert(commit_outbound_id.clone(), candidate.clone());
                        Some(candidate)
                    };
                    let removed_terminal =
                        compact_terminal_inbound(&mut state.inbound, &commit_connector_id);
                    let mut delta = commit_rollback_delta
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    delta.committed_target = Some(committed_target);
                    delta.inserted_outbound = inserted_outbound;
                    delta.removed_terminal = removed_terminal;
                    Ok(())
                },
                move |state, baseline| {
                    let delta = rollback_delta
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    for (removed_key, removed_record) in &delta.removed_terminal {
                        state
                            .inbound
                            .entry(removed_key.clone())
                            .or_insert_with(|| removed_record.clone());
                    }
                    if delta.committed_target.as_ref().is_some_and(|committed| {
                        state.inbound.get(&rollback_key) == Some(committed)
                    }) {
                        state.inbound.insert(rollback_key.clone(), rollback_target);
                    }
                    if delta.inserted_outbound.as_ref().is_some_and(|inserted| {
                        state.outbound.get(&rollback_outbound_id) == Some(inserted)
                    }) {
                        state.outbound.remove(&rollback_outbound_id);
                    } else if let Some(previous) = previous_outbound {
                        state
                            .outbound
                            .entry(rollback_outbound_id)
                            .or_insert(previous);
                    }
                    state
                        .rollback_agent_runtime(baseline)
                        .map(|_| ())
                        .map_err(ApiError::service_unavailable)
                },
            )
            .await;
        if run.is_err() {
            self.statuses
                .lock()
                .await
                .insert(connector_id, ConnectorRuntimeStatus::Error);
            return Err(ConnectorManagerError::Persistence);
        }
        Ok(true)
    }

    pub(crate) async fn deliver_pending_once(
        &self,
        connector_id: String,
    ) -> Result<bool, ConnectorManagerError> {
        let manager = self.clone();
        tokio::spawn(async move { manager.deliver_pending_once_owned(connector_id).await })
            .await
            .map_err(|_| ConnectorManagerError::WorkerStopped)?
    }

    async fn deliver_pending_once_owned(
        &self,
        connector_id: String,
    ) -> Result<bool, ConnectorManagerError> {
        let _lifecycle = self.lifecycle_lock.lock().await;
        self.ensure_open()?;
        let token = match self.credentials.load(&connector_id).await {
            Ok(Some(token)) => token,
            Ok(None) => {
                self.statuses
                    .lock()
                    .await
                    .insert(connector_id, ConnectorRuntimeStatus::CredentialRequired);
                return Err(ConnectorManagerError::Credential);
            }
            Err(error) => {
                let mapped = map_credential_error(error);
                self.statuses.lock().await.insert(
                    connector_id,
                    if mapped == ConnectorManagerError::CredentialStateUncertain {
                        ConnectorRuntimeStatus::Reconciling
                    } else {
                        ConnectorRuntimeStatus::Error
                    },
                );
                return Err(mapped);
            }
        };
        self.deliver_pending_once_with_token(connector_id, &token)
            .await
    }

    async fn deliver_pending_once_with_token(
        &self,
        connector_id: String,
        token: &TelegramBotToken,
    ) -> Result<bool, ConnectorManagerError> {
        let _mutation = self.mutation_lock.lock().await;
        let (chat_id, outbound) = {
            let state = self.state.read().await;
            let connector = state
                .connectors
                .get(&connector_id)
                .filter(|connector| connector.deleted_at_ms.is_none())
                .ok_or(ConnectorManagerError::ConnectorNotFound)?;
            let Some(chat) = connector.approved_chat.as_ref() else {
                return Ok(false);
            };
            let outbound = state
                .outbound
                .values()
                .filter(|record| {
                    record.connector_id == connector_id
                        && matches!(
                            record.delivery_state,
                            OutboundDeliveryState::Pending | OutboundDeliveryState::Failed
                        )
                })
                .min_by_key(|record| (record.created_at_ms, record.id.clone()))
                .cloned();
            let Some(outbound) = outbound else {
                return Ok(false);
            };
            (chat.id.clone(), outbound)
        };
        let delivery = self
            .transport
            .send_message(token, &chat_id, &outbound.text)
            .await;
        let (previous_connector_outbound, persist) = {
            let mut state = self.state.write().await;
            let previous_connector_outbound = state
                .outbound
                .iter()
                .filter(|(_, record)| record.connector_id == connector_id)
                .map(|(id, record)| (id.clone(), record.clone()))
                .collect::<Vec<_>>();
            let delivered_at_ms = now_ms();
            {
                let record = state
                    .outbound
                    .get_mut(&outbound.id)
                    .ok_or(ConnectorManagerError::ConnectorNotFound)?;
                record.attempts = record.attempts.saturating_add(1);
                match &delivery {
                    Ok(_) => {
                        record.delivery_state = OutboundDeliveryState::Delivered;
                        record.delivered_at_ms = Some(delivered_at_ms);
                    }
                    Err(_) => {
                        record.delivery_state = OutboundDeliveryState::Failed;
                        record.delivered_at_ms = None;
                    }
                }
            }
            if delivery.is_ok() {
                compact_delivered_outbox(&mut state.outbound, &connector_id, delivered_at_ms);
            }
            let persist = state.control_plane_persist_request();
            (previous_connector_outbound, persist)
        };
        if persist.save().await.is_err() {
            let mut state = self.state.write().await;
            state
                .outbound
                .retain(|_, record| record.connector_id != connector_id);
            state.outbound.extend(previous_connector_outbound);
            drop(state);
            self.statuses
                .lock()
                .await
                .insert(connector_id, ConnectorRuntimeStatus::Error);
            return Err(ConnectorManagerError::Persistence);
        }
        match delivery {
            Ok(_) => {
                self.refresh_operational_status(&connector_id).await;
                Ok(true)
            }
            Err(error) if is_revoked_credential(error) => {
                self.statuses
                    .lock()
                    .await
                    .insert(connector_id, ConnectorRuntimeStatus::CredentialRequired);
                Err(ConnectorManagerError::Credential)
            }
            Err(_) => {
                self.statuses
                    .lock()
                    .await
                    .insert(connector_id, ConnectorRuntimeStatus::Error);
                Err(ConnectorManagerError::Transport)
            }
        }
    }

    pub(crate) async fn delete(&self, connector_id: String) -> Result<(), ConnectorManagerError> {
        let manager = self.clone();
        tokio::spawn(async move { manager.delete_owned(connector_id).await })
            .await
            .map_err(|_| ConnectorManagerError::WorkerStopped)?
    }

    pub(crate) async fn delete_agent(&self, agent_id: String) -> Result<(), ConnectorManagerError> {
        let manager = self.clone();
        tokio::spawn(async move {
            let _lifecycle = manager.lifecycle_lock.lock().await;
            manager.ensure_open()?;
            if manager.state.read().await.get_agent(&agent_id).is_none() {
                return Err(ConnectorManagerError::AgentNotFound);
            }
            let connectors = manager
                .state
                .read()
                .await
                .connectors
                .values()
                .filter(|connector| {
                    connector.agent_id == agent_id && connector.deleted_at_ms.is_none()
                })
                .cloned()
                .collect::<Vec<_>>();
            let mut previous = Vec::with_capacity(connectors.len());
            for connector in connectors {
                let token = manager
                    .credentials
                    .load(&connector.id)
                    .await
                    .map_err(map_credential_error)?;
                let status = manager
                    .statuses
                    .lock()
                    .await
                    .get(&connector.id)
                    .copied()
                    .unwrap_or_else(|| connector_operational_status(&connector));
                previous.push((connector, token, status));
            }
            for (connector, _, _) in &previous {
                manager.stop_worker(&connector.id).await?;
            }
            for (connector, _, _) in &previous {
                if let Err(error) = manager.credentials.delete(&connector.id).await {
                    let mapped = map_credential_error(error);
                    manager.restore_agent_delete_configuration(&previous).await?;
                    return Err(mapped);
                }
            }

            let _transaction = manager.mutation_lock.lock().await;
            let (agent_snapshot, previous_connectors, previous_inbound, previous_outbound, previous_schedules, persist) = {
                let mut state = manager.state.write().await;
                let agent_snapshot = state.get_agent(&agent_id);
                let previous_connectors = previous
                    .iter()
                    .filter_map(|(connector, _, _)| {
                        state.connectors.get(&connector.id).cloned().map(|record| (record.id.clone(), record))
                    })
                    .collect::<Vec<_>>();
                let previous_inbound = state.inbound.clone();
                let previous_outbound = state.outbound.clone();
                let previous_schedules = state.schedules.clone();
                let now = now_ms();
                for (connector, _, _) in &previous {
                    if let Some(record) = state.connectors.get_mut(&connector.id) {
                        record.enabled = false;
                        record.deleted_at_ms = Some(now);
                        record.pending_pairing = None;
                        record.updated_at_ms = now;
                    }
                    state.inbound.retain(|(record_connector_id, _), record| {
                        record_connector_id != &connector.id
                            || matches!(
                                record.processing_state,
                                InboundProcessingState::Processed
                                    | InboundProcessingState::Rejected
                            )
                    });
                    state.outbound.retain(|_, record| {
                        record.connector_id != connector.id
                            || record.delivery_state == OutboundDeliveryState::Delivered
                    });
                    compact_delivered_outbox(&mut state.outbound, &connector.id, now);
                    for schedule in state.schedules.values_mut() {
                        if matches!(
                            &schedule.target,
                            ScheduleTarget::Connector { connector_id } if connector_id == &connector.id
                        ) {
                            schedule.enabled = false;
                            schedule.updated_at_ms = now;
                        }
                    }
                }
                state.schedules.retain(|_, schedule| schedule.agent_id != agent_id);
                state.remove_agent(&agent_id);
                let persist = state.control_plane_persist_request();
                (
                    agent_snapshot,
                    previous_connectors,
                    previous_inbound,
                    previous_outbound,
                    previous_schedules,
                    persist,
                )
            };

            if persist.save().await.is_err() {
                {
                    let mut state = manager.state.write().await;
                    for (connector_id, connector) in previous_connectors {
                        state.connectors.insert(connector_id, connector);
                    }
                    state.inbound = previous_inbound;
                    state.outbound = previous_outbound;
                    state.schedules = previous_schedules;
                    if let Some(agent_snapshot) = agent_snapshot {
                        state
                            .restore_removed_agent(agent_snapshot)
                            .map_err(|_| ConnectorManagerError::Persistence)?;
                    }
                }
                manager.restore_agent_delete_configuration(&previous).await?;
                drop(_transaction);
                return Err(ConnectorManagerError::Persistence);
            }
            for (connector, _, _) in &previous {
                manager.statuses.lock().await.remove(&connector.id);
            }
            Ok(())
        })
        .await
        .map_err(|_| ConnectorManagerError::WorkerStopped)?
    }

    async fn restore_agent_delete_configuration(
        &self,
        previous: &[(
            TelegramConnectorRecord,
            Option<TelegramBotToken>,
            ConnectorRuntimeStatus,
        )],
    ) -> Result<(), ConnectorManagerError> {
        for (connector, token, _) in previous {
            let result = match token {
                Some(token) => self.credentials.put(&connector.id, token.clone()).await,
                None => self.credentials.delete(&connector.id).await,
            };
            if result.is_err() {
                self.statuses
                    .lock()
                    .await
                    .insert(connector.id.clone(), ConnectorRuntimeStatus::Reconciling);
                return Err(ConnectorManagerError::CredentialStateUncertain);
            }
        }
        for (connector, token, status) in previous {
            if let Some(token) = token {
                self.start_worker(connector.id.clone(), token.clone())
                    .await?;
                self.statuses
                    .lock()
                    .await
                    .insert(connector.id.clone(), *status);
            } else {
                self.statuses.lock().await.insert(
                    connector.id.clone(),
                    ConnectorRuntimeStatus::CredentialRequired,
                );
            }
        }
        Ok(())
    }

    async fn delete_owned(&self, connector_id: String) -> Result<(), ConnectorManagerError> {
        let _lifecycle = self.lifecycle_lock.lock().await;
        self.ensure_open()?;
        self.delete_unlocked(connector_id).await
    }

    async fn delete_unlocked(&self, connector_id: String) -> Result<(), ConnectorManagerError> {
        let connector = {
            let state = self.state.read().await;
            state
                .connectors
                .get(&connector_id)
                .filter(|connector| connector.deleted_at_ms.is_none())
                .cloned()
                .ok_or(ConnectorManagerError::ConnectorNotFound)?
        };
        let previous_status = self
            .statuses
            .lock()
            .await
            .get(&connector_id)
            .copied()
            .unwrap_or_else(|| connector_operational_status(&connector));
        let previous_token = match self.credentials.load(&connector_id).await {
            Ok(token) => token,
            Err(error) => {
                let mapped = map_credential_error(error);
                if mapped == ConnectorManagerError::CredentialStateUncertain {
                    self.stop_worker(&connector_id).await?;
                    self.statuses
                        .lock()
                        .await
                        .insert(connector_id, ConnectorRuntimeStatus::Reconciling);
                }
                return Err(mapped);
            }
        };
        self.stop_worker(&connector_id).await?;
        let _mutation = self.mutation_lock.lock().await;

        if let Err(error) = self.credentials.delete(&connector_id).await {
            let mapped = map_credential_error(error);
            if mapped == ConnectorManagerError::CredentialStateUncertain {
                self.statuses
                    .lock()
                    .await
                    .insert(connector_id, ConnectorRuntimeStatus::Reconciling);
            } else {
                drop(_mutation);
                if let Some(previous_token) = previous_token {
                    self.start_worker(connector_id.clone(), previous_token)
                        .await?;
                    self.statuses
                        .lock()
                        .await
                        .insert(connector_id, previous_status);
                } else {
                    self.statuses
                        .lock()
                        .await
                        .insert(connector_id, ConnectorRuntimeStatus::CredentialRequired);
                }
            }
            return Err(mapped);
        }

        let (previous_connector, previous_inbound, previous_outbound, previous_schedules, persist) = {
            let mut state = self.state.write().await;
            let previous_connector = state.connectors.get(&connector_id).cloned();
            let previous_inbound = state.inbound.clone();
            let previous_outbound = state.outbound.clone();
            let previous_schedules = state.schedules.clone();
            let now = now_ms();
            let connector = state
                .connectors
                .get_mut(&connector_id)
                .expect("active connector was prevalidated");
            connector.enabled = false;
            connector.deleted_at_ms = Some(now);
            connector.pending_pairing = None;
            connector.updated_at_ms = now;
            state.inbound.retain(|(record_connector_id, _), record| {
                record_connector_id != &connector_id
                    || matches!(
                        record.processing_state,
                        InboundProcessingState::Processed | InboundProcessingState::Rejected
                    )
            });
            state.outbound.retain(|_, record| {
                record.connector_id != connector_id
                    || record.delivery_state == OutboundDeliveryState::Delivered
            });
            compact_delivered_outbox(&mut state.outbound, &connector_id, now);
            for schedule in state.schedules.values_mut() {
                if matches!(
                    &schedule.target,
                    ScheduleTarget::Connector { connector_id: target } if target == &connector_id
                ) {
                    schedule.enabled = false;
                    schedule.updated_at_ms = now;
                    schedule.last_safe_outcome = Some(ScheduleSafeOutcome {
                        status: ScheduleOutcomeStatus::Failed,
                        occurred_at_ms: now,
                        error_code: Some("schedule_target_unavailable".into()),
                    });
                }
            }
            let persist = state.control_plane_persist_request();
            (
                previous_connector,
                previous_inbound,
                previous_outbound,
                previous_schedules,
                persist,
            )
        };

        if persist.save().await.is_err() {
            let mut state = self.state.write().await;
            if let Some(connector) = previous_connector {
                state.connectors.insert(connector_id.clone(), connector);
            }
            state.inbound = previous_inbound;
            state.outbound = previous_outbound;
            state.schedules = previous_schedules;
            drop(state);
            let Some(previous_token) = previous_token else {
                self.statuses
                    .lock()
                    .await
                    .insert(connector_id, ConnectorRuntimeStatus::CredentialRequired);
                return Err(ConnectorManagerError::Persistence);
            };
            match self
                .credentials
                .put(&connector_id, previous_token.clone())
                .await
            {
                Ok(()) => {
                    drop(_mutation);
                    self.start_worker(connector_id.clone(), previous_token)
                        .await?;
                    self.statuses
                        .lock()
                        .await
                        .insert(connector_id, previous_status);
                    return Err(ConnectorManagerError::Persistence);
                }
                Err(_) => {
                    self.statuses
                        .lock()
                        .await
                        .insert(connector_id, ConnectorRuntimeStatus::Reconciling);
                    return Err(ConnectorManagerError::CredentialStateUncertain);
                }
            }
        }
        self.statuses.lock().await.remove(&connector_id);
        Ok(())
    }

    pub(crate) async fn restart(&self, connector_id: String) -> Result<(), ConnectorManagerError> {
        let manager = self.clone();
        tokio::spawn(async move { manager.restart_owned(connector_id).await })
            .await
            .map_err(|_| ConnectorManagerError::WorkerStopped)?
    }

    async fn restart_owned(&self, connector_id: String) -> Result<(), ConnectorManagerError> {
        let _lifecycle = self.lifecycle_lock.lock().await;
        self.ensure_open()?;
        self.restart_unlocked(connector_id).await
    }

    async fn restart_unlocked(&self, connector_id: String) -> Result<(), ConnectorManagerError> {
        self.stop_worker(&connector_id).await?;
        let connector = self
            .state
            .read()
            .await
            .connectors
            .get(&connector_id)
            .filter(|connector| connector.is_active())
            .cloned()
            .ok_or(ConnectorManagerError::ConnectorNotFound)?;
        let token = match self.credentials.load(&connector_id).await {
            Ok(Some(token)) => token,
            Ok(None) => {
                self.statuses
                    .lock()
                    .await
                    .insert(connector_id, ConnectorRuntimeStatus::CredentialRequired);
                return Ok(());
            }
            Err(CredentialStoreError::CredentialStateUncertain) => {
                self.statuses
                    .lock()
                    .await
                    .insert(connector_id, ConnectorRuntimeStatus::Reconciling);
                return Ok(());
            }
            Err(_) => {
                self.statuses
                    .lock()
                    .await
                    .insert(connector_id, ConnectorRuntimeStatus::Error);
                return Ok(());
            }
        };
        let status = if connector.approved_chat.is_some() {
            ConnectorRuntimeStatus::Ready
        } else {
            ConnectorRuntimeStatus::Pairing
        };
        self.statuses
            .lock()
            .await
            .insert(connector_id.clone(), status);
        self.start_worker(connector_id, token).await
    }

    pub(crate) async fn start_restored(&self) {
        let _lifecycle = self.lifecycle_lock.lock().await;
        if self.ensure_open().is_err() {
            return;
        }
        self.cleanup_restored_credentials().await;
        let connectors = self
            .state
            .read()
            .await
            .connectors
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for connector in connectors {
            if connector.deleted_at_ms.is_some() {
                match self.credentials.delete(&connector.id).await {
                    Ok(()) => {
                        self.statuses.lock().await.remove(&connector.id);
                    }
                    Err(_) => {
                        self.statuses
                            .lock()
                            .await
                            .insert(connector.id, ConnectorRuntimeStatus::Reconciling);
                    }
                }
                continue;
            }
            if connector.is_active() {
                let _ = self.restart_unlocked(connector.id).await;
            } else {
                self.statuses
                    .lock()
                    .await
                    .insert(connector.id, ConnectorRuntimeStatus::CredentialRequired);
            }
        }
    }

    async fn cleanup_restored_credentials(&self) {
        let connector_ids = self
            .state
            .read()
            .await
            .credential_cleanup
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for connector_id in connector_ids {
            if self.credentials.delete(&connector_id).await.is_err() {
                self.statuses
                    .lock()
                    .await
                    .insert(connector_id, ConnectorRuntimeStatus::Reconciling);
                continue;
            }

            let _mutation = self.mutation_lock.lock().await;
            let (intent, persist) = {
                let mut state = self.state.write().await;
                let intent = state.credential_cleanup.remove(&connector_id);
                (intent, state.control_plane_persist_request())
            };
            if persist.save().await.is_err() {
                if let Some(intent) = intent {
                    self.state
                        .write()
                        .await
                        .credential_cleanup
                        .insert(connector_id.clone(), intent);
                }
                self.statuses
                    .lock()
                    .await
                    .insert(connector_id, ConnectorRuntimeStatus::Reconciling);
            } else {
                self.statuses.lock().await.remove(&connector_id);
            }
        }
    }

    async fn start_worker(
        &self,
        connector_id: String,
        token: TelegramBotToken,
    ) -> Result<(), ConnectorManagerError> {
        self.stop_worker(&connector_id).await?;
        let (cancel, receiver) = watch::channel(false);
        let owner_close = self
            .owner_close
            .as_ref()
            .expect("only public manager owners start workers")
            .subscribe();
        let (start, started) = oneshot::channel();
        let generation = self.worker_generation.fetch_add(1, Ordering::Relaxed);
        let manager = self.detached_worker_context();
        let worker_registry = Arc::downgrade(&self.workers);
        let worker_connector_id = connector_id.clone();
        let join = tokio::spawn(async move {
            if started.await.is_ok() {
                manager
                    .worker_loop(worker_connector_id.clone(), token, receiver, owner_close)
                    .await;
                if let Some(worker_registry) = worker_registry.upgrade() {
                    let mut workers = worker_registry.lock().await;
                    if workers
                        .get(&worker_connector_id)
                        .is_some_and(|handle| handle.generation == generation)
                    {
                        workers.remove(&worker_connector_id);
                    }
                }
            }
        });
        self.workers.lock().await.insert(
            connector_id,
            WorkerHandle {
                generation,
                cancel,
                join,
            },
        );
        let _ = start.send(());
        Ok(())
    }

    fn detached_worker_context(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
            runs: self.runs.clone(),
            credentials: Arc::clone(&self.credentials),
            transport: Arc::clone(&self.transport),
            lifecycle_lock: Arc::new(Mutex::new(())),
            mutation_lock: Arc::clone(&self.mutation_lock),
            statuses: Arc::clone(&self.statuses),
            workers: Arc::new(Mutex::new(HashMap::new())),
            owner_send_locks: Arc::clone(&self.owner_send_locks),
            worker_generation: Arc::clone(&self.worker_generation),
            closing: Arc::clone(&self.closing),
            owner_close: None,
        }
    }

    async fn stop_worker(&self, connector_id: &str) -> Result<(), ConnectorManagerError> {
        let handle = self.workers.lock().await.remove(connector_id);
        let Some(handle) = handle else {
            return Ok(());
        };
        let _ = handle.cancel.send(true);
        handle
            .join
            .await
            .map_err(|_| ConnectorManagerError::WorkerStopped)
    }

    async fn worker_loop(
        &self,
        connector_id: String,
        token: TelegramBotToken,
        mut cancel: watch::Receiver<bool>,
        mut owner_close: watch::Receiver<bool>,
    ) {
        let mut poll_backoff = POLL_RETRY_INITIAL;
        'poll: loop {
            if receiver_stopped(&cancel) || receiver_stopped(&owner_close) {
                return;
            }
            match self
                .expire_pending_pairing_at(&connector_id, now_ms())
                .await
            {
                Ok(_) => {}
                Err(ConnectorManagerError::ConnectorNotFound) => return,
                Err(_) => {
                    if wait_or_stop(&mut cancel, &mut owner_close, Duration::from_millis(100)).await
                    {
                        return;
                    }
                    continue;
                }
            }
            let offset = match self.state.read().await.connectors.get(&connector_id) {
                Some(connector) if connector.is_active() => connector.next_update_id,
                _ => return,
            };
            let batch = tokio::select! {
                changed = cancel.changed() => {
                    let _ = changed;
                    return;
                }
                changed = owner_close.changed() => {
                    let _ = changed;
                    return;
                }
                result = self.transport.get_updates(&token, offset) => result,
            };
            let batch = match batch {
                Ok(batch) => {
                    poll_backoff = POLL_RETRY_INITIAL;
                    batch
                }
                Err(error) if is_revoked_credential(error) => {
                    self.statuses
                        .lock()
                        .await
                        .insert(connector_id, ConnectorRuntimeStatus::CredentialRequired);
                    return;
                }
                Err(_) => {
                    self.statuses
                        .lock()
                        .await
                        .insert(connector_id.clone(), ConnectorRuntimeStatus::Error);
                    if wait_or_stop(&mut cancel, &mut owner_close, poll_backoff).await {
                        return;
                    }
                    poll_backoff = next_poll_backoff(poll_backoff);
                    continue;
                }
            };
            let had_updates = !batch.updates.is_empty();
            if had_updates || batch.next_update_id != offset {
                match self.accept_batch(connector_id.clone(), batch).await {
                    Ok(()) => {}
                    Err(ConnectorManagerError::Backpressure) => {
                        loop {
                            match self
                                .deliver_pending_once_with_token(connector_id.clone(), &token)
                                .await
                            {
                                Ok(true) => continue,
                                Ok(false) => break,
                                Err(ConnectorManagerError::Transport) => {
                                    self.statuses.lock().await.insert(
                                        connector_id.clone(),
                                        ConnectorRuntimeStatus::Degraded,
                                    );
                                    if wait_or_stop(
                                        &mut cancel,
                                        &mut owner_close,
                                        Duration::from_millis(100),
                                    )
                                    .await
                                    {
                                        return;
                                    }
                                    continue 'poll;
                                }
                                Err(_) => return,
                            }
                        }
                        match self.process_pending_once(connector_id.clone()).await {
                            Ok(true) | Ok(false) => {}
                            Err(_) => return,
                        }
                        self.statuses
                            .lock()
                            .await
                            .insert(connector_id.clone(), ConnectorRuntimeStatus::Degraded);
                        if wait_or_stop(&mut cancel, &mut owner_close, Duration::from_millis(100))
                            .await
                        {
                            return;
                        }
                        continue;
                    }
                    Err(_) => {
                        self.statuses
                            .lock()
                            .await
                            .insert(connector_id, ConnectorRuntimeStatus::Error);
                        return;
                    }
                }
            }

            self.refresh_operational_status(&connector_id).await;

            let pending_chat = self
                .state
                .read()
                .await
                .connectors
                .get(&connector_id)
                .and_then(|connector| {
                    if connector.approved_chat.is_none() {
                        connector
                            .pending_pairing
                            .as_ref()
                            .map(|pairing| pairing.chat.id.clone())
                    } else {
                        None
                    }
                });
            if had_updates {
                if let Some(chat_id) = pending_chat {
                    let _ = self
                        .transport
                        .send_message(
                            &token,
                            &chat_id,
                            "This chat is waiting for approval in the AnimaOS web console.",
                        )
                        .await;
                }
            }

            loop {
                match self.process_pending_once(connector_id.clone()).await {
                    Ok(true) => continue,
                    Ok(false) => break,
                    Err(_) => return,
                }
            }
            loop {
                match self
                    .deliver_pending_once_with_token(connector_id.clone(), &token)
                    .await
                {
                    Ok(true) => continue,
                    Ok(false) => break,
                    Err(ConnectorManagerError::Transport) => break,
                    Err(_) => return,
                }
            }
            if !had_updates
                && wait_or_stop(&mut cancel, &mut owner_close, Duration::from_millis(100)).await
            {
                return;
            }
        }
    }

    pub(crate) async fn status(&self, connector_id: &str) -> Option<ConnectorRuntimeStatus> {
        self.statuses.lock().await.get(connector_id).copied()
    }

    async fn refresh_operational_status(&self, connector_id: &str) {
        let status = {
            let state = self.state.read().await;
            state.connectors.get(connector_id).map(|connector| {
                if state.outbound.values().any(|record| {
                    record.connector_id == connector_id
                        && record.delivery_state == OutboundDeliveryState::Failed
                }) {
                    ConnectorRuntimeStatus::Error
                } else {
                    connector_operational_status(connector)
                }
            })
        };
        if let Some(status) = status {
            self.statuses
                .lock()
                .await
                .insert(connector_id.to_string(), status);
        }
    }

    pub(crate) async fn worker_count(&self) -> usize {
        self.workers.lock().await.len()
    }

    pub(crate) async fn shutdown(&self) {
        self.closing.store(true, Ordering::SeqCst);
        if let Some(owner_close) = &self.owner_close {
            let _ = owner_close.send(true);
        }
        let _lifecycle = self.lifecycle_lock.lock().await;
        let connector_ids = self
            .workers
            .lock()
            .await
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for connector_id in connector_ids {
            let _ = self.stop_worker(&connector_id).await;
        }
    }
}

fn owner_send_replay(
    state: &DaemonState,
    connector: &TelegramConnectorRecord,
    text: &str,
    idempotency_key: &str,
) -> Result<Option<(AgentRunEnvelope, bool)>, ConnectorManagerError> {
    let snapshot = state
        .get_agent(&connector.agent_id)
        .ok_or(ConnectorManagerError::AgentNotFound)?;
    let Some((user_index, user)) = snapshot.messages.iter().enumerate().find(|(_, message)| {
        message.room_id == connector.room_id
            && message.role == MessageRole::User
            && message.content.metadata.as_ref().is_some_and(|metadata| {
                matches!(
                    metadata.get("idempotencyKey"),
                    Some(DataValue::String(value)) if value == idempotency_key
                ) && matches!(
                    metadata.get("connectorId"),
                    Some(DataValue::String(value)) if value == &connector.id
                )
            })
    }) else {
        return Ok(None);
    };
    if user.content.text != text {
        return Err(ConnectorManagerError::IdempotencyConflict);
    }
    let Some(assistant) = snapshot
        .messages
        .iter()
        .skip(user_index + 1)
        .find(|message| {
            message.room_id == connector.room_id && message.role == MessageRole::Assistant
        })
    else {
        return Ok(None);
    };
    let delivery_queued = state.outbound.values().any(|outbound| {
        outbound.connector_id == connector.id && outbound.assistant_message_id == assistant.id
    });
    let result = TaskResult::success(assistant.content.clone(), 0);
    Ok(Some((
        AgentRunEnvelope {
            agent: AgentRuntimeSnapshotResponse::from(&snapshot),
            result: TaskResultResponse::from(&result),
        },
        delivery_queued,
    )))
}

fn map_credential_error(error: CredentialStoreError) -> ConnectorManagerError {
    match error {
        CredentialStoreError::CredentialStateUncertain => {
            ConnectorManagerError::CredentialStateUncertain
        }
        _ => ConnectorManagerError::Credential,
    }
}

fn map_token_validation_error(error: TelegramTransportError) -> ConnectorManagerError {
    if is_invalid_token(error) {
        ConnectorManagerError::InvalidToken
    } else {
        ConnectorManagerError::Transport
    }
}

fn is_invalid_token(error: TelegramTransportError) -> bool {
    matches!(
        error,
        TelegramTransportError::HttpStatus {
            status: 400 | 401 | 404
        } | TelegramTransportError::UpstreamApi {
            code: Some(400 | 401 | 404)
        }
    )
}

fn is_revoked_credential(error: TelegramTransportError) -> bool {
    matches!(
        error,
        TelegramTransportError::HttpStatus { status: 401 }
            | TelegramTransportError::UpstreamApi { code: Some(401) }
    )
}

fn next_poll_backoff(current: Duration) -> Duration {
    current.saturating_mul(2).min(POLL_RETRY_MAX)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn next_connector_id(now: u64) -> String {
    let sequence = CONNECTOR_ID_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("telegram-{now}-{sequence}")
}

fn same_inbound_identity(
    existing: &TelegramInboundRecord,
    candidate: &TelegramInboundRecord,
) -> bool {
    existing.connector_id == candidate.connector_id
        && existing.update_id == candidate.update_id
        && existing.agent_id == candidate.agent_id
        && existing.room_id == candidate.room_id
        && existing.normalized_text == candidate.normalized_text
        && existing.sender == candidate.sender
        && existing.chat == candidate.chat
        && existing.run_idempotency_key == candidate.run_idempotency_key
}

fn connector_operational_status(connector: &TelegramConnectorRecord) -> ConnectorRuntimeStatus {
    if connector.approved_chat.is_some() {
        ConnectorRuntimeStatus::Ready
    } else {
        ConnectorRuntimeStatus::Pairing
    }
}

fn compact_delivered_outbox(
    outbox: &mut HashMap<String, TelegramOutboundRecord>,
    connector_id: &str,
    now: u64,
) {
    let cutoff = now.saturating_sub(DELIVERED_RETENTION_MS);
    outbox.retain(|_, record| {
        record.connector_id != connector_id
            || record.delivery_state != OutboundDeliveryState::Delivered
            || record
                .delivered_at_ms
                .is_none_or(|delivered_at_ms| delivered_at_ms >= cutoff)
    });
    let mut delivered = outbox
        .values()
        .filter(|record| {
            record.connector_id == connector_id
                && record.delivery_state == OutboundDeliveryState::Delivered
        })
        .map(|record| {
            (
                record.delivered_at_ms.unwrap_or(record.created_at_ms),
                record.id.clone(),
            )
        })
        .collect::<Vec<_>>();
    let excess = delivered.len().saturating_sub(MAX_RETAINED_DELIVERED);
    if excess == 0 {
        return;
    }
    delivered.sort();
    for (_, id) in delivered.into_iter().take(excess) {
        outbox.remove(&id);
    }
}

fn compact_terminal_inbound(
    inbound: &mut HashMap<(String, i64), TelegramInboundRecord>,
    connector_id: &str,
) -> Vec<((String, i64), TelegramInboundRecord)> {
    let mut terminal = inbound
        .iter()
        .filter(|((record_connector_id, _), record)| {
            record_connector_id == connector_id
                && matches!(
                    record.processing_state,
                    InboundProcessingState::Processed | InboundProcessingState::Rejected
                )
        })
        .map(|(key, record)| (record.received_at_ms, record.update_id, key.clone()))
        .collect::<Vec<_>>();
    let excess = terminal.len().saturating_sub(MAX_RETAINED_TERMINAL_INBOUND);
    if excess == 0 {
        return Vec::new();
    }
    terminal.sort();
    let mut removed = Vec::with_capacity(excess);
    for (_, _, key) in terminal.into_iter().take(excess) {
        if let Some(record) = inbound.remove(&key) {
            removed.push((key, record));
        }
    }
    removed
}

fn receiver_stopped(receiver: &watch::Receiver<bool>) -> bool {
    *receiver.borrow() || receiver.has_changed().is_err()
}

async fn wait_or_stop(
    cancel: &mut watch::Receiver<bool>,
    owner_close: &mut watch::Receiver<bool>,
    duration: Duration,
) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(duration) => false,
        changed = cancel.changed() => {
            let _ = changed;
            true
        }
        changed = owner_close.changed() => {
            let _ = changed;
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use anima_core::{
        AgentConfig, AgentConfigUpdate, AgentSettings, Content, DataValue, ModelAdapter,
        ModelGenerateRequest, ModelGenerateResponse, ModelStopReason, TokenUsage,
    };
    use async_trait::async_trait;
    use tokio::sync::{RwLock, Semaphore};

    use super::{ConnectorManager, ConnectorRuntimeStatus, TelegramTransport};
    use crate::agent_runs::{AgentRunCoordinator, AgentRunRequest, RunRoom};
    use crate::app::SharedDaemonState;
    use crate::connectors::credentials::{
        ConnectorCredentialStore, CredentialStoreError, InMemoryCredentialStore, TelegramBotToken,
    };
    use crate::connectors::telegram::{
        TelegramSentMessage, TelegramTextUpdate, TelegramTransportError, TelegramUpdateBatch,
    };
    use crate::connectors::{
        InboundProcessingState, TelegramBotIdentity, TelegramChatKind, TelegramChatMetadata,
        TelegramSenderMetadata,
    };
    use crate::state::DaemonState;

    #[derive(Default)]
    struct FakeTransport {
        calls: Mutex<Vec<&'static str>>,
        sent: Mutex<Vec<(String, String)>>,
        poll_calls: AtomicUsize,
        remaining_poll_failures: AtomicUsize,
        remaining_send_failures: AtomicUsize,
        revoked_poll: AtomicBool,
        revoked_send: AtomicBool,
    }

    #[derive(Default)]
    struct UncertainDeleteStore {
        inner: InMemoryCredentialStore,
    }

    #[derive(Default)]
    struct UncertainLoadStore;

    #[derive(Default)]
    struct FailingCredentialStore {
        inner: InMemoryCredentialStore,
        fail_next_load: AtomicBool,
        fail_next_put: AtomicBool,
    }

    struct GateLoadCredentialStore {
        inner: InMemoryCredentialStore,
        gate_load: AtomicBool,
        entered: Arc<Semaphore>,
        release: Arc<Semaphore>,
    }

    struct GatePutCredentialStore {
        inner: InMemoryCredentialStore,
        gate_put: AtomicBool,
        fail_put: AtomicBool,
        entered: Arc<Semaphore>,
        release: Arc<Semaphore>,
    }

    struct GatePutUncertainDeleteStore {
        inner: InMemoryCredentialStore,
        entered: Arc<Semaphore>,
        release: Arc<Semaphore>,
        fail_next_delete: AtomicBool,
    }

    struct GateModelAdapter {
        entered: Arc<Semaphore>,
        release: Arc<Semaphore>,
    }

    struct DropAwarePollingTransport {
        entered: Arc<Semaphore>,
        exited: Arc<Semaphore>,
    }

    struct ReplacementRaceCredentialStore {
        inner: InMemoryCredentialStore,
        fixed_worker_delivery: Arc<AtomicBool>,
        worker_load_entered: Arc<Semaphore>,
        replacement_put_done: Arc<Semaphore>,
        load_calls: AtomicUsize,
        put_calls: AtomicUsize,
    }

    struct ReplacementRaceTransport {
        fixed_worker_delivery: Arc<AtomicBool>,
        send_entered: Arc<Semaphore>,
        release_send: Arc<Semaphore>,
        gate_first_send: AtomicBool,
        sent_tokens: Mutex<Vec<String>>,
    }

    struct InboundBackpressureTransport {
        enabled: AtomicBool,
        fail_delivery: AtomicBool,
        delivery_attempts: Arc<Semaphore>,
        update: TelegramTextUpdate,
    }

    struct PollDropGuard(Arc<Semaphore>);

    impl Drop for PollDropGuard {
        fn drop(&mut self) {
            self.0.add_permits(1);
        }
    }

    #[async_trait]
    impl ModelAdapter for GateModelAdapter {
        fn provider(&self) -> &str {
            "gate"
        }

        async fn generate(
            &self,
            _config: &AgentConfig,
            _request: &ModelGenerateRequest,
        ) -> Result<ModelGenerateResponse, String> {
            self.entered.add_permits(1);
            self.release
                .acquire()
                .await
                .expect("release semaphore should remain open")
                .forget();
            Ok(ModelGenerateResponse {
                content: Content {
                    text: "gated response".into(),
                    ..Content::default()
                },
                tool_calls: None,
                usage: TokenUsage::default(),
                stop_reason: ModelStopReason::End,
            })
        }
    }

    #[async_trait]
    impl ConnectorCredentialStore for UncertainDeleteStore {
        async fn load(
            &self,
            connector_id: &str,
        ) -> Result<Option<TelegramBotToken>, CredentialStoreError> {
            self.inner.load(connector_id).await
        }

        async fn put(
            &self,
            connector_id: &str,
            token: TelegramBotToken,
        ) -> Result<(), CredentialStoreError> {
            self.inner.put(connector_id, token).await
        }

        async fn delete(&self, _connector_id: &str) -> Result<(), CredentialStoreError> {
            Err(CredentialStoreError::CredentialStateUncertain)
        }
    }

    #[async_trait]
    impl ConnectorCredentialStore for UncertainLoadStore {
        async fn load(
            &self,
            _connector_id: &str,
        ) -> Result<Option<TelegramBotToken>, CredentialStoreError> {
            Err(CredentialStoreError::CredentialStateUncertain)
        }

        async fn put(
            &self,
            _connector_id: &str,
            _token: TelegramBotToken,
        ) -> Result<(), CredentialStoreError> {
            Ok(())
        }

        async fn delete(&self, _connector_id: &str) -> Result<(), CredentialStoreError> {
            Ok(())
        }
    }

    #[async_trait]
    impl ConnectorCredentialStore for FailingCredentialStore {
        async fn load(
            &self,
            connector_id: &str,
        ) -> Result<Option<TelegramBotToken>, CredentialStoreError> {
            if self.fail_next_load.swap(false, Ordering::SeqCst) {
                return Err(CredentialStoreError::BackendUnavailable);
            }
            self.inner.load(connector_id).await
        }

        async fn put(
            &self,
            connector_id: &str,
            token: TelegramBotToken,
        ) -> Result<(), CredentialStoreError> {
            if self.fail_next_put.swap(false, Ordering::SeqCst) {
                return Err(CredentialStoreError::BackendUnavailable);
            }
            self.inner.put(connector_id, token).await
        }

        async fn delete(&self, connector_id: &str) -> Result<(), CredentialStoreError> {
            self.inner.delete(connector_id).await
        }
    }

    #[async_trait]
    impl ConnectorCredentialStore for GateLoadCredentialStore {
        async fn load(
            &self,
            connector_id: &str,
        ) -> Result<Option<TelegramBotToken>, CredentialStoreError> {
            if self.gate_load.load(Ordering::SeqCst) {
                self.entered.add_permits(1);
                self.release
                    .acquire()
                    .await
                    .map_err(|_| CredentialStoreError::OperationCancelled)?
                    .forget();
            }
            self.inner.load(connector_id).await
        }

        async fn put(
            &self,
            connector_id: &str,
            token: TelegramBotToken,
        ) -> Result<(), CredentialStoreError> {
            self.inner.put(connector_id, token).await
        }

        async fn delete(&self, connector_id: &str) -> Result<(), CredentialStoreError> {
            self.inner.delete(connector_id).await
        }
    }

    #[async_trait]
    impl ConnectorCredentialStore for GatePutCredentialStore {
        async fn load(
            &self,
            connector_id: &str,
        ) -> Result<Option<TelegramBotToken>, CredentialStoreError> {
            self.inner.load(connector_id).await
        }

        async fn put(
            &self,
            connector_id: &str,
            token: TelegramBotToken,
        ) -> Result<(), CredentialStoreError> {
            if self.gate_put.swap(false, Ordering::SeqCst) {
                self.entered.add_permits(1);
                self.release
                    .acquire()
                    .await
                    .map_err(|_| CredentialStoreError::OperationCancelled)?
                    .forget();
            }
            if self.fail_put.swap(false, Ordering::SeqCst) {
                return Err(CredentialStoreError::BackendUnavailable);
            }
            self.inner.put(connector_id, token).await
        }

        async fn delete(&self, connector_id: &str) -> Result<(), CredentialStoreError> {
            self.inner.delete(connector_id).await
        }
    }

    #[async_trait]
    impl ConnectorCredentialStore for GatePutUncertainDeleteStore {
        async fn load(
            &self,
            connector_id: &str,
        ) -> Result<Option<TelegramBotToken>, CredentialStoreError> {
            self.inner.load(connector_id).await
        }

        async fn put(
            &self,
            connector_id: &str,
            token: TelegramBotToken,
        ) -> Result<(), CredentialStoreError> {
            self.entered.add_permits(1);
            self.release
                .acquire()
                .await
                .map_err(|_| CredentialStoreError::OperationCancelled)?
                .forget();
            self.inner.put(connector_id, token).await
        }

        async fn delete(&self, connector_id: &str) -> Result<(), CredentialStoreError> {
            if self.fail_next_delete.swap(false, Ordering::SeqCst) {
                return Err(CredentialStoreError::CredentialStateUncertain);
            }
            self.inner.delete(connector_id).await
        }
    }

    #[async_trait]
    impl TelegramTransport for FakeTransport {
        async fn get_me(
            &self,
            token: &TelegramBotToken,
        ) -> Result<TelegramBotIdentity, TelegramTransportError> {
            self.calls.lock().unwrap().push("get_me");
            Ok(TelegramBotIdentity {
                id: token.expose().split(':').next().unwrap_or("bot").into(),
                username: Some("anima_bot".into()),
                display_name: Some("Anima".into()),
            })
        }

        async fn get_updates(
            &self,
            _token: &TelegramBotToken,
            offset: i64,
        ) -> Result<TelegramUpdateBatch, TelegramTransportError> {
            self.poll_calls.fetch_add(1, Ordering::SeqCst);
            if self.revoked_poll.swap(false, Ordering::SeqCst) {
                return Err(TelegramTransportError::UpstreamApi { code: Some(401) });
            }
            if self
                .remaining_poll_failures
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok()
            {
                return Err(TelegramTransportError::Transport);
            }
            Ok(TelegramUpdateBatch {
                updates: Vec::new(),
                next_update_id: offset,
            })
        }

        async fn send_message(
            &self,
            _token: &TelegramBotToken,
            chat_id: &str,
            text: &str,
        ) -> Result<Vec<TelegramSentMessage>, TelegramTransportError> {
            if self.revoked_send.swap(false, Ordering::SeqCst) {
                return Err(TelegramTransportError::HttpStatus { status: 401 });
            }
            if self
                .remaining_send_failures
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok()
            {
                return Err(TelegramTransportError::Transport);
            }
            self.sent
                .lock()
                .unwrap()
                .push((chat_id.to_string(), text.to_string()));
            Ok(Vec::new())
        }
    }

    #[async_trait]
    impl TelegramTransport for DropAwarePollingTransport {
        async fn get_me(
            &self,
            _token: &TelegramBotToken,
        ) -> Result<TelegramBotIdentity, TelegramTransportError> {
            Ok(TelegramBotIdentity {
                id: "42".into(),
                username: Some("drop_bot".into()),
                display_name: Some("Drop Bot".into()),
            })
        }

        async fn get_updates(
            &self,
            _token: &TelegramBotToken,
            _offset: i64,
        ) -> Result<TelegramUpdateBatch, TelegramTransportError> {
            let _guard = PollDropGuard(Arc::clone(&self.exited));
            self.entered.add_permits(1);
            std::future::pending().await
        }

        async fn send_message(
            &self,
            _token: &TelegramBotToken,
            _chat_id: &str,
            _text: &str,
        ) -> Result<Vec<TelegramSentMessage>, TelegramTransportError> {
            Ok(Vec::new())
        }
    }

    #[async_trait]
    impl ConnectorCredentialStore for ReplacementRaceCredentialStore {
        async fn load(
            &self,
            connector_id: &str,
        ) -> Result<Option<TelegramBotToken>, CredentialStoreError> {
            if self.fixed_worker_delivery.load(Ordering::SeqCst) {
                return self.inner.load(connector_id).await;
            }
            if self.load_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                self.worker_load_entered.add_permits(1);
                self.replacement_put_done
                    .acquire()
                    .await
                    .map_err(|_| CredentialStoreError::OperationCancelled)?
                    .forget();
            }
            self.inner.load(connector_id).await
        }

        async fn put(
            &self,
            connector_id: &str,
            token: TelegramBotToken,
        ) -> Result<(), CredentialStoreError> {
            self.inner.put(connector_id, token).await?;
            if self.put_calls.fetch_add(1, Ordering::SeqCst) > 0 {
                self.replacement_put_done.add_permits(2);
            }
            Ok(())
        }

        async fn delete(&self, connector_id: &str) -> Result<(), CredentialStoreError> {
            self.inner.delete(connector_id).await
        }
    }

    #[async_trait]
    impl TelegramTransport for ReplacementRaceTransport {
        async fn get_me(
            &self,
            token: &TelegramBotToken,
        ) -> Result<TelegramBotIdentity, TelegramTransportError> {
            Ok(TelegramBotIdentity {
                id: token.expose().split(':').next().unwrap_or("bot").into(),
                username: Some("race_bot".into()),
                display_name: Some("Race Bot".into()),
            })
        }

        async fn get_updates(
            &self,
            _token: &TelegramBotToken,
            offset: i64,
        ) -> Result<TelegramUpdateBatch, TelegramTransportError> {
            Ok(TelegramUpdateBatch {
                updates: Vec::new(),
                next_update_id: offset,
            })
        }

        async fn send_message(
            &self,
            token: &TelegramBotToken,
            _chat_id: &str,
            _text: &str,
        ) -> Result<Vec<TelegramSentMessage>, TelegramTransportError> {
            self.fixed_worker_delivery.store(true, Ordering::SeqCst);
            self.send_entered.add_permits(2);
            if self.gate_first_send.swap(false, Ordering::SeqCst) {
                self.release_send
                    .acquire()
                    .await
                    .map_err(|_| TelegramTransportError::Transport)?
                    .forget();
            }
            self.sent_tokens
                .lock()
                .unwrap()
                .push(token.expose().to_string());
            Ok(Vec::new())
        }
    }

    #[async_trait]
    impl TelegramTransport for InboundBackpressureTransport {
        async fn get_me(
            &self,
            _token: &TelegramBotToken,
        ) -> Result<TelegramBotIdentity, TelegramTransportError> {
            Ok(TelegramBotIdentity {
                id: "42".into(),
                username: Some("bounded_bot".into()),
                display_name: Some("Bounded Bot".into()),
            })
        }

        async fn get_updates(
            &self,
            _token: &TelegramBotToken,
            offset: i64,
        ) -> Result<TelegramUpdateBatch, TelegramTransportError> {
            if self.enabled.load(Ordering::SeqCst) && offset <= self.update.update_id {
                Ok(TelegramUpdateBatch {
                    updates: vec![self.update.clone()],
                    next_update_id: self.update.update_id + 1,
                })
            } else {
                Ok(TelegramUpdateBatch {
                    updates: Vec::new(),
                    next_update_id: offset,
                })
            }
        }

        async fn send_message(
            &self,
            _token: &TelegramBotToken,
            _chat_id: &str,
            _text: &str,
        ) -> Result<Vec<TelegramSentMessage>, TelegramTransportError> {
            self.delivery_attempts.add_permits(1);
            if self.fail_delivery.load(Ordering::SeqCst) {
                Err(TelegramTransportError::Transport)
            } else {
                Ok(Vec::new())
            }
        }
    }

    #[tokio::test]
    async fn create_verifies_before_storing_and_publishes_a_stable_room() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let credentials = Arc::new(InMemoryCredentialStore::default());
        let transport = Arc::new(FakeTransport::default());
        let manager = manager(Arc::clone(&state), credentials.clone(), transport.clone());

        let connector = manager
            .create(
                agent_id.clone(),
                TelegramBotToken::parse("42:test-token").unwrap(),
            )
            .await
            .expect("verified connector should be created");

        assert_eq!(transport.calls.lock().unwrap().as_slice(), ["get_me"]);
        assert_eq!(connector.agent_id, agent_id);
        assert_eq!(connector.room_id, format!("telegram:{}", connector.id));
        assert!(connector.enabled);
        assert_eq!(
            credentials
                .load(&connector.id)
                .await
                .unwrap()
                .unwrap()
                .expose(),
            "42:test-token"
        );
        assert_eq!(
            state.read().await.connectors.get(&connector.id),
            Some(&connector)
        );

        manager.shutdown().await;
    }

    #[tokio::test]
    async fn create_persists_cleanup_intent_before_vault_write_without_publishing_connector() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let credentials = Arc::new(GatePutUncertainDeleteStore {
            inner: InMemoryCredentialStore::default(),
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
            fail_next_delete: AtomicBool::new(true),
        });
        let manager = manager(
            Arc::clone(&state),
            credentials,
            Arc::new(FakeTransport::default()),
        );

        let creating = {
            let manager = manager.clone();
            tokio::spawn(async move {
                manager
                    .create(agent_id, TelegramBotToken::parse("42:draft-token").unwrap())
                    .await
            })
        };
        entered
            .acquire()
            .await
            .expect("credential put should be reached")
            .forget();

        let state_guard = state.read().await;
        assert!(state_guard.connectors.is_empty());
        assert_eq!(state_guard.credential_cleanup.len(), 1);
        let intent_id = state_guard
            .credential_cleanup
            .keys()
            .next()
            .unwrap()
            .clone();
        drop(state_guard);
        release.add_permits(1);
        let connector = creating.await.unwrap().unwrap();
        assert_eq!(connector.id, intent_id);
        assert!(state.read().await.connectors[&connector.id].is_active());
        assert!(state.read().await.credential_cleanup.is_empty());

        manager.shutdown().await;
    }

    #[tokio::test]
    async fn failed_active_create_publish_leaves_durable_intent_then_restart_cleans_credential() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let temporary = std::env::temp_dir().join(format!(
            "anima-create-intent-restart-{}-{}",
            std::process::id(),
            super::now_ms()
        ));
        let snapshot_path = temporary.join("control-plane.json");
        state.write().await.set_control_plane_store(Some(
            crate::control_plane_store::ControlPlaneStoreConfig::Json(snapshot_path.clone()),
        ));
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let credentials = Arc::new(GatePutUncertainDeleteStore {
            inner: InMemoryCredentialStore::default(),
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
            fail_next_delete: AtomicBool::new(true),
        });
        let first_manager = manager(
            Arc::clone(&state),
            credentials.clone(),
            Arc::new(FakeTransport::default()),
        );
        let creating = {
            let first_manager = first_manager.clone();
            tokio::spawn(async move {
                first_manager
                    .create(
                        agent_id.clone(),
                        TelegramBotToken::parse("42:cleanup-on-restart").unwrap(),
                    )
                    .await
            })
        };
        entered.acquire().await.unwrap().forget();
        let gate = state
            .write()
            .await
            .install_test_control_plane_save_gate(true);
        gate.release.add_permits(1);
        release.add_permits(1);
        assert_eq!(
            creating.await.unwrap().unwrap_err(),
            super::ConnectorManagerError::CredentialStateUncertain
        );
        let intent_id = state
            .read()
            .await
            .credential_cleanup
            .keys()
            .next()
            .unwrap()
            .clone();
        assert!(state.read().await.connectors.is_empty());
        assert_eq!(
            first_manager.status(&intent_id).await,
            Some(ConnectorRuntimeStatus::Reconciling)
        );
        assert!(credentials.load(&intent_id).await.unwrap().is_some());
        let durable = crate::control_plane_store::load_control_plane_snapshot(
            &crate::control_plane_store::ControlPlaneStoreConfig::Json(snapshot_path.clone()),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(durable.credential_cleanup[0].connector_id, intent_id);

        first_manager.shutdown().await;
        let mut fresh = DaemonState::new();
        fresh.set_control_plane_store(Some(
            crate::control_plane_store::ControlPlaneStoreConfig::Json(snapshot_path),
        ));
        fresh.restore_control_plane_snapshot(durable).unwrap();
        let restored_state = Arc::new(RwLock::new(fresh));
        let restored = manager(
            Arc::clone(&restored_state),
            credentials.clone(),
            Arc::new(FakeTransport::default()),
        );
        restored.start_restored().await;
        assert!(credentials.load(&intent_id).await.unwrap().is_none());
        assert!(restored_state.read().await.credential_cleanup.is_empty());
        assert_eq!(restored.status(&intent_id).await, None);
        let restored_agent_id = restored_state.read().await.list_agents()[0]
            .state
            .id
            .clone();
        release.add_permits(1);
        let replacement = restored
            .create(
                restored_agent_id,
                TelegramBotToken::parse("84:retry-after-cleanup").unwrap(),
            )
            .await
            .expect("a completed cleanup intent must not block retry");
        assert!(replacement.is_active());
        restored.shutdown().await;
        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[tokio::test]
    async fn persistent_startup_cleanup_failure_survives_restarts_for_later_reconciliation() {
        let state = state_with_agent();
        let temporary = std::env::temp_dir().join(format!(
            "anima-persistent-cleanup-intent-{}-{}",
            std::process::id(),
            super::now_ms()
        ));
        let snapshot_path = temporary.join("control-plane.json");
        state.write().await.set_control_plane_store(Some(
            crate::control_plane_store::ControlPlaneStoreConfig::Json(snapshot_path.clone()),
        ));
        let connector_id = "telegram-orphan-persistent".to_string();
        let persist = {
            let mut state = state.write().await;
            state.credential_cleanup.insert(
                connector_id.clone(),
                crate::connectors::TelegramCredentialCleanupIntent {
                    connector_id: connector_id.clone(),
                    created_at_ms: 1,
                },
            );
            state.control_plane_persist_request()
        };
        persist.save().await.unwrap();
        let credentials = Arc::new(UncertainDeleteStore::default());
        credentials
            .inner
            .put(
                &connector_id,
                TelegramBotToken::parse("42:persistent-orphan").unwrap(),
            )
            .await
            .unwrap();

        let first = manager(
            Arc::clone(&state),
            credentials.clone(),
            Arc::new(FakeTransport::default()),
        );
        first.start_restored().await;
        assert!(state
            .read()
            .await
            .credential_cleanup
            .contains_key(&connector_id));
        assert!(credentials.load(&connector_id).await.unwrap().is_some());
        assert_eq!(
            first.status(&connector_id).await,
            Some(ConnectorRuntimeStatus::Reconciling)
        );
        first.shutdown().await;

        let durable = crate::control_plane_store::load_control_plane_snapshot(
            &crate::control_plane_store::ControlPlaneStoreConfig::Json(snapshot_path.clone()),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(durable.credential_cleanup.len(), 1);
        let mut fresh = DaemonState::new();
        fresh.set_control_plane_store(Some(
            crate::control_plane_store::ControlPlaneStoreConfig::Json(snapshot_path),
        ));
        fresh.restore_control_plane_snapshot(durable).unwrap();
        let restored_state = Arc::new(RwLock::new(fresh));
        let restored = manager(
            Arc::clone(&restored_state),
            credentials.clone(),
            Arc::new(FakeTransport::default()),
        );
        restored.start_restored().await;
        assert!(restored_state
            .read()
            .await
            .credential_cleanup
            .contains_key(&connector_id));
        assert!(credentials.load(&connector_id).await.unwrap().is_some());
        assert_eq!(
            restored.status(&connector_id).await,
            Some(ConnectorRuntimeStatus::Reconciling)
        );
        restored.shutdown().await;
        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[tokio::test]
    async fn startup_cleanup_harmlessly_removes_stale_intent_for_absent_credential() {
        let state = state_with_agent();
        let temporary = std::env::temp_dir().join(format!(
            "anima-stale-cleanup-intent-{}-{}",
            std::process::id(),
            super::now_ms()
        ));
        let snapshot_path = temporary.join("control-plane.json");
        state.write().await.set_control_plane_store(Some(
            crate::control_plane_store::ControlPlaneStoreConfig::Json(snapshot_path.clone()),
        ));
        let connector_id = "telegram-orphan-absent".to_string();
        let persist = {
            let mut state = state.write().await;
            state.credential_cleanup.insert(
                connector_id.clone(),
                crate::connectors::TelegramCredentialCleanupIntent {
                    connector_id: connector_id.clone(),
                    created_at_ms: 1,
                },
            );
            state.control_plane_persist_request()
        };
        persist.save().await.unwrap();
        let manager = manager(
            Arc::clone(&state),
            Arc::new(InMemoryCredentialStore::default()),
            Arc::new(FakeTransport::default()),
        );

        manager.start_restored().await;

        assert!(state.read().await.credential_cleanup.is_empty());
        assert_eq!(manager.status(&connector_id).await, None);
        let durable = crate::control_plane_store::load_control_plane_snapshot(
            &crate::control_plane_store::ControlPlaneStoreConfig::Json(snapshot_path),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(durable.credential_cleanup.is_empty());
        manager.shutdown().await;
        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[tokio::test]
    async fn failed_initial_cleanup_intent_save_never_mutates_the_vault() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let invalid_path = std::env::temp_dir().join(format!(
            "anima-create-reconciliation-unavailable-{}-{}",
            std::process::id(),
            super::now_ms()
        ));
        std::fs::create_dir_all(&invalid_path).unwrap();
        state.write().await.set_control_plane_store(Some(
            crate::control_plane_store::ControlPlaneStoreConfig::Json(invalid_path.clone()),
        ));
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let credentials = Arc::new(GatePutUncertainDeleteStore {
            inner: InMemoryCredentialStore::default(),
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
            fail_next_delete: AtomicBool::new(true),
        });
        let manager = manager(
            Arc::clone(&state),
            credentials,
            Arc::new(FakeTransport::default()),
        );
        assert_eq!(
            manager
                .create(
                    agent_id,
                    TelegramBotToken::parse("42:uncertain-create").unwrap(),
                )
                .await
                .unwrap_err(),
            super::ConnectorManagerError::Persistence
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(50), entered.acquire())
                .await
                .is_err()
        );
        assert!(state.read().await.connectors.is_empty());
        assert!(state.read().await.credential_cleanup.is_empty());
        drop(release);
        manager.shutdown().await;
        std::fs::remove_dir_all(invalid_path).unwrap();
    }

    #[tokio::test]
    async fn successful_poll_recovers_status_after_transient_transport_error() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let transport = Arc::new(FakeTransport::default());
        transport.remaining_poll_failures.store(1, Ordering::SeqCst);
        let manager = manager(
            Arc::clone(&state),
            Arc::new(InMemoryCredentialStore::default()),
            transport,
        );
        let connector = manager
            .create(
                agent_id,
                TelegramBotToken::parse("42:transient-token").unwrap(),
            )
            .await
            .unwrap();

        assert!(
            wait_for_status(
                &manager,
                &connector.id,
                ConnectorRuntimeStatus::Error,
                Duration::from_millis(500),
            )
            .await,
            "the failed poll should expose a safe transient error"
        );
        assert!(
            wait_for_status(
                &manager,
                &connector.id,
                ConnectorRuntimeStatus::Pairing,
                Duration::from_secs(2),
            )
            .await,
            "a later successful poll should restore the operational status"
        );

        manager.shutdown().await;
    }

    #[tokio::test]
    async fn revoked_poll_credential_stops_worker_and_requires_replacement() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let transport = Arc::new(FakeTransport::default());
        transport.revoked_poll.store(true, Ordering::SeqCst);
        let manager = manager(
            state,
            Arc::new(InMemoryCredentialStore::default()),
            transport,
        );
        let connector = manager
            .create(
                agent_id,
                TelegramBotToken::parse("42:revoked-token").unwrap(),
            )
            .await
            .unwrap();

        assert!(
            wait_for_status(
                &manager,
                &connector.id,
                ConnectorRuntimeStatus::CredentialRequired,
                Duration::from_secs(1),
            )
            .await
        );
        tokio::time::timeout(Duration::from_secs(1), async {
            while manager.worker_count().await != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("revoked credentials must terminate their poller");
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn revoked_delivery_credential_stops_worker_and_requires_replacement() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let transport = Arc::new(FakeTransport::default());
        let manager = manager(
            Arc::clone(&state),
            Arc::new(InMemoryCredentialStore::default()),
            transport.clone(),
        );
        let connector = manager
            .create(
                agent_id.clone(),
                TelegramBotToken::parse("42:revoked-delivery").unwrap(),
            )
            .await
            .unwrap();
        transport.revoked_send.store(true, Ordering::SeqCst);
        {
            let mut state = state.write().await;
            state
                .connectors
                .get_mut(&connector.id)
                .unwrap()
                .approved_chat = Some(text_update(1, "202", "pair").chat);
            state.outbound.insert(
                "revoked-outbound".into(),
                crate::connectors::TelegramOutboundRecord {
                    id: "revoked-outbound".into(),
                    connector_id: connector.id.clone(),
                    agent_id,
                    room_id: connector.room_id.clone(),
                    assistant_message_id: "assistant-revoked".into(),
                    text: "cannot deliver".into(),
                    created_at_ms: super::now_ms(),
                    delivered_at_ms: None,
                    attempts: 0,
                    delivery_state: crate::connectors::OutboundDeliveryState::Pending,
                },
            );
        }

        assert!(
            wait_for_status(
                &manager,
                &connector.id,
                ConnectorRuntimeStatus::CredentialRequired,
                Duration::from_secs(1),
            )
            .await
        );
        tokio::time::timeout(Duration::from_secs(1), async {
            while manager.worker_count().await != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("revoked delivery credentials must terminate their worker");
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn only_one_active_connector_is_allowed_and_deletion_leaves_a_nonblocking_tombstone() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let credentials = Arc::new(InMemoryCredentialStore::default());
        let transport = Arc::new(FakeTransport::default());
        let manager = manager(Arc::clone(&state), credentials.clone(), transport);
        let first = manager
            .create(
                agent_id.clone(),
                TelegramBotToken::parse("42:first-token").unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            manager
                .create(
                    agent_id.clone(),
                    TelegramBotToken::parse("42:blocked-token").unwrap(),
                )
                .await
                .unwrap_err(),
            super::ConnectorManagerError::AgentAlreadyConnected
        );

        manager.delete(first.id.clone()).await.unwrap();
        let tombstone = state
            .read()
            .await
            .connectors
            .get(&first.id)
            .cloned()
            .unwrap();
        assert!(!tombstone.enabled);
        assert!(tombstone.deleted_at_ms.is_some());
        assert!(credentials.load(&first.id).await.unwrap().is_none());

        let replacement = manager
            .create(
                agent_id,
                TelegramBotToken::parse("42:replacement-token").unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(replacement.id, first.id);
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn batch_acceptance_advances_cursor_pairs_latest_then_accepts_only_approved_chat() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let manager = manager(
            Arc::clone(&state),
            Arc::new(InMemoryCredentialStore::default()),
            Arc::new(FakeTransport::default()),
        );
        let connector = manager
            .create(
                agent_id,
                TelegramBotToken::parse("42:pairing-token").unwrap(),
            )
            .await
            .unwrap();

        manager
            .accept_batch(
                connector.id.clone(),
                TelegramUpdateBatch {
                    updates: vec![
                        text_update(1, "101", "first"),
                        text_update(2, "202", "latest"),
                    ],
                    next_update_id: 3,
                },
            )
            .await
            .unwrap();
        {
            let state = state.read().await;
            let persisted = state.connectors.get(&connector.id).unwrap();
            assert_eq!(persisted.next_update_id, 3);
            assert_eq!(persisted.pending_pairing.as_ref().unwrap().chat.id, "202");
            assert!(state.inbound.is_empty());
        }

        manager.approve_pending(connector.id.clone()).await.unwrap();
        manager
            .accept_batch(
                connector.id.clone(),
                TelegramUpdateBatch {
                    updates: vec![
                        text_update(3, "101", "ignored"),
                        text_update(4, "202", "accepted"),
                    ],
                    next_update_id: 5,
                },
            )
            .await
            .unwrap();
        let state_guard = state.read().await;
        assert_eq!(state_guard.connectors[&connector.id].next_update_id, 5);
        assert_eq!(state_guard.inbound.len(), 1);
        let inbound = state_guard.inbound.get(&(connector.id.clone(), 4)).unwrap();
        assert_eq!(inbound.normalized_text, "accepted");
        assert_eq!(inbound.processing_state, InboundProcessingState::Received);
        assert_eq!(
            inbound.run_idempotency_key,
            format!("telegram:{}:update:4", connector.id)
        );
        drop(state_guard);

        manager.shutdown().await;
    }

    #[tokio::test]
    async fn stale_pairing_candidate_cannot_be_approved_and_a_new_chat_replaces_it() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let temporary = std::env::temp_dir().join(format!(
            "anima-pairing-approval-expiry-{}-{}",
            std::process::id(),
            super::now_ms()
        ));
        let snapshot_path = temporary.join("control-plane.json");
        state.write().await.set_control_plane_store(Some(
            crate::control_plane_store::ControlPlaneStoreConfig::Json(snapshot_path.clone()),
        ));
        let manager = manager(
            Arc::clone(&state),
            Arc::new(InMemoryCredentialStore::default()),
            Arc::new(FakeTransport::default()),
        );
        let connector = manager
            .create(
                agent_id,
                TelegramBotToken::parse("42:pairing-expiry").unwrap(),
            )
            .await
            .unwrap();
        manager
            .accept_batch(
                connector.id.clone(),
                TelegramUpdateBatch {
                    updates: vec![text_update(1, "101", "old")],
                    next_update_id: 2,
                },
            )
            .await
            .unwrap();
        state
            .write()
            .await
            .connectors
            .get_mut(&connector.id)
            .unwrap()
            .pending_pairing
            .as_mut()
            .unwrap()
            .requested_at_ms = super::now_ms().saturating_sub(super::PAIRING_CANDIDATE_TTL_MS + 1);

        assert_eq!(
            manager
                .approve_pending(connector.id.clone())
                .await
                .unwrap_err(),
            super::ConnectorManagerError::PendingPairingNotFound
        );
        assert!(state.read().await.connectors[&connector.id]
            .pending_pairing
            .is_none());
        let persisted = crate::control_plane_store::load_control_plane_snapshot(
            &crate::control_plane_store::ControlPlaneStoreConfig::Json(snapshot_path),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(persisted.connectors[0].pending_pairing.is_none());

        manager
            .accept_batch(
                connector.id.clone(),
                TelegramUpdateBatch {
                    updates: vec![text_update(2, "202", "new")],
                    next_update_id: 3,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            state.read().await.connectors[&connector.id]
                .pending_pairing
                .as_ref()
                .unwrap()
                .chat
                .id,
            "202"
        );
        manager.shutdown().await;
        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[tokio::test]
    async fn worker_proactively_expires_pairing_candidate_durably_and_allows_a_new_chat() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let temporary = std::env::temp_dir().join(format!(
            "anima-pairing-maintenance-{}-{}",
            std::process::id(),
            super::now_ms()
        ));
        let snapshot_path = temporary.join("control-plane.json");
        state.write().await.set_control_plane_store(Some(
            crate::control_plane_store::ControlPlaneStoreConfig::Json(snapshot_path.clone()),
        ));
        let manager = manager(
            Arc::clone(&state),
            Arc::new(InMemoryCredentialStore::default()),
            Arc::new(FakeTransport::default()),
        );
        let connector = manager
            .create(
                agent_id,
                TelegramBotToken::parse("42:pairing-maintenance").unwrap(),
            )
            .await
            .unwrap();
        manager.stop_worker(&connector.id).await.unwrap();
        manager
            .accept_batch(
                connector.id.clone(),
                TelegramUpdateBatch {
                    updates: vec![text_update(1, "101", "old")],
                    next_update_id: 2,
                },
            )
            .await
            .unwrap();
        let persist = {
            let mut state = state.write().await;
            state
                .connectors
                .get_mut(&connector.id)
                .unwrap()
                .pending_pairing
                .as_mut()
                .unwrap()
                .requested_at_ms =
                super::now_ms().saturating_sub(super::PAIRING_CANDIDATE_TTL_MS + 1);
            state.control_plane_persist_request()
        };
        persist.save().await.unwrap();

        manager
            .start_worker(
                connector.id.clone(),
                TelegramBotToken::parse("42:pairing-maintenance").unwrap(),
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if state.read().await.connectors[&connector.id]
                    .pending_pairing
                    .is_none()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("worker maintenance must clear an expired pairing candidate");
        let durable = crate::control_plane_store::load_control_plane_snapshot(
            &crate::control_plane_store::ControlPlaneStoreConfig::Json(snapshot_path),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(durable.connectors[0].pending_pairing.is_none());

        manager.stop_worker(&connector.id).await.unwrap();
        manager
            .accept_batch(
                connector.id.clone(),
                TelegramUpdateBatch {
                    updates: vec![text_update(2, "202", "new")],
                    next_update_id: 3,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            state.read().await.connectors[&connector.id]
                .pending_pairing
                .as_ref()
                .unwrap()
                .chat
                .id,
            "202"
        );
        manager.shutdown().await;
        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[tokio::test]
    async fn duplicate_update_reuses_exact_record_but_rejects_conflicting_payload() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let manager = manager(
            Arc::clone(&state),
            Arc::new(InMemoryCredentialStore::default()),
            Arc::new(FakeTransport::default()),
        );
        let connector = manager
            .create(
                agent_id,
                TelegramBotToken::parse("42:duplicate-token").unwrap(),
            )
            .await
            .unwrap();
        manager
            .accept_batch(
                connector.id.clone(),
                TelegramUpdateBatch {
                    updates: vec![text_update(1, "202", "pair")],
                    next_update_id: 2,
                },
            )
            .await
            .unwrap();
        manager.approve_pending(connector.id.clone()).await.unwrap();
        let accepted = TelegramUpdateBatch {
            updates: vec![text_update(2, "202", "same")],
            next_update_id: 3,
        };
        manager
            .accept_batch(connector.id.clone(), accepted.clone())
            .await
            .unwrap();
        manager
            .accept_batch(connector.id.clone(), accepted)
            .await
            .unwrap();
        assert_eq!(state.read().await.inbound.len(), 1);

        let error = manager
            .accept_batch(
                connector.id.clone(),
                TelegramUpdateBatch {
                    updates: vec![text_update(2, "202", "conflict")],
                    next_update_id: 3,
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error, super::ConnectorManagerError::ConflictingUpdate);
        assert_eq!(state.read().await.inbound.len(), 1);
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn conflicting_batch_prevalidates_before_inserting_any_new_update() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let manager = manager(
            Arc::clone(&state),
            Arc::new(InMemoryCredentialStore::default()),
            Arc::new(FakeTransport::default()),
        );
        let connector = manager
            .create(
                agent_id,
                TelegramBotToken::parse("42:atomic-batch").unwrap(),
            )
            .await
            .unwrap();
        manager
            .accept_batch(
                connector.id.clone(),
                TelegramUpdateBatch {
                    updates: vec![text_update(1, "202", "pair")],
                    next_update_id: 2,
                },
            )
            .await
            .unwrap();
        manager.approve_pending(connector.id.clone()).await.unwrap();
        manager
            .accept_batch(
                connector.id.clone(),
                TelegramUpdateBatch {
                    updates: vec![text_update(2, "202", "original")],
                    next_update_id: 3,
                },
            )
            .await
            .unwrap();

        let error = manager
            .accept_batch(
                connector.id.clone(),
                TelegramUpdateBatch {
                    updates: vec![
                        text_update(3, "202", "must-not-stick"),
                        text_update(2, "202", "conflict"),
                    ],
                    next_update_id: 4,
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error, super::ConnectorManagerError::ConflictingUpdate);
        let guard = state.read().await;
        assert!(!guard.inbound.contains_key(&(connector.id.clone(), 3)));
        assert_eq!(guard.connectors[&connector.id].next_update_id, 3);
        drop(guard);
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn failed_batch_persistence_rolls_back_cursor_and_pairing_candidate() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let manager = manager(
            Arc::clone(&state),
            Arc::new(InMemoryCredentialStore::default()),
            Arc::new(FakeTransport::default()),
        );
        let connector = manager
            .create(
                agent_id,
                TelegramBotToken::parse("42:persist-failure").unwrap(),
            )
            .await
            .unwrap();
        let invalid_path = std::env::temp_dir().join(format!(
            "anima-connector-invalid-store-{}-{}",
            std::process::id(),
            super::now_ms()
        ));
        std::fs::create_dir_all(&invalid_path).unwrap();
        state.write().await.set_control_plane_store(Some(
            crate::control_plane_store::ControlPlaneStoreConfig::Json(invalid_path.clone()),
        ));

        assert_eq!(
            manager
                .accept_batch(
                    connector.id.clone(),
                    TelegramUpdateBatch {
                        updates: vec![text_update(1, "202", "must roll back")],
                        next_update_id: 2,
                    },
                )
                .await
                .unwrap_err(),
            super::ConnectorManagerError::Persistence
        );
        let persisted = state.read().await.connectors[&connector.id].clone();
        assert_eq!(persisted.next_update_id, 0);
        assert!(persisted.pending_pairing.is_none());
        assert!(state.read().await.inbound.is_empty());
        manager.shutdown().await;
        std::fs::remove_dir_all(invalid_path).unwrap();
    }

    #[tokio::test]
    async fn failed_gated_connector_publish_rolls_back_before_agent_publisher_enters() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let temporary = std::env::temp_dir().join(format!(
            "anima-publish-isolation-{}-{}",
            std::process::id(),
            super::now_ms()
        ));
        let snapshot_path = temporary.join("control-plane.json");
        state.write().await.set_control_plane_store(Some(
            crate::control_plane_store::ControlPlaneStoreConfig::Json(snapshot_path.clone()),
        ));
        let manager = manager(
            Arc::clone(&state),
            Arc::new(InMemoryCredentialStore::default()),
            Arc::new(FakeTransport::default()),
        );
        let connector = manager
            .create(
                agent_id.clone(),
                TelegramBotToken::parse("42:publish-gate").unwrap(),
            )
            .await
            .unwrap();
        manager.stop_worker(&connector.id).await.unwrap();
        let gate = state
            .write()
            .await
            .install_test_control_plane_save_gate(true);

        let accepting = {
            let manager = manager.clone();
            let connector_id = connector.id.clone();
            tokio::spawn(async move {
                manager
                    .accept_batch(
                        connector_id,
                        TelegramUpdateBatch {
                            updates: vec![text_update(1, "202", "pair")],
                            next_update_id: 2,
                        },
                    )
                    .await
            })
        };
        gate.entered.acquire().await.unwrap().forget();

        let mut updating = {
            let runs = manager.runs.clone();
            let state = Arc::clone(&state);
            let agent_id = agent_id.clone();
            tokio::spawn(async move {
                let _transaction = runs.control_plane_transaction().await;
                let persist = {
                    let mut state = state.write().await;
                    state
                        .update_agent(
                            &agent_id,
                            AgentConfigUpdate {
                                name: Some("after rollback".into()),
                                ..AgentConfigUpdate::default()
                            },
                        )
                        .unwrap();
                    state.control_plane_persist_request()
                };
                persist.save().await.unwrap();
            })
        };
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut updating)
                .await
                .is_err()
        );

        gate.release.add_permits(1);
        assert_eq!(
            accepting.await.unwrap().unwrap_err(),
            super::ConnectorManagerError::Persistence
        );
        updating.await.unwrap();

        let state_guard = state.read().await;
        assert_eq!(state_guard.connectors[&connector.id].next_update_id, 0);
        assert_eq!(
            state_guard.get_agent(&agent_id).unwrap().state.config.name,
            "after rollback"
        );
        drop(state_guard);
        let persisted = crate::control_plane_store::load_control_plane_snapshot(
            &crate::control_plane_store::ControlPlaneStoreConfig::Json(snapshot_path),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(persisted.connectors[0].next_update_id, 0);
        assert_eq!(persisted.agents[0].state.config.name, "after rollback");
        manager.shutdown().await;
        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[tokio::test]
    async fn failed_gated_delivery_rolls_back_before_agent_publisher_enters() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let temporary = std::env::temp_dir().join(format!(
            "anima-delivery-publish-isolation-{}-{}",
            std::process::id(),
            super::now_ms()
        ));
        let snapshot_path = temporary.join("control-plane.json");
        state.write().await.set_control_plane_store(Some(
            crate::control_plane_store::ControlPlaneStoreConfig::Json(snapshot_path.clone()),
        ));
        let transport = Arc::new(FakeTransport::default());
        let manager = manager(
            Arc::clone(&state),
            Arc::new(InMemoryCredentialStore::default()),
            transport.clone(),
        );
        let connector = manager
            .create(
                agent_id.clone(),
                TelegramBotToken::parse("42:delivery-publish-gate").unwrap(),
            )
            .await
            .unwrap();
        manager.stop_worker(&connector.id).await.unwrap();
        manager
            .accept_batch(
                connector.id.clone(),
                TelegramUpdateBatch {
                    updates: vec![text_update(1, "202", "pair")],
                    next_update_id: 2,
                },
            )
            .await
            .unwrap();
        manager.approve_pending(connector.id.clone()).await.unwrap();
        manager
            .accept_batch(
                connector.id.clone(),
                TelegramUpdateBatch {
                    updates: vec![text_update(2, "202", "deliver once")],
                    next_update_id: 3,
                },
            )
            .await
            .unwrap();
        manager
            .process_pending_once(connector.id.clone())
            .await
            .unwrap();
        let outbound_id = state.read().await.outbound.keys().next().unwrap().clone();
        let gate = state
            .write()
            .await
            .install_test_control_plane_save_gate(true);

        let delivering = {
            let manager = manager.clone();
            let connector_id = connector.id.clone();
            tokio::spawn(async move { manager.deliver_pending_once(connector_id).await })
        };
        gate.entered.acquire().await.unwrap().forget();

        let mut updating = {
            let runs = manager.runs.clone();
            let state = Arc::clone(&state);
            let agent_id = agent_id.clone();
            tokio::spawn(async move {
                let _transaction = runs.control_plane_transaction().await;
                let persist = {
                    let mut state = state.write().await;
                    state
                        .update_agent(
                            &agent_id,
                            AgentConfigUpdate {
                                name: Some("after delivery rollback".into()),
                                ..AgentConfigUpdate::default()
                            },
                        )
                        .unwrap();
                    state.control_plane_persist_request()
                };
                persist.save().await.unwrap();
            })
        };
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut updating)
                .await
                .is_err()
        );

        gate.release.add_permits(1);
        assert_eq!(
            delivering.await.unwrap().unwrap_err(),
            super::ConnectorManagerError::Persistence
        );
        assert_eq!(transport.sent.lock().unwrap().len(), 1);
        updating.await.unwrap();

        let state_guard = state.read().await;
        let outbound = &state_guard.outbound[&outbound_id];
        assert_eq!(
            outbound.delivery_state,
            crate::connectors::OutboundDeliveryState::Pending
        );
        assert_eq!(outbound.attempts, 0);
        assert_eq!(
            state_guard.get_agent(&agent_id).unwrap().state.config.name,
            "after delivery rollback"
        );
        drop(state_guard);
        let persisted = crate::control_plane_store::load_control_plane_snapshot(
            &crate::control_plane_store::ControlPlaneStoreConfig::Json(snapshot_path),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            persisted.outbound[0].delivery_state,
            crate::connectors::OutboundDeliveryState::Pending
        );
        assert_eq!(persisted.outbound[0].attempts, 0);
        assert_eq!(
            persisted.agents[0].state.config.name,
            "after delivery rollback"
        );
        assert!(manager
            .deliver_pending_once(connector.id.clone())
            .await
            .unwrap());
        assert_eq!(transport.sent.lock().unwrap().len(), 2);
        let retried = state.read().await.outbound[&outbound_id].clone();
        assert_eq!(
            retried.delivery_state,
            crate::connectors::OutboundDeliveryState::Delivered
        );
        assert_eq!(retried.attempts, 1);
        manager.shutdown().await;
        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[tokio::test]
    async fn failed_final_run_snapshot_leaves_no_deliverable_outbox_and_retries_processing() {
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let mut daemon = DaemonState::with_model_adapter(Arc::new(GateModelAdapter {
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        }));
        daemon.create_agent(test_config()).unwrap();
        let state = Arc::new(RwLock::new(daemon));
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let manager = manager(
            Arc::clone(&state),
            Arc::new(InMemoryCredentialStore::default()),
            Arc::new(FakeTransport::default()),
        );
        let connector = manager
            .create(
                agent_id.clone(),
                TelegramBotToken::parse("42:token").unwrap(),
            )
            .await
            .unwrap();
        manager.shutdown().await;
        manager
            .accept_batch(
                connector.id.clone(),
                TelegramUpdateBatch {
                    updates: vec![text_update(1, "101", "pair")],
                    next_update_id: 2,
                },
            )
            .await
            .unwrap();
        manager.approve_pending(connector.id.clone()).await.unwrap();
        manager
            .accept_batch(
                connector.id.clone(),
                TelegramUpdateBatch {
                    updates: vec![text_update(2, "101", "run")],
                    next_update_id: 3,
                },
            )
            .await
            .unwrap();
        let baseline_agent = state.read().await.get_agent(&agent_id).unwrap();

        let temporary = std::env::temp_dir().join(format!(
            "anima-connector-final-persist-{}-{}",
            std::process::id(),
            super::now_ms()
        ));
        std::fs::create_dir_all(&temporary).unwrap();
        let snapshot_path = temporary.join("control-plane.json");
        state.write().await.set_control_plane_store(Some(
            crate::control_plane_store::ControlPlaneStoreConfig::Json(snapshot_path.clone()),
        ));
        let processing_manager = manager.clone();
        let connector_id = connector.id.clone();
        let processing =
            tokio::spawn(
                async move { processing_manager.process_pending_once(connector_id).await },
            );
        entered
            .acquire()
            .await
            .expect("model should enter after the running snapshot is durable")
            .forget();
        state
            .write()
            .await
            .update_agent(
                &agent_id,
                AgentConfigUpdate {
                    name: Some("updated-operator".into()),
                    ..AgentConfigUpdate::default()
                },
            )
            .unwrap();
        std::fs::remove_file(&snapshot_path).unwrap();
        std::fs::create_dir(&snapshot_path).unwrap();
        release.add_permits(1);

        assert_eq!(
            processing.await.unwrap().unwrap_err(),
            super::ConnectorManagerError::Persistence
        );
        let state_guard = state.read().await;
        assert_eq!(
            state_guard
                .inbound
                .get(&(connector.id.clone(), 2))
                .unwrap()
                .processing_state,
            InboundProcessingState::Processing
        );
        assert!(state_guard
            .outbound
            .values()
            .all(|record| record.connector_id != connector.id));
        drop(state_guard);

        let rolled_back = state.read().await.get_agent(&agent_id).unwrap();
        assert_eq!(rolled_back.messages, baseline_agent.messages);
        assert_eq!(rolled_back.state.name, "updated-operator");

        std::fs::remove_dir(&snapshot_path).unwrap();
        let retry_manager = manager.clone();
        let retry_connector_id = connector.id.clone();
        let retry =
            tokio::spawn(
                async move { retry_manager.process_pending_once(retry_connector_id).await },
            );
        entered
            .acquire()
            .await
            .expect("retry should enter the model")
            .forget();
        release.add_permits(1);
        assert!(retry.await.unwrap().unwrap());
        let retried = state.read().await.get_agent(&agent_id).unwrap();
        assert_eq!(retried.messages.len(), baseline_agent.messages.len() + 2);
        assert_eq!(retried.state.name, "updated-operator");
        assert_eq!(
            state
                .read()
                .await
                .inbound
                .get(&(connector.id.clone(), 2))
                .unwrap()
                .processing_state,
            InboundProcessingState::Processed
        );
        assert_eq!(
            state
                .read()
                .await
                .outbound
                .values()
                .filter(|record| record.connector_id == connector.id)
                .count(),
            1
        );
        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[tokio::test]
    async fn owner_thread_idempotency_singleflights_replays_and_rejects_conflicts() {
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let mut daemon = DaemonState::with_model_adapter(Arc::new(GateModelAdapter {
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        }));
        daemon.create_agent(test_config()).unwrap();
        let state = Arc::new(RwLock::new(daemon));
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let manager = manager(
            Arc::clone(&state),
            Arc::new(InMemoryCredentialStore::default()),
            Arc::new(FakeTransport::default()),
        );
        let connector = manager
            .create(
                agent_id.clone(),
                TelegramBotToken::parse("42:owner-idempotency").unwrap(),
            )
            .await
            .unwrap();
        manager.stop_worker(&connector.id).await.unwrap();

        let first = {
            let manager = manager.clone();
            let agent_id = agent_id.clone();
            let connector_id = connector.id.clone();
            tokio::spawn(async move {
                manager
                    .send_from_owner(
                        agent_id,
                        connector_id,
                        "same owner turn".into(),
                        "owner-key".into(),
                    )
                    .await
            })
        };
        entered.acquire().await.unwrap().forget();
        let retry = {
            let manager = manager.clone();
            let agent_id = agent_id.clone();
            let connector_id = connector.id.clone();
            tokio::spawn(async move {
                manager
                    .send_from_owner(
                        agent_id,
                        connector_id,
                        "same owner turn".into(),
                        "owner-key".into(),
                    )
                    .await
            })
        };
        assert!(
            tokio::time::timeout(Duration::from_millis(20), entered.acquire())
                .await
                .is_err(),
            "retry must wait on the same manager-owned singleflight"
        );
        release.add_permits(1);
        assert!(!first.await.unwrap().unwrap().1);
        assert!(!retry.await.unwrap().unwrap().1);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), entered.acquire())
                .await
                .is_err(),
            "committed retry must replay without another model call"
        );

        let snapshot = state.read().await.get_agent(&agent_id).unwrap();
        let room_messages = snapshot
            .messages
            .iter()
            .filter(|message| message.room_id == connector.room_id)
            .collect::<Vec<_>>();
        assert_eq!(room_messages.len(), 2);
        assert_eq!(
            room_messages[0]
                .content
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("idempotencyKey")),
            Some(&DataValue::String("owner-key".into()))
        );

        assert_eq!(
            manager
                .send_from_owner(
                    agent_id,
                    connector.id,
                    "different owner turn".into(),
                    "owner-key".into(),
                )
                .await
                .unwrap_err(),
            super::ConnectorManagerError::IdempotencyConflict
        );
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn owner_thread_idempotency_replays_after_control_plane_restart() {
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let mut daemon = DaemonState::with_model_adapter(Arc::new(GateModelAdapter {
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        }));
        daemon.create_agent(test_config()).unwrap();
        let temporary = std::env::temp_dir().join(format!(
            "anima-owner-idempotency-restart-{}-{}",
            std::process::id(),
            super::now_ms()
        ));
        let snapshot_path = temporary.join("control-plane.json");
        daemon.set_control_plane_store(Some(
            crate::control_plane_store::ControlPlaneStoreConfig::Json(snapshot_path.clone()),
        ));
        let state = Arc::new(RwLock::new(daemon));
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let first_manager = manager(
            Arc::clone(&state),
            Arc::new(InMemoryCredentialStore::default()),
            Arc::new(FakeTransport::default()),
        );
        let connector = first_manager
            .create(
                agent_id.clone(),
                TelegramBotToken::parse("42:owner-restart").unwrap(),
            )
            .await
            .unwrap();
        first_manager.stop_worker(&connector.id).await.unwrap();
        let first = {
            let manager = first_manager.clone();
            let agent_id = agent_id.clone();
            let connector_id = connector.id.clone();
            tokio::spawn(async move {
                manager
                    .send_from_owner(
                        agent_id,
                        connector_id,
                        "restart-safe turn".into(),
                        "restart-key".into(),
                    )
                    .await
            })
        };
        entered.acquire().await.unwrap().forget();
        release.add_permits(1);
        assert!(!first.await.unwrap().unwrap().1);
        first_manager.shutdown().await;

        let snapshot = crate::control_plane_store::load_control_plane_snapshot(
            &crate::control_plane_store::ControlPlaneStoreConfig::Json(snapshot_path),
        )
        .await
        .unwrap()
        .unwrap();
        let retry_entered = Arc::new(Semaphore::new(0));
        let mut restored = DaemonState::with_model_adapter(Arc::new(GateModelAdapter {
            entered: Arc::clone(&retry_entered),
            release: Arc::new(Semaphore::new(0)),
        }));
        restored.restore_control_plane_snapshot(snapshot).unwrap();
        let restored = Arc::new(RwLock::new(restored));
        let retry_manager = manager(
            Arc::clone(&restored),
            Arc::new(InMemoryCredentialStore::default()),
            Arc::new(FakeTransport::default()),
        );
        let replay = retry_manager
            .send_from_owner(
                agent_id.clone(),
                connector.id.clone(),
                "restart-safe turn".into(),
                "restart-key".into(),
            )
            .await
            .unwrap();
        assert!(!replay.1);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), retry_entered.acquire())
                .await
                .is_err(),
            "durable replay must not call the model after restart"
        );
        assert_eq!(
            restored
                .read()
                .await
                .get_agent(&agent_id)
                .unwrap()
                .messages
                .iter()
                .filter(|message| message.room_id == connector.room_id)
                .count(),
            2
        );
        retry_manager.shutdown().await;
        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[tokio::test]
    async fn delivery_cannot_send_outbound_from_a_failing_final_owner_thread_snapshot() {
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let mut daemon = DaemonState::with_model_adapter(Arc::new(GateModelAdapter {
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        }));
        daemon.create_agent(test_config()).unwrap();
        let state = Arc::new(RwLock::new(daemon));
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let transport = Arc::new(FakeTransport::default());
        let manager = manager(
            Arc::clone(&state),
            Arc::new(InMemoryCredentialStore::default()),
            transport.clone(),
        );
        let connector = manager
            .create(
                agent_id.clone(),
                TelegramBotToken::parse("42:owner-final-save-gate").unwrap(),
            )
            .await
            .unwrap();
        manager.stop_worker(&connector.id).await.unwrap();
        manager
            .accept_batch(
                connector.id.clone(),
                TelegramUpdateBatch {
                    updates: vec![text_update(1, "101", "pair")],
                    next_update_id: 2,
                },
            )
            .await
            .unwrap();
        manager
            .approve_pending_chat(connector.id.clone(), Some("101".into()))
            .await
            .unwrap();

        let sending = {
            let manager = manager.clone();
            let agent_id = agent_id.clone();
            let connector_id = connector.id.clone();
            tokio::spawn(async move {
                manager
                    .send_from_owner(
                        agent_id,
                        connector_id,
                        "owner web turn".into(),
                        "owner-final-save".into(),
                    )
                    .await
            })
        };
        entered.acquire().await.unwrap().forget();
        let save_gate = state
            .write()
            .await
            .install_test_control_plane_save_gate(true);
        release.add_permits(1);
        save_gate.entered.acquire().await.unwrap().forget();

        let delivering = {
            let manager = manager.clone();
            let connector_id = connector.id.clone();
            tokio::spawn(async move { manager.deliver_pending_once(connector_id).await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            transport.sent.lock().unwrap().is_empty(),
            "delivery must not observe outbound before the final snapshot commits"
        );

        save_gate.release.add_permits(1);
        assert_eq!(
            sending.await.unwrap().unwrap_err(),
            super::ConnectorManagerError::Persistence
        );
        assert!(!delivering.await.unwrap().unwrap());
        assert!(transport.sent.lock().unwrap().is_empty());
        assert!(state
            .read()
            .await
            .outbound
            .values()
            .all(|outbound| outbound.connector_id != connector.id));
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn owner_thread_run_releases_lifecycle_lock_and_rolls_back_if_deleted() {
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let mut daemon = DaemonState::with_model_adapter(Arc::new(GateModelAdapter {
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        }));
        daemon.create_agent(test_config()).unwrap();
        let state = Arc::new(RwLock::new(daemon));
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let manager = manager(
            Arc::clone(&state),
            Arc::new(InMemoryCredentialStore::default()),
            Arc::new(FakeTransport::default()),
        );
        let connector = manager
            .create(
                agent_id.clone(),
                TelegramBotToken::parse("42:owner-thread").unwrap(),
            )
            .await
            .unwrap();
        manager
            .accept_batch(
                connector.id.clone(),
                TelegramUpdateBatch {
                    updates: vec![text_update(1, "101", "pair")],
                    next_update_id: 2,
                },
            )
            .await
            .unwrap();
        manager
            .approve_pending_chat(connector.id.clone(), Some("101".into()))
            .await
            .unwrap();
        let baseline = state.read().await.get_agent(&agent_id).unwrap();

        let sending_manager = manager.clone();
        let sending_agent_id = agent_id.clone();
        let sending_connector_id = connector.id.clone();
        let sending = tokio::spawn(async move {
            sending_manager
                .send_from_owner(
                    sending_agent_id,
                    sending_connector_id,
                    "owner web turn".into(),
                    "owner-delete-race".into(),
                )
                .await
        });
        entered
            .acquire()
            .await
            .expect("owner thread run should enter model")
            .forget();

        let deleting_manager = manager.clone();
        let deleting_connector_id = connector.id.clone();
        let deleting =
            tokio::spawn(async move { deleting_manager.delete(deleting_connector_id).await });
        let deleted_while_model_was_running =
            tokio::time::timeout(Duration::from_millis(200), deleting).await;
        release.add_permits(1);

        assert!(
            deleted_while_model_was_running.is_ok(),
            "connector lifecycle mutations must not wait for model/tool execution"
        );
        assert!(deleted_while_model_was_running.unwrap().unwrap().is_ok());
        assert_eq!(
            sending.await.unwrap().unwrap_err(),
            super::ConnectorManagerError::Persistence
        );
        let state = state.read().await;
        assert_eq!(
            state.get_agent(&agent_id).unwrap().messages,
            baseline.messages
        );
        assert!(state
            .outbound
            .values()
            .all(|outbound| outbound.connector_id != connector.id));
        assert!(state.connectors[&connector.id].deleted_at_ms.is_some());
    }

    #[tokio::test]
    async fn failed_run_publish_preserves_inbound_accepted_while_model_was_running() {
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let mut daemon = DaemonState::with_model_adapter(Arc::new(GateModelAdapter {
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        }));
        daemon.create_agent(test_config()).unwrap();
        let state = Arc::new(RwLock::new(daemon));
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let temporary = std::env::temp_dir().join(format!(
            "anima-run-delta-rollback-{}-{}",
            std::process::id(),
            super::now_ms()
        ));
        let snapshot_path = temporary.join("control-plane.json");
        state.write().await.set_control_plane_store(Some(
            crate::control_plane_store::ControlPlaneStoreConfig::Json(snapshot_path.clone()),
        ));
        let manager = manager(
            Arc::clone(&state),
            Arc::new(InMemoryCredentialStore::default()),
            Arc::new(FakeTransport::default()),
        );
        let connector = manager
            .create(
                agent_id.clone(),
                TelegramBotToken::parse("42:delta-rollback").unwrap(),
            )
            .await
            .unwrap();
        manager.stop_worker(&connector.id).await.unwrap();
        manager
            .accept_batch(
                connector.id.clone(),
                TelegramUpdateBatch {
                    updates: vec![text_update(1, "101", "pair")],
                    next_update_id: 2,
                },
            )
            .await
            .unwrap();
        manager.approve_pending(connector.id.clone()).await.unwrap();
        manager
            .accept_batch(
                connector.id.clone(),
                TelegramUpdateBatch {
                    updates: vec![text_update(2, "101", "run A")],
                    next_update_id: 3,
                },
            )
            .await
            .unwrap();

        let processing = {
            let manager = manager.clone();
            let connector_id = connector.id.clone();
            tokio::spawn(async move { manager.process_pending_once(connector_id).await })
        };
        entered.acquire().await.unwrap().forget();
        manager
            .accept_batch(
                connector.id.clone(),
                TelegramUpdateBatch {
                    updates: vec![text_update(3, "101", "run B")],
                    next_update_id: 4,
                },
            )
            .await
            .unwrap();
        let gate = state
            .write()
            .await
            .install_test_control_plane_save_gate(true);
        release.add_permits(1);
        gate.entered.acquire().await.unwrap().forget();
        gate.release.add_permits(1);

        assert_eq!(
            processing.await.unwrap().unwrap_err(),
            super::ConnectorManagerError::Persistence
        );
        let state_guard = state.read().await;
        assert!(state_guard.inbound.contains_key(&(connector.id.clone(), 2)));
        assert_eq!(
            state_guard.inbound[&(connector.id.clone(), 3)].normalized_text,
            "run B"
        );
        drop(state_guard);

        let _transaction = manager.runs.control_plane_transaction().await;
        let later_publish = {
            let mut state = state.write().await;
            state
                .update_agent(
                    &agent_id,
                    AgentConfigUpdate {
                        name: Some("published after rollback".into()),
                        ..AgentConfigUpdate::default()
                    },
                )
                .unwrap();
            state.control_plane_persist_request()
        };
        later_publish.save().await.unwrap();
        drop(_transaction);
        let persisted = crate::control_plane_store::load_control_plane_snapshot(
            &crate::control_plane_store::ControlPlaneStoreConfig::Json(snapshot_path),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(persisted
            .inbound
            .iter()
            .any(|record| record.connector_id == connector.id && record.update_id == 3));
        let mut restored = DaemonState::new();
        restored.restore_control_plane_snapshot(persisted).unwrap();
        assert!(restored.inbound.contains_key(&(connector.id.clone(), 3)));
        manager.shutdown().await;
        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[tokio::test]
    async fn serialized_rollback_preserves_a_turn_committed_while_connector_waited() {
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let mut daemon = DaemonState::with_model_adapter(Arc::new(GateModelAdapter {
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        }));
        daemon.create_agent(test_config()).unwrap();
        let state = Arc::new(RwLock::new(daemon));
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let limiter = Arc::new(Semaphore::new(2));
        let runs = AgentRunCoordinator::new(Arc::clone(&state), Arc::clone(&limiter));
        let manager = ConnectorManager::new(
            Arc::clone(&state),
            runs.clone(),
            Arc::new(InMemoryCredentialStore::default()),
            Arc::new(FakeTransport::default()),
        );
        let connector = manager
            .create(
                agent_id.clone(),
                TelegramBotToken::parse("42:serialized-token").unwrap(),
            )
            .await
            .unwrap();
        manager.shutdown().await;
        manager
            .accept_batch(
                connector.id.clone(),
                TelegramUpdateBatch {
                    updates: vec![text_update(1, "101", "pair")],
                    next_update_id: 2,
                },
            )
            .await
            .unwrap();
        manager.approve_pending(connector.id.clone()).await.unwrap();
        manager
            .accept_batch(
                connector.id.clone(),
                TelegramUpdateBatch {
                    updates: vec![text_update(2, "101", "connector run")],
                    next_update_id: 3,
                },
            )
            .await
            .unwrap();
        let original = state.read().await.get_agent(&agent_id).unwrap();

        let temporary = std::env::temp_dir().join(format!(
            "anima-connector-serialized-rollback-{}-{}",
            std::process::id(),
            super::now_ms()
        ));
        std::fs::create_dir_all(&temporary).unwrap();
        let snapshot_path = temporary.join("control-plane.json");
        state.write().await.set_control_plane_store(Some(
            crate::control_plane_store::ControlPlaneStoreConfig::Json(snapshot_path.clone()),
        ));

        let intervening_runs = runs.clone();
        let intervening_agent_id = agent_id.clone();
        let intervening = tokio::spawn(async move {
            intervening_runs
                .run(AgentRunRequest {
                    agent_id: intervening_agent_id,
                    content: Content {
                        text: "intervening turn".into(),
                        ..Content::default()
                    },
                    room: RunRoom::Generated,
                    idempotency_key: None,
                })
                .await
        });
        entered
            .acquire()
            .await
            .expect("intervening turn should hold the per-agent lock")
            .forget();

        let processing_manager = manager.clone();
        let connector_id = connector.id.clone();
        let processing =
            tokio::spawn(
                async move { processing_manager.process_pending_once(connector_id).await },
            );
        tokio::time::timeout(Duration::from_secs(1), async {
            while limiter.available_permits() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("connector should acquire global admission and wait on the agent lock");

        release.add_permits(1);
        intervening.await.unwrap().unwrap();
        entered
            .acquire()
            .await
            .expect("connector should enter only after the intervening turn commits")
            .forget();
        let committed_intervening = state.read().await.get_agent(&agent_id).unwrap();
        assert_eq!(
            committed_intervening.messages.len(),
            original.messages.len() + 2
        );

        std::fs::remove_file(&snapshot_path).unwrap();
        std::fs::create_dir(&snapshot_path).unwrap();
        release.add_permits(1);
        assert_eq!(
            processing.await.unwrap().unwrap_err(),
            super::ConnectorManagerError::Persistence
        );

        let rolled_back = state.read().await.get_agent(&agent_id).unwrap();
        assert_eq!(rolled_back.messages, committed_intervening.messages);
        assert_eq!(
            state
                .read()
                .await
                .inbound
                .get(&(connector.id.clone(), 2))
                .unwrap()
                .processing_state,
            InboundProcessingState::Processing
        );
        assert!(state
            .read()
            .await
            .outbound
            .values()
            .all(|record| record.connector_id != connector.id));

        std::fs::remove_dir(&snapshot_path).unwrap();
        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[tokio::test]
    async fn background_processing_waits_for_global_admission_instead_of_failing() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let limiter = Arc::new(Semaphore::new(0));
        let runs = AgentRunCoordinator::new(Arc::clone(&state), Arc::clone(&limiter));
        let manager = ConnectorManager::new(
            Arc::clone(&state),
            runs,
            Arc::new(InMemoryCredentialStore::default()),
            Arc::new(FakeTransport::default()),
        );
        let connector = manager
            .create(agent_id, TelegramBotToken::parse("42:token").unwrap())
            .await
            .unwrap();
        manager.shutdown().await;
        manager
            .accept_batch(
                connector.id.clone(),
                TelegramUpdateBatch {
                    updates: vec![text_update(1, "101", "pair")],
                    next_update_id: 2,
                },
            )
            .await
            .unwrap();
        manager.approve_pending(connector.id.clone()).await.unwrap();
        manager
            .accept_batch(
                connector.id.clone(),
                TelegramUpdateBatch {
                    updates: vec![text_update(2, "101", "wait")],
                    next_update_id: 3,
                },
            )
            .await
            .unwrap();

        let processing_manager = manager.clone();
        let connector_id = connector.id.clone();
        let processing =
            tokio::spawn(
                async move { processing_manager.process_pending_once(connector_id).await },
            );
        tokio::time::sleep(Duration::from_millis(20)).await;
        let waited = !processing.is_finished();
        limiter.add_permits(1);
        let result = processing.await.unwrap();

        assert!(waited);
        assert_eq!(result, Ok(true));
    }

    #[tokio::test]
    async fn processing_commits_agent_message_inbound_and_outbox_then_delivers_stored_text() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let credentials = Arc::new(InMemoryCredentialStore::default());
        let transport = Arc::new(FakeTransport::default());
        let manager = manager(Arc::clone(&state), credentials, transport.clone());
        let connector = manager
            .create(
                agent_id.clone(),
                TelegramBotToken::parse("42:processing-token").unwrap(),
            )
            .await
            .unwrap();
        manager
            .accept_batch(
                connector.id.clone(),
                TelegramUpdateBatch {
                    updates: vec![text_update(1, "202", "pair")],
                    next_update_id: 2,
                },
            )
            .await
            .unwrap();
        manager.approve_pending(connector.id.clone()).await.unwrap();
        manager
            .accept_batch(
                connector.id.clone(),
                TelegramUpdateBatch {
                    updates: vec![text_update(2, "202", "hello from telegram")],
                    next_update_id: 3,
                },
            )
            .await
            .unwrap();

        manager
            .process_pending_once(connector.id.clone())
            .await
            .unwrap();
        let outbound = {
            let state = state.read().await;
            assert_eq!(
                state.inbound[&(connector.id.clone(), 2)].processing_state,
                InboundProcessingState::Processed
            );
            let outbound = state.outbound.values().next().cloned().unwrap();
            assert_eq!(outbound.text, "operator handled task: hello from telegram");
            let snapshot = state.get_agent(&agent_id).unwrap();
            assert!(snapshot.messages.iter().any(|message| {
                message.id == outbound.assistant_message_id
                    && message.room_id == connector.room_id
                    && message.role == anima_core::MessageRole::Assistant
            }));
            outbound
        };

        {
            let mut state = state.write().await;
            for (id, connector_id) in [
                ("old-selected", connector.id.clone()),
                ("old-other", "other-connector".into()),
            ] {
                state.outbound.insert(
                    id.into(),
                    crate::connectors::TelegramOutboundRecord {
                        id: id.into(),
                        connector_id,
                        agent_id: agent_id.clone(),
                        room_id: connector.room_id.clone(),
                        assistant_message_id: format!("assistant-{id}"),
                        text: id.into(),
                        created_at_ms: 1,
                        delivered_at_ms: Some(1),
                        attempts: 1,
                        delivery_state: crate::connectors::OutboundDeliveryState::Delivered,
                    },
                );
            }
        }

        manager
            .deliver_pending_once(connector.id.clone())
            .await
            .unwrap();
        let delivered = state.read().await.outbound[&outbound.id].clone();
        assert_eq!(
            delivered.delivery_state,
            crate::connectors::OutboundDeliveryState::Delivered
        );
        assert_eq!(delivered.attempts, 1);
        assert!(delivered.delivered_at_ms.is_some());
        assert!(!state.read().await.outbound.contains_key("old-selected"));
        assert!(state.read().await.outbound.contains_key("old-other"));
        assert_eq!(
            transport.sent.lock().unwrap().as_slice(),
            [("202".into(), outbound.text)]
        );
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn delivery_retry_uses_outbox_without_running_the_agent_again() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let transport = Arc::new(FakeTransport::default());
        transport.remaining_send_failures.store(1, Ordering::SeqCst);
        let manager = manager(
            Arc::clone(&state),
            Arc::new(InMemoryCredentialStore::default()),
            transport.clone(),
        );
        let connector = manager
            .create(
                agent_id.clone(),
                TelegramBotToken::parse("42:retry-token").unwrap(),
            )
            .await
            .unwrap();
        manager.stop_worker(&connector.id).await.unwrap();
        manager
            .accept_batch(
                connector.id.clone(),
                TelegramUpdateBatch {
                    updates: vec![text_update(1, "202", "pair")],
                    next_update_id: 2,
                },
            )
            .await
            .unwrap();
        manager.approve_pending(connector.id.clone()).await.unwrap();
        manager
            .accept_batch(
                connector.id.clone(),
                TelegramUpdateBatch {
                    updates: vec![text_update(2, "202", "retry me")],
                    next_update_id: 3,
                },
            )
            .await
            .unwrap();
        manager
            .process_pending_once(connector.id.clone())
            .await
            .unwrap();
        let message_count = state
            .read()
            .await
            .get_agent(&agent_id)
            .unwrap()
            .message_count;

        assert_eq!(
            manager
                .deliver_pending_once(connector.id.clone())
                .await
                .unwrap_err(),
            super::ConnectorManagerError::Transport
        );
        assert_eq!(
            manager.status(&connector.id).await,
            Some(ConnectorRuntimeStatus::Error)
        );
        manager.refresh_operational_status(&connector.id).await;
        assert_eq!(
            manager.status(&connector.id).await,
            Some(ConnectorRuntimeStatus::Error),
            "a successful poll/status refresh must not mask failed delivery"
        );
        assert!(manager
            .deliver_pending_once(connector.id.clone())
            .await
            .unwrap());
        assert_eq!(
            manager.status(&connector.id).await,
            Some(ConnectorRuntimeStatus::Ready)
        );
        assert!(!manager
            .process_pending_once(connector.id.clone())
            .await
            .unwrap());
        assert_eq!(
            state
                .read()
                .await
                .get_agent(&agent_id)
                .unwrap()
                .message_count,
            message_count
        );
        assert_eq!(transport.sent.lock().unwrap().len(), 1);
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn undelivered_outbox_backpressure_preserves_inbound_without_running_agent() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let manager = manager(
            Arc::clone(&state),
            Arc::new(InMemoryCredentialStore::default()),
            Arc::new(FakeTransport::default()),
        );
        let connector = manager
            .create(
                agent_id.clone(),
                TelegramBotToken::parse("42:backpressure").unwrap(),
            )
            .await
            .unwrap();
        manager.shutdown().await;
        manager
            .accept_batch(
                connector.id.clone(),
                TelegramUpdateBatch {
                    updates: vec![text_update(1, "202", "pair")],
                    next_update_id: 2,
                },
            )
            .await
            .unwrap();
        manager.approve_pending(connector.id.clone()).await.unwrap();
        manager
            .accept_batch(
                connector.id.clone(),
                TelegramUpdateBatch {
                    updates: vec![text_update(2, "202", "do not lose me")],
                    next_update_id: 3,
                },
            )
            .await
            .unwrap();
        {
            let mut state = state.write().await;
            for index in 0..super::MAX_UNDELIVERED_OUTBOUND {
                let id = format!("blocked-{index}");
                state.outbound.insert(
                    id.clone(),
                    crate::connectors::TelegramOutboundRecord {
                        id,
                        connector_id: connector.id.clone(),
                        agent_id: agent_id.clone(),
                        room_id: connector.room_id.clone(),
                        assistant_message_id: format!("assistant-{index}"),
                        text: "retry later".into(),
                        created_at_ms: index as u64,
                        delivered_at_ms: None,
                        attempts: 1,
                        delivery_state: crate::connectors::OutboundDeliveryState::Failed,
                    },
                );
            }
        }
        let message_count = state
            .read()
            .await
            .get_agent(&agent_id)
            .unwrap()
            .message_count;

        assert!(!manager
            .process_pending_once(connector.id.clone())
            .await
            .unwrap());
        assert_eq!(
            state.read().await.inbound[&(connector.id.clone(), 2)].processing_state,
            InboundProcessingState::Received
        );
        assert_eq!(
            state
                .read()
                .await
                .get_agent(&agent_id)
                .unwrap()
                .message_count,
            message_count
        );
        assert_eq!(
            manager.status(&connector.id).await,
            Some(ConnectorRuntimeStatus::Error)
        );
    }

    #[tokio::test]
    async fn inbound_backpressure_is_bounded_and_worker_recovers_without_losing_the_batch() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let delivery_attempts = Arc::new(Semaphore::new(0));
        let transport = Arc::new(InboundBackpressureTransport {
            enabled: AtomicBool::new(false),
            fail_delivery: AtomicBool::new(true),
            delivery_attempts: Arc::clone(&delivery_attempts),
            update: text_update(10_000, "202", "resume after pressure"),
        });
        let manager = manager(
            Arc::clone(&state),
            Arc::new(InMemoryCredentialStore::default()),
            transport.clone(),
        );
        let token = TelegramBotToken::parse("42:bounded-inbound").unwrap();
        let connector = manager
            .create(agent_id.clone(), token.clone())
            .await
            .unwrap();
        manager.stop_worker(&connector.id).await.unwrap();
        {
            let mut state = state.write().await;
            state
                .connectors
                .get_mut(&connector.id)
                .unwrap()
                .approved_chat = Some(text_update(0, "202", "pair").chat);
            for index in 0..super::MAX_LIVE_INBOUND {
                let update_id = index as i64 + 1;
                state.inbound.insert(
                    (connector.id.clone(), update_id),
                    crate::connectors::TelegramInboundRecord {
                        connector_id: connector.id.clone(),
                        update_id,
                        agent_id: agent_id.clone(),
                        room_id: connector.room_id.clone(),
                        normalized_text: format!("queued-{index}"),
                        sender: text_update(update_id, "202", "queued").sender,
                        chat: text_update(update_id, "202", "queued").chat,
                        received_at_ms: index as u64 + 1,
                        processing_state: InboundProcessingState::Received,
                        run_idempotency_key: format!(
                            "telegram:{}:update:{update_id}",
                            connector.id
                        ),
                    },
                );
            }
            state.outbound.insert(
                "pressure-blocker".into(),
                crate::connectors::TelegramOutboundRecord {
                    id: "pressure-blocker".into(),
                    connector_id: connector.id.clone(),
                    agent_id,
                    room_id: connector.room_id.clone(),
                    assistant_message_id: "pressure-assistant".into(),
                    text: "retry delivery".into(),
                    created_at_ms: 1,
                    delivered_at_ms: None,
                    attempts: 1,
                    delivery_state: crate::connectors::OutboundDeliveryState::Failed,
                },
            );
        }
        transport.enabled.store(true, Ordering::SeqCst);
        manager
            .start_worker(connector.id.clone(), token)
            .await
            .unwrap();

        tokio::time::timeout(Duration::from_secs(1), delivery_attempts.acquire())
            .await
            .expect("a pressured worker must keep retrying delivery")
            .unwrap()
            .forget();
        {
            let state = state.read().await;
            assert_eq!(state.connectors[&connector.id].next_update_id, 0);
            assert_eq!(
                state
                    .inbound
                    .values()
                    .filter(|record| {
                        record.connector_id == connector.id
                            && matches!(
                                record.processing_state,
                                InboundProcessingState::Received
                                    | InboundProcessingState::Processing
                            )
                    })
                    .count(),
                super::MAX_LIVE_INBOUND
            );
        }
        assert_eq!(
            manager.status(&connector.id).await,
            Some(ConnectorRuntimeStatus::Degraded)
        );

        transport.fail_delivery.store(false, Ordering::SeqCst);
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if state.read().await.connectors[&connector.id].next_update_id == 10_001 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("delivery recovery must resume ingestion at the unchanged cursor");
        let state = state.read().await;
        assert!(state.inbound.contains_key(&(connector.id.clone(), 10_000)));
        assert!(
            state
                .inbound
                .values()
                .filter(|record| {
                    record.connector_id == connector.id
                        && matches!(
                            record.processing_state,
                            InboundProcessingState::Received | InboundProcessingState::Processing
                        )
                })
                .count()
                <= super::MAX_LIVE_INBOUND
        );
        drop(state);
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn uncertain_delivery_credential_enters_reconciliation_without_sending() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let setup = manager(
            Arc::clone(&state),
            Arc::new(InMemoryCredentialStore::default()),
            Arc::new(FakeTransport::default()),
        );
        let connector = setup
            .create(agent_id, TelegramBotToken::parse("42:token").unwrap())
            .await
            .unwrap();
        setup.shutdown().await;
        setup
            .accept_batch(
                connector.id.clone(),
                TelegramUpdateBatch {
                    updates: vec![text_update(1, "101", "pair")],
                    next_update_id: 2,
                },
            )
            .await
            .unwrap();
        setup.approve_pending(connector.id.clone()).await.unwrap();
        setup
            .accept_batch(
                connector.id.clone(),
                TelegramUpdateBatch {
                    updates: vec![text_update(2, "101", "run")],
                    next_update_id: 3,
                },
            )
            .await
            .unwrap();
        setup
            .process_pending_once(connector.id.clone())
            .await
            .unwrap();

        let transport = Arc::new(FakeTransport::default());
        let delivery = manager(state, Arc::new(UncertainLoadStore), transport.clone());
        assert_eq!(
            delivery
                .deliver_pending_once(connector.id.clone())
                .await
                .unwrap_err(),
            super::ConnectorManagerError::CredentialStateUncertain
        );
        assert_eq!(
            delivery.status(&connector.id).await,
            Some(super::ConnectorRuntimeStatus::Reconciling)
        );
        assert!(transport.sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn create_starts_one_worker_restart_replaces_it_and_shutdown_joins_it() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let manager = manager(
            state,
            Arc::new(InMemoryCredentialStore::default()),
            Arc::new(FakeTransport::default()),
        );
        let connector = manager
            .create(
                agent_id,
                TelegramBotToken::parse("42:worker-token").unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(manager.worker_count().await, 1);

        manager.restart(connector.id.clone()).await.unwrap();
        assert_eq!(manager.worker_count().await, 1);

        manager.shutdown().await;
        assert_eq!(manager.worker_count().await, 0);
    }

    #[tokio::test]
    async fn concurrent_restart_and_shutdown_cannot_resurrect_a_worker() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let credentials = Arc::new(GateLoadCredentialStore {
            inner: InMemoryCredentialStore::default(),
            gate_load: AtomicBool::new(false),
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        });
        let manager = manager(
            state,
            credentials.clone(),
            Arc::new(FakeTransport::default()),
        );
        let connector = manager
            .create(
                agent_id,
                TelegramBotToken::parse("42:worker-token").unwrap(),
            )
            .await
            .unwrap();
        credentials.gate_load.store(true, Ordering::SeqCst);

        let restarting_manager = manager.clone();
        let connector_id = connector.id.clone();
        let restart = tokio::spawn(async move { restarting_manager.restart(connector_id).await });
        entered
            .acquire()
            .await
            .expect("restart should reach credential load")
            .forget();
        let shutdown_manager = manager.clone();
        let shutdown = tokio::spawn(async move { shutdown_manager.shutdown().await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        let shutdown_waited_for_restart = !shutdown.is_finished();
        release.add_permits(1);
        let restart_result = restart.await.unwrap();
        assert!(
            restart_result.is_ok()
                || restart_result == Err(super::ConnectorManagerError::WorkerStopped)
        );
        shutdown.await.unwrap();
        let remaining_workers = manager.worker_count().await;
        manager.shutdown().await;

        assert!(shutdown_waited_for_restart);
        assert_eq!(remaining_workers, 0);
    }

    #[tokio::test]
    async fn naturally_finished_worker_is_pruned_from_the_registry() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let manager = manager(
            Arc::clone(&state),
            Arc::new(InMemoryCredentialStore::default()),
            Arc::new(FakeTransport::default()),
        );
        let connector = manager
            .create(
                agent_id,
                TelegramBotToken::parse("42:worker-token").unwrap(),
            )
            .await
            .unwrap();
        state
            .write()
            .await
            .connectors
            .get_mut(&connector.id)
            .unwrap()
            .enabled = false;

        let pruned = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if manager.worker_count().await == 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok();
        manager.shutdown().await;

        assert!(pruned);
    }

    #[tokio::test]
    async fn shutdown_is_terminal_for_later_restart_requests() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let manager = manager(
            state,
            Arc::new(InMemoryCredentialStore::default()),
            Arc::new(FakeTransport::default()),
        );
        let connector = manager
            .create(
                agent_id,
                TelegramBotToken::parse("42:shutdown-token").unwrap(),
            )
            .await
            .unwrap();

        manager.shutdown().await;

        assert_eq!(
            manager.restart(connector.id).await.unwrap_err(),
            super::ConnectorManagerError::WorkerStopped
        );
        assert_eq!(manager.worker_count().await, 0);
    }

    #[tokio::test]
    async fn dropping_manager_does_not_leave_a_worker_registry_cycle() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let manager = manager(
            state,
            Arc::new(InMemoryCredentialStore::default()),
            Arc::new(FakeTransport::default()),
        );
        manager
            .create(agent_id, TelegramBotToken::parse("42:drop-token").unwrap())
            .await
            .unwrap();
        let registry = Arc::downgrade(&manager.workers);

        drop(manager);

        tokio::time::timeout(Duration::from_secs(1), async {
            while registry.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("worker must not strongly own its manager registry");
    }

    #[tokio::test]
    async fn dropping_last_manager_owner_cancels_the_in_flight_poll() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let entered = Arc::new(Semaphore::new(0));
        let exited = Arc::new(Semaphore::new(0));
        let manager = manager(
            state,
            Arc::new(InMemoryCredentialStore::default()),
            Arc::new(DropAwarePollingTransport {
                entered: Arc::clone(&entered),
                exited: Arc::clone(&exited),
            }),
        );
        manager
            .create(agent_id, TelegramBotToken::parse("42:drop-token").unwrap())
            .await
            .unwrap();
        entered.acquire().await.unwrap().forget();
        let registry_observer = Arc::clone(&manager.workers);

        drop(manager);

        tokio::time::timeout(Duration::from_secs(1), exited.acquire())
            .await
            .expect("dropping the last public manager owner must cancel polling")
            .unwrap()
            .forget();
        drop(registry_observer);
    }

    #[tokio::test]
    async fn restored_connector_without_a_credential_is_safe_and_does_not_block_startup() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let credentials = Arc::new(InMemoryCredentialStore::default());
        let first = manager(
            Arc::clone(&state),
            credentials.clone(),
            Arc::new(FakeTransport::default()),
        );
        let connector = first
            .create(
                agent_id,
                TelegramBotToken::parse("42:missing-token").unwrap(),
            )
            .await
            .unwrap();
        first.shutdown().await;
        credentials.delete(&connector.id).await.unwrap();

        let restored = manager(state, credentials, Arc::new(FakeTransport::default()));
        restored.start_restored().await;
        assert_eq!(restored.worker_count().await, 0);
        assert_eq!(
            restored.status(&connector.id).await,
            Some(super::ConnectorRuntimeStatus::CredentialRequired)
        );
        restored.shutdown().await;
    }

    #[tokio::test]
    async fn restored_connector_without_credential_can_be_repaired_by_replacement() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let credentials = Arc::new(InMemoryCredentialStore::default());
        let first = manager(
            Arc::clone(&state),
            credentials.clone(),
            Arc::new(FakeTransport::default()),
        );
        let connector = first
            .create(agent_id, TelegramBotToken::parse("42:lost-token").unwrap())
            .await
            .unwrap();
        first.shutdown().await;
        credentials.delete(&connector.id).await.unwrap();
        let manager = manager(
            Arc::clone(&state),
            credentials.clone(),
            Arc::new(FakeTransport::default()),
        );
        manager.start_restored().await;
        assert_eq!(
            manager.status(&connector.id).await,
            Some(ConnectorRuntimeStatus::CredentialRequired)
        );

        let repaired = manager
            .replace_token(
                connector.id.clone(),
                TelegramBotToken::parse("84:repair-token").unwrap(),
            )
            .await
            .expect("a supplied verified token should repair missing credentials");

        assert_eq!(repaired.bot.id, "84");
        assert_eq!(manager.worker_count().await, 1);
        assert_eq!(
            credentials
                .load(&connector.id)
                .await
                .unwrap()
                .unwrap()
                .expose(),
            "84:repair-token"
        );
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn restored_connector_without_credential_can_be_deleted() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let credentials = Arc::new(InMemoryCredentialStore::default());
        let first = manager(
            Arc::clone(&state),
            credentials.clone(),
            Arc::new(FakeTransport::default()),
        );
        let connector = first
            .create(agent_id, TelegramBotToken::parse("42:lost-token").unwrap())
            .await
            .unwrap();
        first.shutdown().await;
        credentials.delete(&connector.id).await.unwrap();
        let manager = manager(
            Arc::clone(&state),
            credentials,
            Arc::new(FakeTransport::default()),
        );
        manager.start_restored().await;

        manager
            .delete(connector.id.clone())
            .await
            .expect("verified credential absence should not block metadata deletion");

        assert!(!state.read().await.connectors[&connector.id].is_active());
        assert_eq!(manager.status(&connector.id).await, None);
    }

    #[tokio::test]
    async fn replacement_load_failure_keeps_the_previous_worker_and_state() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let credentials = Arc::new(FailingCredentialStore::default());
        let manager = manager(
            Arc::clone(&state),
            credentials.clone(),
            Arc::new(FakeTransport::default()),
        );
        let connector = manager
            .create(agent_id, TelegramBotToken::parse("42:old-token").unwrap())
            .await
            .unwrap();
        credentials.fail_next_load.store(true, Ordering::SeqCst);

        assert_eq!(
            manager
                .replace_token(
                    connector.id.clone(),
                    TelegramBotToken::parse("84:new-token").unwrap(),
                )
                .await
                .unwrap_err(),
            super::ConnectorManagerError::Credential
        );
        assert_eq!(manager.worker_count().await, 1);
        assert_eq!(
            manager.status(&connector.id).await,
            Some(super::ConnectorRuntimeStatus::Pairing)
        );
        assert_eq!(state.read().await.connectors[&connector.id], connector);
        manager.stop_worker(&connector.id).await.unwrap();
    }

    #[tokio::test]
    async fn replacement_put_failure_restores_the_previous_worker_token_and_status() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let credentials = Arc::new(FailingCredentialStore::default());
        let manager = manager(
            Arc::clone(&state),
            credentials.clone(),
            Arc::new(FakeTransport::default()),
        );
        let connector = manager
            .create(agent_id, TelegramBotToken::parse("42:old-token").unwrap())
            .await
            .unwrap();
        credentials.fail_next_put.store(true, Ordering::SeqCst);

        assert_eq!(
            manager
                .replace_token(
                    connector.id.clone(),
                    TelegramBotToken::parse("84:new-token").unwrap(),
                )
                .await
                .unwrap_err(),
            super::ConnectorManagerError::Credential
        );
        assert_eq!(manager.worker_count().await, 1);
        assert_eq!(
            manager.status(&connector.id).await,
            Some(super::ConnectorRuntimeStatus::Pairing)
        );
        assert_eq!(
            credentials
                .load(&connector.id)
                .await
                .unwrap()
                .unwrap()
                .expose(),
            "42:old-token"
        );
        assert_eq!(state.read().await.connectors[&connector.id], connector);
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn failed_replacement_never_lets_old_worker_deliver_with_uncommitted_new_token() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let fixed_worker_delivery = Arc::new(AtomicBool::new(false));
        let worker_load_entered = Arc::new(Semaphore::new(0));
        let replacement_put_done = Arc::new(Semaphore::new(0));
        let send_entered = Arc::new(Semaphore::new(0));
        let release_send = Arc::new(Semaphore::new(0));
        let credentials = Arc::new(ReplacementRaceCredentialStore {
            inner: InMemoryCredentialStore::default(),
            fixed_worker_delivery: Arc::clone(&fixed_worker_delivery),
            worker_load_entered: Arc::clone(&worker_load_entered),
            replacement_put_done: Arc::clone(&replacement_put_done),
            load_calls: AtomicUsize::new(0),
            put_calls: AtomicUsize::new(0),
        });
        let transport = Arc::new(ReplacementRaceTransport {
            fixed_worker_delivery: Arc::clone(&fixed_worker_delivery),
            send_entered: Arc::clone(&send_entered),
            release_send: Arc::clone(&release_send),
            gate_first_send: AtomicBool::new(true),
            sent_tokens: Mutex::new(Vec::new()),
        });
        let manager = manager(Arc::clone(&state), credentials, transport.clone());
        let old_token = TelegramBotToken::parse("42:old-token").unwrap();
        let connector = manager
            .create(agent_id.clone(), old_token.clone())
            .await
            .unwrap();
        manager.stop_worker(&connector.id).await.unwrap();
        {
            let mut state = state.write().await;
            state
                .connectors
                .get_mut(&connector.id)
                .unwrap()
                .approved_chat = Some(TelegramChatMetadata {
                id: "202".into(),
                kind: TelegramChatKind::Private,
                title: None,
                username: Some("operator".into()),
            });
            state.outbound.insert(
                "replacement-race-outbound".into(),
                crate::connectors::TelegramOutboundRecord {
                    id: "replacement-race-outbound".into(),
                    connector_id: connector.id.clone(),
                    agent_id,
                    room_id: connector.room_id.clone(),
                    assistant_message_id: "replacement-race-assistant".into(),
                    text: "old worker response".into(),
                    created_at_ms: super::now_ms(),
                    delivered_at_ms: None,
                    attempts: 0,
                    delivery_state: crate::connectors::OutboundDeliveryState::Pending,
                },
            );
        }
        let temporary = std::env::temp_dir().join(format!(
            "anima-replacement-token-race-{}-{}",
            std::process::id(),
            super::now_ms()
        ));
        std::fs::create_dir_all(&temporary).unwrap();
        let invalid_path = temporary.join("cannot-replace-directory");
        std::fs::create_dir(&invalid_path).unwrap();
        state.write().await.set_control_plane_store(Some(
            crate::control_plane_store::ControlPlaneStoreConfig::Json(invalid_path),
        ));
        manager
            .start_worker(connector.id.clone(), old_token)
            .await
            .unwrap();

        tokio::select! {
            permit = worker_load_entered.acquire() => permit.unwrap().forget(),
            permit = send_entered.acquire() => permit.unwrap().forget(),
        }
        let replacing = {
            let manager = manager.clone();
            let connector_id = connector.id.clone();
            tokio::spawn(async move {
                manager
                    .replace_token(
                        connector_id,
                        TelegramBotToken::parse("84:new-token").unwrap(),
                    )
                    .await
            })
        };
        replacement_put_done.acquire().await.unwrap().forget();
        send_entered.acquire().await.unwrap().forget();
        release_send.add_permits(1);

        assert_eq!(
            replacing.await.unwrap().unwrap_err(),
            super::ConnectorManagerError::Persistence
        );
        assert!(!transport.sent_tokens.lock().unwrap().is_empty());
        assert!(transport
            .sent_tokens
            .lock()
            .unwrap()
            .iter()
            .all(|token| token == "42:old-token"));

        manager.shutdown().await;
        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[tokio::test]
    async fn slow_failed_replacement_put_never_interrupts_the_previous_worker() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let credentials = Arc::new(GatePutCredentialStore {
            inner: InMemoryCredentialStore::default(),
            gate_put: AtomicBool::new(false),
            fail_put: AtomicBool::new(false),
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        });
        let transport = Arc::new(FakeTransport::default());
        let manager = manager(Arc::clone(&state), credentials.clone(), transport.clone());
        let connector = manager
            .create(agent_id, TelegramBotToken::parse("42:old-token").unwrap())
            .await
            .unwrap();
        credentials.gate_put.store(true, Ordering::SeqCst);
        credentials.fail_put.store(true, Ordering::SeqCst);

        let replacing_manager = manager.clone();
        let connector_id = connector.id.clone();
        let replacement = tokio::spawn(async move {
            replacing_manager
                .replace_token(
                    connector_id,
                    TelegramBotToken::parse("84:new-token").unwrap(),
                )
                .await
        });
        entered
            .acquire()
            .await
            .expect("replacement should enter the credential write")
            .forget();

        assert_eq!(
            manager.worker_count().await,
            1,
            "the old poller must remain registered throughout credential preflight"
        );
        let observed_polls = transport.poll_calls.load(Ordering::SeqCst);
        tokio::time::timeout(Duration::from_millis(500), async {
            while transport.poll_calls.load(Ordering::SeqCst) == observed_polls {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the old poller should keep polling while the vault write is blocked");
        assert_eq!(
            manager.status(&connector.id).await,
            Some(ConnectorRuntimeStatus::Pairing)
        );

        release.add_permits(1);
        assert_eq!(
            replacement.await.unwrap().unwrap_err(),
            super::ConnectorManagerError::Credential
        );
        assert_eq!(manager.worker_count().await, 1);
        assert_eq!(
            credentials
                .load(&connector.id)
                .await
                .unwrap()
                .unwrap()
                .expose(),
            "42:old-token"
        );
        assert_eq!(state.read().await.connectors[&connector.id], connector);
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn replacement_state_failure_restores_old_metadata_credential_and_worker() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let credentials = Arc::new(GatePutCredentialStore {
            inner: InMemoryCredentialStore::default(),
            gate_put: AtomicBool::new(false),
            fail_put: AtomicBool::new(false),
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        });
        let manager = manager(
            Arc::clone(&state),
            credentials.clone(),
            Arc::new(FakeTransport::default()),
        );
        let connector = manager
            .create(agent_id, TelegramBotToken::parse("42:old-token").unwrap())
            .await
            .unwrap();
        credentials.gate_put.store(true, Ordering::SeqCst);

        let replacing_manager = manager.clone();
        let connector_id = connector.id.clone();
        let replacement = tokio::spawn(async move {
            replacing_manager
                .replace_token(
                    connector_id,
                    TelegramBotToken::parse("84:new-token").unwrap(),
                )
                .await
        });
        entered
            .acquire()
            .await
            .expect("replacement should enter the credential write")
            .forget();
        state.write().await.connectors.remove(&connector.id);
        release.add_permits(1);

        assert_eq!(
            replacement.await.unwrap().unwrap_err(),
            super::ConnectorManagerError::ConnectorNotFound
        );
        assert_eq!(state.read().await.connectors[&connector.id], connector);
        assert_eq!(
            credentials
                .load(&connector.id)
                .await
                .unwrap()
                .unwrap()
                .expose(),
            "42:old-token"
        );
        assert_eq!(manager.worker_count().await, 1);
        assert_eq!(
            manager.status(&connector.id).await,
            Some(ConnectorRuntimeStatus::Pairing)
        );
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn replacement_persist_failure_restores_previous_vault_state_metadata_and_worker() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let credentials = Arc::new(InMemoryCredentialStore::default());
        let manager = manager(
            Arc::clone(&state),
            credentials.clone(),
            Arc::new(FakeTransport::default()),
        );
        let connector = manager
            .create(agent_id, TelegramBotToken::parse("42:old-token").unwrap())
            .await
            .unwrap();
        let invalid_path = invalid_snapshot_directory("replace");
        state.write().await.set_control_plane_store(Some(
            crate::control_plane_store::ControlPlaneStoreConfig::Json(invalid_path.clone()),
        ));

        assert_eq!(
            manager
                .replace_token(
                    connector.id.clone(),
                    TelegramBotToken::parse("84:new-token").unwrap(),
                )
                .await
                .unwrap_err(),
            super::ConnectorManagerError::Persistence
        );
        assert_eq!(manager.worker_count().await, 1);
        assert_eq!(
            manager.status(&connector.id).await,
            Some(super::ConnectorRuntimeStatus::Pairing)
        );
        assert_eq!(
            credentials
                .load(&connector.id)
                .await
                .unwrap()
                .unwrap()
                .expose(),
            "42:old-token"
        );
        assert_eq!(state.read().await.connectors[&connector.id], connector);
        manager.shutdown().await;
        std::fs::remove_dir_all(invalid_path).unwrap();
    }

    #[tokio::test]
    async fn replace_token_verifies_and_updates_bot_identity_without_changing_room() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let credentials = Arc::new(InMemoryCredentialStore::default());
        let manager = manager(
            Arc::clone(&state),
            credentials.clone(),
            Arc::new(FakeTransport::default()),
        );
        let connector = manager
            .create(agent_id, TelegramBotToken::parse("42:old-token").unwrap())
            .await
            .unwrap();

        let replaced = manager
            .replace_token(
                connector.id.clone(),
                TelegramBotToken::parse("99:new-token").unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(replaced.id, connector.id);
        assert_eq!(replaced.room_id, connector.room_id);
        assert_eq!(replaced.bot.id, "99");
        assert_eq!(
            credentials
                .load(&connector.id)
                .await
                .unwrap()
                .unwrap()
                .expose(),
            "99:new-token"
        );
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn agent_cleanup_tombstones_connectors_and_removes_agent_in_one_transaction() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let manager = manager(
            Arc::clone(&state),
            Arc::new(InMemoryCredentialStore::default()),
            Arc::new(FakeTransport::default()),
        );
        let connector = manager
            .create(
                agent_id.clone(),
                TelegramBotToken::parse("42:cleanup-token").unwrap(),
            )
            .await
            .unwrap();

        manager.delete_agent(agent_id.clone()).await.unwrap();
        assert!(!state.read().await.connectors[&connector.id].is_active());
        assert!(state.read().await.get_agent(&agent_id).is_none());
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn agent_cleanup_final_save_failure_restores_agent_connector_credential_and_worker() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let credentials = Arc::new(InMemoryCredentialStore::default());
        let manager = manager(
            Arc::clone(&state),
            credentials.clone(),
            Arc::new(FakeTransport::default()),
        );
        let connector = manager
            .create(
                agent_id.clone(),
                TelegramBotToken::parse("42:agent-delete-token").unwrap(),
            )
            .await
            .unwrap();
        let invalid_path = invalid_snapshot_directory("agent-delete");
        state.write().await.set_control_plane_store(Some(
            crate::control_plane_store::ControlPlaneStoreConfig::Json(invalid_path.clone()),
        ));

        assert_eq!(
            manager.delete_agent(agent_id.clone()).await.unwrap_err(),
            super::ConnectorManagerError::Persistence
        );

        assert!(state.read().await.get_agent(&agent_id).is_some());
        assert_eq!(state.read().await.connectors[&connector.id], connector);
        assert_eq!(
            credentials
                .load(&connector.id)
                .await
                .unwrap()
                .unwrap()
                .expose(),
            "42:agent-delete-token"
        );
        assert_eq!(manager.worker_count().await, 1);
        manager.shutdown().await;
        std::fs::remove_dir_all(invalid_path).unwrap();
    }

    #[tokio::test]
    async fn delete_persist_failure_restores_credential_metadata_status_and_worker() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let credentials = Arc::new(InMemoryCredentialStore::default());
        let manager = manager(
            Arc::clone(&state),
            credentials.clone(),
            Arc::new(FakeTransport::default()),
        );
        let connector = manager
            .create(agent_id, TelegramBotToken::parse("42:old-token").unwrap())
            .await
            .unwrap();
        let invalid_path = invalid_snapshot_directory("delete");
        state.write().await.set_control_plane_store(Some(
            crate::control_plane_store::ControlPlaneStoreConfig::Json(invalid_path.clone()),
        ));

        assert_eq!(
            manager.delete(connector.id.clone()).await.unwrap_err(),
            super::ConnectorManagerError::Persistence
        );
        assert_eq!(state.read().await.connectors[&connector.id], connector);
        assert_eq!(
            credentials
                .load(&connector.id)
                .await
                .unwrap()
                .unwrap()
                .expose(),
            "42:old-token"
        );
        assert_eq!(manager.worker_count().await, 1);
        assert_eq!(
            manager.status(&connector.id).await,
            Some(super::ConnectorRuntimeStatus::Pairing)
        );
        manager.shutdown().await;
        std::fs::remove_dir_all(invalid_path).unwrap();
    }

    #[tokio::test]
    async fn uncertain_credential_delete_stops_worker_and_preserves_metadata_for_reconciliation() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let manager = manager(
            Arc::clone(&state),
            Arc::new(UncertainDeleteStore::default()),
            Arc::new(FakeTransport::default()),
        );
        let connector = manager
            .create(
                agent_id,
                TelegramBotToken::parse("42:uncertain-delete").unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            manager.delete(connector.id.clone()).await.unwrap_err(),
            super::ConnectorManagerError::CredentialStateUncertain
        );
        assert!(state.read().await.connectors[&connector.id].is_active());
        assert_eq!(manager.worker_count().await, 0);
        assert_eq!(
            manager.status(&connector.id).await,
            Some(super::ConnectorRuntimeStatus::Reconciling)
        );
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn deletion_archives_completed_history_purges_pending_work_and_disables_schedules() {
        use crate::connectors::{OutboundDeliveryState, TelegramOutboundRecord};
        use crate::schedules::{ScheduleTarget, ScheduleTrigger, ScheduledPromptRecord};

        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let manager = manager(
            Arc::clone(&state),
            Arc::new(InMemoryCredentialStore::default()),
            Arc::new(FakeTransport::default()),
        );
        let connector = manager
            .create(
                agent_id.clone(),
                TelegramBotToken::parse("42:archive-token").unwrap(),
            )
            .await
            .unwrap();
        manager.stop_worker(&connector.id).await.unwrap();
        manager
            .accept_batch(
                connector.id.clone(),
                TelegramUpdateBatch {
                    updates: vec![text_update(1, "202", "pair")],
                    next_update_id: 2,
                },
            )
            .await
            .unwrap();
        manager.approve_pending(connector.id.clone()).await.unwrap();
        manager
            .accept_batch(
                connector.id.clone(),
                TelegramUpdateBatch {
                    updates: (2..=5)
                        .map(|id| text_update(id, "202", &format!("message-{id}")))
                        .collect(),
                    next_update_id: 6,
                },
            )
            .await
            .unwrap();
        let now = super::now_ms();
        {
            let mut guard = state.write().await;
            for (update_id, processing_state) in [
                (2, InboundProcessingState::Processed),
                (3, InboundProcessingState::Rejected),
                (4, InboundProcessingState::Received),
                (5, InboundProcessingState::Processing),
            ] {
                guard
                    .inbound
                    .get_mut(&(connector.id.clone(), update_id))
                    .unwrap()
                    .processing_state = processing_state;
            }
            for (id, delivery_state, delivered_at_ms) in [
                ("delivered", OutboundDeliveryState::Delivered, Some(now)),
                ("pending", OutboundDeliveryState::Pending, None),
                ("failed", OutboundDeliveryState::Failed, None),
            ] {
                guard.outbound.insert(
                    id.into(),
                    TelegramOutboundRecord {
                        id: id.into(),
                        connector_id: connector.id.clone(),
                        agent_id: agent_id.clone(),
                        room_id: connector.room_id.clone(),
                        assistant_message_id: format!("assistant-{id}"),
                        text: id.into(),
                        created_at_ms: now,
                        delivered_at_ms,
                        attempts: 1,
                        delivery_state,
                    },
                );
            }
            guard.schedules.insert(
                "telegram-schedule".into(),
                ScheduledPromptRecord {
                    id: "telegram-schedule".into(),
                    import_idempotency_key: None,
                    agent_id,
                    prompt: "check in".into(),
                    trigger: ScheduleTrigger::Interval {
                        interval_ms: 60_000,
                    },
                    enabled: true,
                    target: ScheduleTarget::Connector {
                        connector_id: connector.id.clone(),
                    },
                    next_due_at_ms: now + 60_000,
                    last_fired: None,
                    last_safe_outcome: None,
                    created_at_ms: now,
                    updated_at_ms: now,
                },
            );
        }

        manager.delete(connector.id.clone()).await.unwrap();
        let guard = state.read().await;
        assert!(guard.inbound.contains_key(&(connector.id.clone(), 2)));
        assert!(guard.inbound.contains_key(&(connector.id.clone(), 3)));
        assert!(!guard.inbound.contains_key(&(connector.id.clone(), 4)));
        assert!(!guard.inbound.contains_key(&(connector.id.clone(), 5)));
        assert!(guard.outbound.contains_key("delivered"));
        assert!(!guard.outbound.contains_key("pending"));
        assert!(!guard.outbound.contains_key("failed"));
        assert!(!guard.schedules["telegram-schedule"].enabled);
        drop(guard);
        manager.shutdown().await;
    }

    #[test]
    fn outbox_compaction_removes_old_delivered_but_never_pending_or_failed() {
        use crate::connectors::{OutboundDeliveryState, TelegramOutboundRecord};
        use std::collections::HashMap;

        let now = 10 * 24 * 60 * 60 * 1000;
        let mut outbox = HashMap::new();
        for (id, state, delivered_at_ms) in [
            ("old", OutboundDeliveryState::Delivered, Some(1)),
            ("recent", OutboundDeliveryState::Delivered, Some(now - 1)),
            ("pending", OutboundDeliveryState::Pending, None),
            ("failed", OutboundDeliveryState::Failed, None),
        ] {
            outbox.insert(
                id.to_string(),
                TelegramOutboundRecord {
                    id: id.into(),
                    connector_id: "connector".into(),
                    agent_id: "agent".into(),
                    room_id: "room".into(),
                    assistant_message_id: format!("message-{id}"),
                    text: id.into(),
                    created_at_ms: 1,
                    delivered_at_ms,
                    attempts: 1,
                    delivery_state: state,
                },
            );
        }

        super::compact_delivered_outbox(&mut outbox, "connector", now);
        assert!(!outbox.contains_key("old"));
        assert!(outbox.contains_key("recent"));
        assert!(outbox.contains_key("pending"));
        assert!(outbox.contains_key("failed"));
    }

    #[test]
    fn outbox_compaction_caps_only_the_selected_connector() {
        use crate::connectors::{OutboundDeliveryState, TelegramOutboundRecord};
        use std::collections::HashMap;

        let now = super::DELIVERED_RETENTION_MS + 10_000;
        let mut outbox = HashMap::new();
        for connector_id in ["selected", "other"] {
            for index in 0..1_002_u64 {
                let id = format!("{connector_id}-{index}");
                outbox.insert(
                    id.clone(),
                    TelegramOutboundRecord {
                        id: id.clone(),
                        connector_id: connector_id.into(),
                        agent_id: format!("agent-{connector_id}"),
                        room_id: format!("room-{connector_id}"),
                        assistant_message_id: format!("message-{id}"),
                        text: id,
                        created_at_ms: now - 1_000 + index,
                        delivered_at_ms: Some(now - 1_000 + index),
                        attempts: 1,
                        delivery_state: OutboundDeliveryState::Delivered,
                    },
                );
            }
        }
        for state in [
            OutboundDeliveryState::Pending,
            OutboundDeliveryState::Failed,
        ] {
            let id = format!("selected-{state:?}");
            outbox.insert(
                id.clone(),
                TelegramOutboundRecord {
                    id: id.clone(),
                    connector_id: "selected".into(),
                    agent_id: "agent-selected".into(),
                    room_id: "room-selected".into(),
                    assistant_message_id: format!("message-{id}"),
                    text: id,
                    created_at_ms: now,
                    delivered_at_ms: None,
                    attempts: 1,
                    delivery_state: state,
                },
            );
        }

        super::compact_delivered_outbox(&mut outbox, "selected", now);

        assert_eq!(
            outbox
                .values()
                .filter(|record| {
                    record.connector_id == "selected"
                        && record.delivery_state == OutboundDeliveryState::Delivered
                })
                .count(),
            1_000
        );
        assert_eq!(
            outbox
                .values()
                .filter(|record| record.connector_id == "other")
                .count(),
            1_002
        );
        assert_eq!(
            outbox
                .values()
                .filter(|record| {
                    record.connector_id == "selected"
                        && record.delivery_state != OutboundDeliveryState::Delivered
                })
                .count(),
            2
        );
    }

    #[test]
    fn terminal_inbound_compaction_is_bounded_per_connector_and_keeps_live_records() {
        use std::collections::HashMap;

        let sender = TelegramSenderMetadata {
            id: "sender".into(),
            username: None,
            display_name: None,
        };
        let chat = TelegramChatMetadata {
            id: "chat".into(),
            kind: TelegramChatKind::Private,
            title: None,
            username: None,
        };
        let mut inbound = HashMap::new();
        for connector_id in ["selected", "other"] {
            for update_id in 0..1_002_i64 {
                inbound.insert(
                    (connector_id.to_string(), update_id),
                    crate::connectors::TelegramInboundRecord {
                        connector_id: connector_id.into(),
                        update_id,
                        agent_id: format!("agent-{connector_id}"),
                        room_id: format!("room-{connector_id}"),
                        normalized_text: "done".into(),
                        sender: sender.clone(),
                        chat: chat.clone(),
                        received_at_ms: update_id as u64,
                        processing_state: InboundProcessingState::Processed,
                        run_idempotency_key: format!("{connector_id}-{update_id}"),
                    },
                );
            }
        }
        inbound.insert(
            ("selected".into(), 2_000),
            crate::connectors::TelegramInboundRecord {
                connector_id: "selected".into(),
                update_id: 2_000,
                agent_id: "agent-selected".into(),
                room_id: "room-selected".into(),
                normalized_text: "live".into(),
                sender,
                chat,
                received_at_ms: 2_000,
                processing_state: InboundProcessingState::Processing,
                run_idempotency_key: "selected-live".into(),
            },
        );

        super::compact_terminal_inbound(&mut inbound, "selected");

        assert_eq!(
            inbound
                .values()
                .filter(|record| {
                    record.connector_id == "selected"
                        && matches!(
                            record.processing_state,
                            InboundProcessingState::Processed | InboundProcessingState::Rejected
                        )
                })
                .count(),
            super::MAX_RETAINED_TERMINAL_INBOUND
        );
        assert_eq!(
            inbound
                .values()
                .filter(|record| record.connector_id == "other")
                .count(),
            1_002
        );
        assert_eq!(
            inbound[&("selected".into(), 2_000)].processing_state,
            InboundProcessingState::Processing
        );
    }

    #[test]
    fn poll_backoff_is_exponential_and_capped() {
        let mut delay = super::POLL_RETRY_INITIAL;
        assert_eq!(delay, Duration::from_millis(100));
        delay = super::next_poll_backoff(delay);
        assert_eq!(delay, Duration::from_millis(200));
        for _ in 0..20 {
            delay = super::next_poll_backoff(delay);
        }
        assert_eq!(delay, super::POLL_RETRY_MAX);
        assert_eq!(super::next_poll_backoff(delay), super::POLL_RETRY_MAX);
    }

    fn invalid_snapshot_directory(label: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "anima-connector-{label}-invalid-{}-{}",
            std::process::id(),
            super::now_ms()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn text_update(update_id: i64, chat_id: &str, text: &str) -> TelegramTextUpdate {
        TelegramTextUpdate {
            update_id,
            text: text.into(),
            sender: TelegramSenderMetadata {
                id: "sender-1".into(),
                username: Some("operator".into()),
                display_name: Some("Operator".into()),
            },
            chat: TelegramChatMetadata {
                id: chat_id.into(),
                kind: TelegramChatKind::Private,
                title: None,
                username: Some("operator".into()),
            },
        }
    }

    fn state_with_agent() -> SharedDaemonState {
        let mut state = DaemonState::new();
        state.create_agent(test_config()).unwrap();
        Arc::new(RwLock::new(state))
    }

    fn manager(
        state: SharedDaemonState,
        credentials: Arc<dyn ConnectorCredentialStore>,
        transport: Arc<dyn TelegramTransport>,
    ) -> ConnectorManager {
        let runs = AgentRunCoordinator::new(Arc::clone(&state), Arc::new(Semaphore::new(4)));
        ConnectorManager::new(state, runs, credentials, transport)
    }

    async fn wait_for_status(
        manager: &ConnectorManager,
        connector_id: &str,
        expected: ConnectorRuntimeStatus,
        timeout: Duration,
    ) -> bool {
        tokio::time::timeout(timeout, async {
            loop {
                if manager.status(connector_id).await == Some(expected) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok()
    }

    fn test_config() -> AgentConfig {
        AgentConfig {
            name: "operator".into(),
            model: "deterministic".into(),
            bio: None,
            lore: None,
            knowledge: None,
            topics: None,
            adjectives: None,
            style: None,
            provider: None,
            system: None,
            tools: None,
            plugins: None,
            settings: Some(AgentSettings::default()),
        }
    }
}
