use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
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
}

struct WorkerHandle {
    generation: u64,
    cancel: watch::Sender<bool>,
    join: JoinHandle<()>,
}

impl ConnectorManager {
    pub(crate) fn new(
        state: SharedDaemonState,
        runs: AgentRunCoordinator,
        credentials: Arc<dyn ConnectorCredentialStore>,
        transport: Arc<dyn TelegramTransport>,
    ) -> Self {
        Self {
            state,
            runs,
            credentials,
            transport,
            lifecycle_lock: Arc::new(Mutex::new(())),
            mutation_lock: Arc::new(Mutex::new(())),
            statuses: Arc::new(Mutex::new(HashMap::new())),
            workers: Arc::new(Mutex::new(HashMap::new())),
            worker_generation: Arc::new(AtomicU64::new(1)),
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
        let _mutation = self.mutation_lock.lock().await;
        {
            let state = self.state.read().await;
            if state.get_agent(&agent_id).is_none() {
                return Err(ConnectorManagerError::AgentNotFound);
            }
            if state
                .connectors
                .values()
                .any(|connector| connector.agent_id == agent_id && connector.is_active())
            {
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
        self.credentials
            .put(&id, token)
            .await
            .map_err(map_credential_error)?;

        let persist = {
            let mut state = self.state.write().await;
            state.connectors.insert(id.clone(), connector.clone());
            state.control_plane_persist_request()
        };
        if persist.save().await.is_err() {
            self.state.write().await.connectors.remove(&id);
            match self.credentials.delete(&id).await {
                Ok(()) => return Err(ConnectorManagerError::Persistence),
                Err(CredentialStoreError::CredentialStateUncertain) => {
                    self.statuses
                        .lock()
                        .await
                        .insert(id, ConnectorRuntimeStatus::Reconciling);
                    return Err(ConnectorManagerError::CredentialStateUncertain);
                }
                Err(_) => return Err(ConnectorManagerError::Credential),
            }
        }
        self.statuses
            .lock()
            .await
            .insert(id.clone(), ConnectorRuntimeStatus::Pairing);
        drop(_mutation);
        self.start_worker(id, worker_token).await?;
        Ok(connector)
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
        let preflight_connector = {
            let state = self.state.read().await;
            state
                .connectors
                .get(&connector_id)
                .filter(|connector| connector.is_active())
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
            Ok(Some(token)) => token,
            Ok(None) => {
                self.stop_worker(&connector_id).await?;
                self.statuses
                    .lock()
                    .await
                    .insert(connector_id, ConnectorRuntimeStatus::CredentialRequired);
                return Err(ConnectorManagerError::Credential);
            }
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
            drop(_mutation);
            if rollback.save().await.is_err() {
                let _ = self.credentials.put(&connector_id, previous_token).await;
                self.statuses
                    .lock()
                    .await
                    .insert(connector_id, ConnectorRuntimeStatus::Reconciling);
                return Err(ConnectorManagerError::Persistence);
            }
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
            let rollback = {
                let mut state = self.state.write().await;
                state
                    .connectors
                    .insert(connector_id.clone(), previous.clone());
                state.control_plane_persist_request()
            };
            if rollback.save().await.is_err() {
                let _ = self.credentials.put(&connector_id, previous_token).await;
                self.statuses
                    .lock()
                    .await
                    .insert(connector_id, ConnectorRuntimeStatus::Reconciling);
                return Err(ConnectorManagerError::Persistence);
            }
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
        previous_token: TelegramBotToken,
        previous_status: ConnectorRuntimeStatus,
    ) -> Result<(), ConnectorManagerError> {
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
            let previous_inbound = state.inbound.clone();
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
            let persist = state.control_plane_persist_request();
            (previous_connector, previous_inbound, persist)
        };

        if persist.save().await.is_err() {
            let mut state = self.state.write().await;
            state
                .connectors
                .insert(connector_id.clone(), previous_connector);
            state.inbound = previous_inbound;
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
        let previous_outbound = self.state.read().await.outbound.get(&outbound_id).cloned();
        let commit_outbound_id = outbound_id.clone();
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
                        state
                            .inbound
                            .get_mut(&commit_key)
                            .expect("inbound was prevalidated")
                            .processing_state = InboundProcessingState::Rejected;
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

                    state
                        .inbound
                        .get_mut(&commit_key)
                        .expect("inbound was prevalidated")
                        .processing_state = InboundProcessingState::Processed;
                    state
                        .outbound
                        .entry(commit_outbound_id.clone())
                        .or_insert(candidate);
                    Ok(())
                },
                |state, baseline| {
                    state
                        .rollback_agent_runtime(baseline)
                        .map(|_| ())
                        .map_err(ApiError::service_unavailable)
                },
            )
            .await;
        if run.is_err() {
            let _mutation = self.mutation_lock.lock().await;
            let mut state = self.state.write().await;
            if let Some(record) = state.inbound.get_mut(&key) {
                record.processing_state = InboundProcessingState::Processing;
            }
            if let Some(previous) = previous_outbound {
                state.outbound.insert(outbound_id, previous);
            } else {
                state.outbound.remove(&outbound_id);
            }
            drop(state);
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
                .filter(|connector| connector.is_active())
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
        let (previous_outbound, persist) = {
            let _mutation = self.mutation_lock.lock().await;
            let mut state = self.state.write().await;
            let previous_outbound = state.outbound.clone();
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
            (previous_outbound, persist)
        };
        if persist.save().await.is_err() {
            self.state.write().await.outbound = previous_outbound;
            self.statuses
                .lock()
                .await
                .insert(connector_id, ConnectorRuntimeStatus::Error);
            return Err(ConnectorManagerError::Persistence);
        }
        delivery
            .map(|_| true)
            .map_err(|_| ConnectorManagerError::Transport)
    }

    pub(crate) async fn delete(&self, connector_id: String) -> Result<(), ConnectorManagerError> {
        let manager = self.clone();
        tokio::spawn(async move { manager.delete_owned(connector_id).await })
            .await
            .map_err(|_| ConnectorManagerError::WorkerStopped)?
    }

    pub(crate) async fn delete_for_agent(
        &self,
        agent_id: String,
    ) -> Result<(), ConnectorManagerError> {
        let manager = self.clone();
        tokio::spawn(async move {
            let _lifecycle = manager.lifecycle_lock.lock().await;
            let connector_ids = manager
                .state
                .read()
                .await
                .connectors
                .values()
                .filter(|connector| connector.agent_id == agent_id && connector.is_active())
                .map(|connector| connector.id.clone())
                .collect::<Vec<_>>();
            for connector_id in connector_ids {
                manager.delete_unlocked(connector_id).await?;
            }
            Ok(())
        })
        .await
        .map_err(|_| ConnectorManagerError::WorkerStopped)?
    }

    async fn delete_owned(&self, connector_id: String) -> Result<(), ConnectorManagerError> {
        let _lifecycle = self.lifecycle_lock.lock().await;
        self.delete_unlocked(connector_id).await
    }

    async fn delete_unlocked(&self, connector_id: String) -> Result<(), ConnectorManagerError> {
        let connector = {
            let state = self.state.read().await;
            state
                .connectors
                .get(&connector_id)
                .filter(|connector| connector.is_active())
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
            Ok(Some(token)) => token,
            Ok(None) => {
                self.stop_worker(&connector_id).await?;
                self.statuses
                    .lock()
                    .await
                    .insert(connector_id, ConnectorRuntimeStatus::CredentialRequired);
                return Err(ConnectorManagerError::Credential);
            }
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
                self.start_worker(connector_id.clone(), previous_token)
                    .await?;
                self.statuses
                    .lock()
                    .await
                    .insert(connector_id, previous_status);
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
        let connector_ids = self
            .state
            .read()
            .await
            .connectors
            .values()
            .filter(|connector| connector.is_active())
            .map(|connector| connector.id.clone())
            .collect::<Vec<_>>();
        for connector_id in connector_ids {
            let _ = self.restart_unlocked(connector_id).await;
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
        let manager = self.clone();
        let worker_connector_id = connector_id.clone();
        let join = tokio::spawn(async move {
            if started.await.is_ok() {
                manager
                    .worker_loop(worker_connector_id.clone(), token, receiver)
                    .await;
                manager
                    .prune_finished_worker(&worker_connector_id, generation)
                    .await;
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

    async fn prune_finished_worker(&self, connector_id: &str, generation: u64) {
        let mut workers = self.workers.lock().await;
        if workers
            .get(connector_id)
            .is_some_and(|handle| handle.generation == generation)
        {
            workers.remove(connector_id);
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
                Ok(batch) => batch,
                Err(_) => {
                    self.statuses
                        .lock()
                        .await
                        .insert(connector_id.clone(), ConnectorRuntimeStatus::Error);
                    if wait_or_cancel(&mut cancel, Duration::from_secs(1)).await {
                        return;
                    }
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

            if let Some(status) = self
                .state
                .read()
                .await
                .connectors
                .get(&connector_id)
                .map(connector_operational_status)
            {
                self.statuses
                    .lock()
                    .await
                    .insert(connector_id.clone(), status);
            }

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

    pub(crate) async fn worker_count(&self) -> usize {
        self.workers.lock().await.len()
    }

    pub(crate) async fn shutdown(&self) {
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
        assert!(manager
            .deliver_pending_once(connector.id.clone())
            .await
            .unwrap());
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
        restart.await.unwrap().unwrap();
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
        manager.shutdown().await;
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
    async fn agent_cleanup_tombstones_connectors_before_agent_removal() {
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

        manager.delete_for_agent(agent_id.clone()).await.unwrap();
        state.write().await.remove_agent(&agent_id);
        assert!(!state.read().await.connectors[&connector.id].is_active());
        manager.shutdown().await;
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
