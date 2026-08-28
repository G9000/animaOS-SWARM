use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anima_core::{Content, DataValue, MessageRole, TaskStatus};
use async_trait::async_trait;
use tokio::sync::{watch, Mutex};
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
    mutation_lock: Arc<Mutex<()>>,
    statuses: Arc<Mutex<HashMap<String, ConnectorRuntimeStatus>>>,
    workers: Arc<Mutex<HashMap<String, WorkerHandle>>>,
}

struct WorkerHandle {
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
            mutation_lock: Arc::new(Mutex::new(())),
            statuses: Arc::new(Mutex::new(HashMap::new())),
            workers: Arc::new(Mutex::new(HashMap::new())),
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
        {
            let state = self.state.read().await;
            if !state
                .connectors
                .get(&connector_id)
                .is_some_and(TelegramConnectorRecord::is_active)
            {
                return Err(ConnectorManagerError::ConnectorNotFound);
            }
        }
        let bot = self
            .transport
            .get_me(&token)
            .await
            .map_err(|_| ConnectorManagerError::Transport)?;
        self.stop_worker(&connector_id).await?;
        let _mutation = self.mutation_lock.lock().await;
        let previous_token = match self.credentials.load(&connector_id).await {
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
        let worker_token = token.clone();
        if let Err(error) = self.credentials.put(&connector_id, token).await {
            let mapped = map_credential_error(error);
            self.statuses.lock().await.insert(
                connector_id.clone(),
                if mapped == ConnectorManagerError::CredentialStateUncertain {
                    ConnectorRuntimeStatus::Reconciling
                } else {
                    ConnectorRuntimeStatus::Error
                },
            );
            if mapped != ConnectorManagerError::CredentialStateUncertain {
                drop(_mutation);
                let _ = self.start_worker(connector_id, previous_token).await;
            }
            return Err(mapped);
        }

        let (previous, updated, persist) = {
            let mut state = self.state.write().await;
            let previous = state
                .connectors
                .get(&connector_id)
                .cloned()
                .ok_or(ConnectorManagerError::ConnectorNotFound)?;
            let connector = state
                .connectors
                .get_mut(&connector_id)
                .expect("connector was prevalidated");
            connector.bot = bot;
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
            match self
                .credentials
                .put(&connector_id, previous_token.clone())
                .await
            {
                Ok(()) => {
                    self.statuses
                        .lock()
                        .await
                        .insert(connector_id.clone(), ConnectorRuntimeStatus::Error);
                    drop(_mutation);
                    let _ = self.start_worker(connector_id, previous_token).await;
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
        drop(_mutation);
        self.start_worker(connector_id, worker_token).await?;
        Ok(updated)
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
            .run_with_commit(request, move |state, snapshot, result| {
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
                        message.room_id == commit_room_id && message.role == MessageRole::Assistant
                    })
                    .cloned()
                    .ok_or_else(|| ApiError::bad_request("agent produced no assistant message"))?;
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
                        return Err(ApiError::bad_request("durable outbound conflicts with run"));
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
            })
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
        let (previous, persist) = {
            let _mutation = self.mutation_lock.lock().await;
            let mut state = self.state.write().await;
            let record = state
                .outbound
                .get_mut(&outbound.id)
                .ok_or(ConnectorManagerError::ConnectorNotFound)?;
            let previous = record.clone();
            record.attempts = record.attempts.saturating_add(1);
            match delivery {
                Ok(_) => {
                    record.delivery_state = OutboundDeliveryState::Delivered;
                    record.delivered_at_ms = Some(now_ms());
                }
                Err(_) => {
                    record.delivery_state = OutboundDeliveryState::Failed;
                    record.delivered_at_ms = None;
                }
            }
            let persist = state.control_plane_persist_request();
            (previous, persist)
        };
        if persist.save().await.is_err() {
            self.state
                .write()
                .await
                .outbound
                .insert(outbound.id, previous);
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
                manager.delete_owned(connector_id).await?;
            }
            Ok(())
        })
        .await
        .map_err(|_| ConnectorManagerError::WorkerStopped)?
    }

    async fn delete_owned(&self, connector_id: String) -> Result<(), ConnectorManagerError> {
        self.stop_worker(&connector_id).await?;
        let _mutation = self.mutation_lock.lock().await;
        {
            let state = self.state.read().await;
            if !state
                .connectors
                .get(&connector_id)
                .is_some_and(TelegramConnectorRecord::is_active)
            {
                return Err(ConnectorManagerError::ConnectorNotFound);
            }
        }

        if let Err(error) = self.credentials.delete(&connector_id).await {
            let mapped = map_credential_error(error);
            self.statuses.lock().await.insert(
                connector_id.clone(),
                if mapped == ConnectorManagerError::CredentialStateUncertain {
                    ConnectorRuntimeStatus::Reconciling
                } else {
                    ConnectorRuntimeStatus::Error
                },
            );
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
            compact_delivered_outbox(&mut state.outbound, now);
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
            self.statuses
                .lock()
                .await
                .insert(connector_id, ConnectorRuntimeStatus::CredentialRequired);
            return Err(ConnectorManagerError::Persistence);
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
            let _ = self.restart(connector_id).await;
        }
    }

    async fn start_worker(
        &self,
        connector_id: String,
        token: TelegramBotToken,
    ) -> Result<(), ConnectorManagerError> {
        self.stop_worker(&connector_id).await?;
        let (cancel, receiver) = watch::channel(false);
        let manager = self.clone();
        let worker_connector_id = connector_id.clone();
        let join = tokio::spawn(async move {
            manager
                .worker_loop(worker_connector_id, token, receiver)
                .await;
        });
        self.workers
            .lock()
            .await
            .insert(connector_id, WorkerHandle { cancel, join });
        Ok(())
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

fn compact_delivered_outbox(outbox: &mut HashMap<String, TelegramOutboundRecord>, now: u64) {
    let cutoff = now.saturating_sub(DELIVERED_RETENTION_MS);
    outbox.retain(|_, record| {
        record.delivery_state != OutboundDeliveryState::Delivered
            || record
                .delivered_at_ms
                .is_none_or(|delivered_at_ms| delivered_at_ms >= cutoff)
    });
    let mut delivered = outbox
        .values()
        .filter(|record| record.delivery_state == OutboundDeliveryState::Delivered)
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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use anima_core::{
        AgentConfig, AgentSettings, Content, ModelAdapter, ModelGenerateRequest,
        ModelGenerateResponse, ModelStopReason, TokenUsage,
    };
    use async_trait::async_trait;
    use tokio::sync::{RwLock, Semaphore};

    use super::{ConnectorManager, TelegramTransport};
    use crate::agent_runs::AgentRunCoordinator;
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
        remaining_send_failures: AtomicUsize,
    }

    #[derive(Default)]
    struct UncertainDeleteStore {
        inner: InMemoryCredentialStore,
    }

    #[derive(Default)]
    struct UncertainLoadStore;

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
                    updates: vec![text_update(2, "101", "run")],
                    next_update_id: 3,
                },
            )
            .await
            .unwrap();

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
        std::fs::remove_file(&snapshot_path).unwrap();
        std::fs::create_dir(&snapshot_path).unwrap();
        release.add_permits(1);

        assert_eq!(
            processing.await.unwrap().unwrap_err(),
            super::ConnectorManagerError::Persistence
        );
        let state = state.read().await;
        assert_eq!(
            state
                .inbound
                .get(&(connector.id.clone(), 2))
                .unwrap()
                .processing_state,
            InboundProcessingState::Processing
        );
        assert!(state
            .outbound
            .values()
            .all(|record| record.connector_id != connector.id));
        drop(state);
        std::fs::remove_dir_all(temporary).unwrap();
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

        super::compact_delivered_outbox(&mut outbox, now);
        assert!(!outbox.contains_key("old"));
        assert!(outbox.contains_key("recent"));
        assert!(outbox.contains_key("pending"));
        assert!(outbox.contains_key("failed"));
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
