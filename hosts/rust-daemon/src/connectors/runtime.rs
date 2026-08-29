use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anima_core::{Content, DataValue, MessageRole, TaskStatus};
use async_trait::async_trait;
use tokio::sync::{oneshot, watch, Mutex};
use tokio::task::JoinHandle;

use super::credentials::{ConnectorCredentialStore, CredentialStoreError, TelegramBotToken};
use super::telegram::{
    TelegramClient, TelegramSentMessage, TelegramTransportError, TelegramUpdateBatch,
};
use super::{
    TelegramBotIdentity, TelegramConnectorRecord, TelegramInboundRecord, TelegramPendingPairing,
};
use crate::agent_runs::{AgentRunCoordinator, AgentRunRequest, RunRoom};
use crate::app::SharedDaemonState;
use crate::connectors::{InboundProcessingState, OutboundDeliveryState, TelegramOutboundRecord};
use crate::routes::ApiError;
use crate::schedules::ScheduleTarget;

static CONNECTOR_ID_SEQUENCE: AtomicU64 = AtomicU64::new(1);
const DELIVERED_RETENTION_MS: u64 = 7 * 24 * 60 * 60 * 1000;
const MAX_RETAINED_DELIVERED: usize = 1000;
const POLL_RETRY_INITIAL: Duration = Duration::from_millis(100);
const POLL_RETRY_MAX: Duration = Duration::from_secs(30);
const PAIRING_CANDIDATE_TTL_MS: u64 = 10 * 60 * 1000;
const MAX_RETAINED_TERMINAL_INBOUND: usize = 1000;
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
    Reconciling,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConnectorManagerError {
    AgentNotFound,
    ConnectorNotFound,
    AgentAlreadyConnected,
    Transport,
    Credential,
    CredentialStateUncertain,
    Persistence,
    ConflictingUpdate,
    WorkerStopped,
}

impl fmt::Display for ConnectorManagerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::AgentNotFound => "agent not found",
            Self::ConnectorNotFound => "connector not found",
            Self::AgentAlreadyConnected => "agent already has an active Telegram connector",
            Self::Transport => "Telegram transport failed",
            Self::Credential => "credential vault operation failed",
            Self::CredentialStateUncertain => "credential vault state requires reconciliation",
            Self::Persistence => "connector state persistence failed",
            Self::ConflictingUpdate => "Telegram update conflicts with durable history",
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
    worker_generation: Arc<AtomicU64>,
    closing: Arc<AtomicBool>,
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
        Self {
            state,
            runs,
            credentials,
            transport,
            lifecycle_lock: Arc::new(Mutex::new(())),
            mutation_lock,
            statuses: Arc::new(Mutex::new(HashMap::new())),
            workers: Arc::new(Mutex::new(HashMap::new())),
            worker_generation: Arc::new(AtomicU64::new(1)),
            closing: Arc::new(AtomicBool::new(false)),
        }
    }

    fn ensure_open(&self) -> Result<(), ConnectorManagerError> {
        if self.closing.load(Ordering::SeqCst) {
            Err(ConnectorManagerError::WorkerStopped)
        } else {
            Ok(())
        }
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
            .map_err(|_| ConnectorManagerError::Transport)?;
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

        let worker_token = token.clone();
        if let Err(error) = self.credentials.put(&id, token).await {
            let mapped = map_credential_error(error);
            if mapped == ConnectorManagerError::CredentialStateUncertain {
                self.retain_create_reconciliation_marker(connector).await;
                return Err(mapped);
            }
            return Err(mapped);
        }

        let _mutation = self.mutation_lock.lock().await;
        let persist = {
            let mut state = self.state.write().await;
            if state.get_agent(&connector.agent_id).is_none() {
                drop(state);
                drop(_mutation);
                return self
                    .cleanup_failed_create(connector, ConnectorManagerError::AgentNotFound)
                    .await;
            }
            if state.connectors.values().any(|candidate| {
                candidate.agent_id == connector.agent_id && candidate.deleted_at_ms.is_none()
            }) {
                drop(state);
                drop(_mutation);
                return self
                    .cleanup_failed_create(connector, ConnectorManagerError::AgentAlreadyConnected)
                    .await;
            }
            state.connectors.insert(id.clone(), connector.clone());
            state.control_plane_persist_request()
        };
        if persist.save().await.is_err() {
            self.state.write().await.connectors.remove(&id);
            drop(_mutation);
            return self
                .cleanup_failed_create(connector, ConnectorManagerError::Persistence)
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

    async fn cleanup_failed_create(
        &self,
        connector: TelegramConnectorRecord,
        original_error: ConnectorManagerError,
    ) -> Result<TelegramConnectorRecord, ConnectorManagerError> {
        if self.credentials.delete(&connector.id).await.is_ok() {
            return Err(original_error);
        }
        self.retain_create_reconciliation_marker(connector).await;
        Err(ConnectorManagerError::CredentialStateUncertain)
    }

    async fn retain_create_reconciliation_marker(&self, mut connector: TelegramConnectorRecord) {
        let now = now_ms();
        connector.enabled = false;
        connector.deleted_at_ms = Some(now);
        connector.pending_pairing = None;
        connector.updated_at_ms = now;
        let connector_id = connector.id.clone();
        let _mutation = self.mutation_lock.lock().await;
        let persist = {
            let mut state = self.state.write().await;
            state.connectors.insert(connector_id.clone(), connector);
            state.control_plane_persist_request()
        };
        let _ = persist.save().await;
        self.statuses
            .lock()
            .await
            .insert(connector_id, ConnectorRuntimeStatus::Reconciling);
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
            .map_err(|_| ConnectorManagerError::Transport)?;
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
        let manager = self.clone();
        tokio::spawn(async move { manager.approve_pending_owned(connector_id).await })
            .await
            .map_err(|_| ConnectorManagerError::WorkerStopped)?
    }

    async fn approve_pending_owned(
        &self,
        connector_id: String,
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
            return Err(ConnectorManagerError::ConnectorNotFound);
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
                .ok_or(ConnectorManagerError::ConnectorNotFound)?;
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
        let delivery = self
            .transport
            .send_message(&token, &chat_id, &outbound.text)
            .await;
        let _mutation = self.mutation_lock.lock().await;
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

    async fn start_worker(
        &self,
        connector_id: String,
        token: TelegramBotToken,
    ) -> Result<(), ConnectorManagerError> {
        self.stop_worker(&connector_id).await?;
        let (cancel, receiver) = watch::channel(false);
        let (start, started) = oneshot::channel();
        let generation = self.worker_generation.fetch_add(1, Ordering::Relaxed);
        let manager = self.detached_worker_context();
        let worker_registry = Arc::downgrade(&self.workers);
        let worker_connector_id = connector_id.clone();
        let join = tokio::spawn(async move {
            if started.await.is_ok() {
                manager
                    .worker_loop(worker_connector_id.clone(), token, receiver)
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
            worker_generation: Arc::clone(&self.worker_generation),
            closing: Arc::clone(&self.closing),
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
    ) {
        let mut poll_backoff = POLL_RETRY_INITIAL;
        loop {
            if *cancel.borrow() {
                return;
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
                    if wait_or_cancel(&mut cancel, poll_backoff).await {
                        return;
                    }
                    poll_backoff = next_poll_backoff(poll_backoff);
                    continue;
                }
            };
            let had_updates = !batch.updates.is_empty();
            if had_updates || batch.next_update_id != offset {
                if self
                    .accept_batch(connector_id.clone(), batch)
                    .await
                    .is_err()
                {
                    self.statuses
                        .lock()
                        .await
                        .insert(connector_id, ConnectorRuntimeStatus::Error);
                    return;
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
                match self.deliver_pending_once(connector_id.clone()).await {
                    Ok(true) => continue,
                    Ok(false) => break,
                    Err(ConnectorManagerError::Transport) => break,
                    Err(_) => return,
                }
            }
            if !had_updates && wait_or_cancel(&mut cancel, Duration::from_millis(100)).await {
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

fn map_credential_error(error: CredentialStoreError) -> ConnectorManagerError {
    match error {
        CredentialStoreError::CredentialStateUncertain => {
            ConnectorManagerError::CredentialStateUncertain
        }
        _ => ConnectorManagerError::Credential,
    }
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

async fn wait_or_cancel(cancel: &mut watch::Receiver<bool>, duration: Duration) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(duration) => false,
        changed = cancel.changed() => {
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
        AgentConfig, AgentConfigUpdate, AgentSettings, Content, ModelAdapter, ModelGenerateRequest,
        ModelGenerateResponse, ModelStopReason, TokenUsage,
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
    }

    struct OneShotUncertainDeleteStore {
        inner: InMemoryCredentialStore,
        fail_next_delete: AtomicBool,
    }

    struct GateModelAdapter {
        entered: Arc<Semaphore>,
        release: Arc<Semaphore>,
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

        async fn delete(&self, _connector_id: &str) -> Result<(), CredentialStoreError> {
            Err(CredentialStoreError::CredentialStateUncertain)
        }
    }

    #[async_trait]
    impl ConnectorCredentialStore for OneShotUncertainDeleteStore {
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
    async fn create_does_not_publish_metadata_before_verified_vault_write() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let credentials = Arc::new(GatePutUncertainDeleteStore {
            inner: InMemoryCredentialStore::default(),
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
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

        assert!(state.read().await.connectors.is_empty());
        release.add_permits(1);
        let connector = creating.await.unwrap().unwrap();
        assert!(state.read().await.connectors[&connector.id].is_active());

        manager.shutdown().await;
    }

    #[tokio::test]
    async fn failed_active_create_publish_leaves_tombstone_then_restart_cleans_credential() {
        let state = state_with_agent();
        let agent_id = state.read().await.list_agents()[0].state.id.clone();
        let credentials = Arc::new(OneShotUncertainDeleteStore {
            inner: InMemoryCredentialStore::default(),
            fail_next_delete: AtomicBool::new(true),
        });
        let first_manager = manager(
            Arc::clone(&state),
            credentials.clone(),
            Arc::new(FakeTransport::default()),
        );
        let gate = state
            .write()
            .await
            .install_test_control_plane_save_gate(true);
        gate.release.add_permits(1);

        assert_eq!(
            first_manager
                .create(
                    agent_id.clone(),
                    TelegramBotToken::parse("42:cleanup-on-restart").unwrap(),
                )
                .await
                .unwrap_err(),
            super::ConnectorManagerError::CredentialStateUncertain
        );
        let marker = state
            .read()
            .await
            .connectors
            .values()
            .next()
            .unwrap()
            .clone();
        assert!(!marker.enabled);
        assert!(marker.deleted_at_ms.is_some());
        assert_eq!(
            first_manager.status(&marker.id).await,
            Some(ConnectorRuntimeStatus::Reconciling)
        );
        assert!(credentials.load(&marker.id).await.unwrap().is_some());

        first_manager.shutdown().await;
        let restored = manager(
            Arc::clone(&state),
            credentials.clone(),
            Arc::new(FakeTransport::default()),
        );
        restored.start_restored().await;
        assert!(credentials.load(&marker.id).await.unwrap().is_none());
        assert_eq!(restored.status(&marker.id).await, None);
        let replacement = restored
            .create(
                agent_id,
                TelegramBotToken::parse("84:retry-after-cleanup").unwrap(),
            )
            .await
            .expect("a tombstoned reconciliation marker must not block retry");
        assert!(replacement.is_active());
        restored.shutdown().await;
    }

    #[tokio::test]
    async fn failed_reconciliation_marker_save_remains_visible_in_memory() {
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
                    .create(
                        agent_id,
                        TelegramBotToken::parse("42:uncertain-create").unwrap(),
                    )
                    .await
            })
        };
        tokio::time::timeout(Duration::from_secs(1), entered.acquire())
            .await
            .expect("vault write must precede the failing metadata publish")
            .unwrap()
            .forget();
        assert!(state.read().await.connectors.is_empty());
        release.add_permits(1);

        assert_eq!(
            creating.await.unwrap().unwrap_err(),
            super::ConnectorManagerError::CredentialStateUncertain
        );
        let marker = state
            .read()
            .await
            .connectors
            .values()
            .next()
            .unwrap()
            .clone();
        assert!(!marker.enabled);
        assert!(marker.deleted_at_ms.is_some());
        assert_eq!(
            manager.status(&marker.id).await,
            Some(ConnectorRuntimeStatus::Reconciling)
        );
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
            super::ConnectorManagerError::ConnectorNotFound
        );
        assert!(state.read().await.connectors[&connector.id]
            .pending_pairing
            .is_none());

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
        let manager = manager(
            Arc::clone(&state),
            Arc::new(InMemoryCredentialStore::default()),
            Arc::new(FakeTransport::default()),
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
