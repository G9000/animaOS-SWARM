use anima_core::{Message, MessageRole};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::connectors::runtime::ConnectorRuntimeStatus;
use crate::connectors::{
    TelegramBotIdentity, TelegramChatKind, TelegramChatMetadata, TelegramConnectorRecord,
    TelegramPendingPairing,
};

use super::agents::{AgentMessageResponse, AgentRunEnvelope};
use super::shared::{ContentResponse, TaskResultResponse};

pub(crate) const MAX_CONNECTOR_MESSAGE_SCALARS: usize = 4096;
pub(crate) const MAX_CONNECTOR_PAGE_LIMIT: usize = 100;
pub(crate) const DEFAULT_CONNECTOR_PAGE_LIMIT: usize = 50;
pub(crate) const MAX_CONNECTOR_CURSOR_LENGTH: usize = 256;

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TelegramCredentialRequest {
    pub(crate) bot_token: Option<String>,
}

impl TelegramCredentialRequest {
    pub(crate) fn into_token(self) -> Result<String, &'static str> {
        let token = self.bot_token.ok_or("botToken is required")?;
        if token.is_empty() {
            return Err("botToken must not be empty");
        }
        Ok(token)
    }
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ConnectorMessageRequest {
    pub(crate) text: Option<String>,
}

impl ConnectorMessageRequest {
    pub(crate) fn into_text(self) -> Result<String, &'static str> {
        let text = self.text.ok_or("text is required")?;
        let scalar_count = text.chars().count();
        if scalar_count == 0 {
            return Err("text must not be empty");
        }
        if scalar_count > MAX_CONNECTOR_MESSAGE_SCALARS {
            return Err("text exceeds Telegram's 4096 character limit");
        }
        Ok(text)
    }
}

#[derive(Clone, Debug, Default, Deserialize, IntoParams, ToSchema)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ConnectorMessagePageQuery {
    pub(crate) before: Option<String>,
    pub(crate) limit: Option<usize>,
}

impl ConnectorMessagePageQuery {
    pub(crate) fn from_query_map(
        query: &std::collections::HashMap<String, String>,
    ) -> Result<Self, &'static str> {
        if query.keys().any(|key| key != "before" && key != "limit") {
            return Err("unsupported connector message query parameter");
        }
        let limit = query
            .get("limit")
            .map(|value| {
                value
                    .parse::<usize>()
                    .map_err(|_| "limit must be an integer")
            })
            .transpose()?;
        Ok(Self {
            before: query.get("before").cloned(),
            limit,
        })
    }

    pub(crate) fn validated(self) -> Result<(Option<String>, usize), &'static str> {
        if self
            .before
            .as_ref()
            .is_some_and(|cursor| cursor.is_empty() || cursor.len() > MAX_CONNECTOR_CURSOR_LENGTH)
        {
            return Err("before must be a bounded message identifier");
        }
        let limit = self.limit.unwrap_or(DEFAULT_CONNECTOR_PAGE_LIMIT);
        if !(1..=MAX_CONNECTOR_PAGE_LIMIT).contains(&limit) {
            return Err("limit must be between 1 and 100");
        }
        Ok((self.before, limit))
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectorErrorBody {
    pub(crate) code: String,
    pub(crate) error: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TelegramBotResponse {
    pub(crate) id: String,
    pub(crate) username: Option<String>,
    pub(crate) display_name: Option<String>,
}

impl From<&TelegramBotIdentity> for TelegramBotResponse {
    fn from(value: &TelegramBotIdentity) -> Self {
        Self {
            id: value.id.clone(),
            username: value.username.clone(),
            display_name: value.display_name.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TelegramChatResponse {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) title: Option<String>,
    pub(crate) username: Option<String>,
}

impl From<&TelegramChatMetadata> for TelegramChatResponse {
    fn from(value: &TelegramChatMetadata) -> Self {
        Self {
            id: value.id.clone(),
            kind: match value.kind {
                TelegramChatKind::Private => "private",
                TelegramChatKind::Group => "group",
                TelegramChatKind::Supergroup => "supergroup",
                TelegramChatKind::Channel => "channel",
            }
            .to_string(),
            title: value.title.clone(),
            username: value.username.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TelegramPendingPairingResponse {
    pub(crate) chat: TelegramChatResponse,
    pub(crate) requested_at_ms: u64,
}

impl From<&TelegramPendingPairing> for TelegramPendingPairingResponse {
    fn from(value: &TelegramPendingPairing) -> Self {
        Self {
            chat: TelegramChatResponse::from(&value.chat),
            requested_at_ms: value.requested_at_ms,
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TelegramConnectorResponse {
    pub(crate) id: String,
    pub(crate) agent_id: String,
    pub(crate) room_id: String,
    #[serde(rename = "type")]
    pub(crate) connector_type: String,
    pub(crate) bot: TelegramBotResponse,
    pub(crate) approved_chat: Option<TelegramChatResponse>,
    pub(crate) pending_pairing: Option<TelegramPendingPairingResponse>,
    pub(crate) status: String,
    pub(crate) enabled: bool,
    pub(crate) created_at_ms: u64,
    pub(crate) updated_at_ms: u64,
}

impl TelegramConnectorResponse {
    pub(crate) fn from_record(
        value: &TelegramConnectorRecord,
        status: ConnectorRuntimeStatus,
    ) -> Self {
        Self {
            id: value.id.clone(),
            agent_id: value.agent_id.clone(),
            room_id: value.room_id.clone(),
            connector_type: "telegram".to_string(),
            bot: TelegramBotResponse::from(&value.bot),
            approved_chat: value.approved_chat.as_ref().map(TelegramChatResponse::from),
            pending_pairing: value
                .pending_pairing
                .as_ref()
                .map(TelegramPendingPairingResponse::from),
            status: connector_status_name(status).to_string(),
            enabled: value.enabled,
            created_at_ms: value.created_at_ms,
            updated_at_ms: value.updated_at_ms,
        }
    }
}

fn connector_status_name(status: ConnectorRuntimeStatus) -> &'static str {
    match status {
        ConnectorRuntimeStatus::Ready => "ready",
        ConnectorRuntimeStatus::Pairing => "pairing",
        ConnectorRuntimeStatus::CredentialRequired => "credentialRequired",
        ConnectorRuntimeStatus::Error => "error",
        ConnectorRuntimeStatus::Degraded => "degraded",
        ConnectorRuntimeStatus::Reconciling => "reconciling",
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct TelegramConnectorEnvelope {
    pub(crate) connector: TelegramConnectorResponse,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct TelegramConnectorsEnvelope {
    pub(crate) connectors: Vec<TelegramConnectorResponse>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectorMessageResponse {
    pub(crate) id: String,
    pub(crate) agent_id: String,
    pub(crate) room_id: String,
    pub(crate) content: ContentResponse,
    pub(crate) role: String,
    pub(crate) created_at_ms: u64,
}

impl From<&Message> for ConnectorMessageResponse {
    fn from(value: &Message) -> Self {
        Self {
            id: value.id.clone(),
            agent_id: value.agent_id.clone(),
            room_id: value.room_id.clone(),
            content: ContentResponse::from(&value.content),
            role: match value.role {
                MessageRole::User => "user",
                MessageRole::Assistant => "assistant",
                MessageRole::System => "system",
                MessageRole::Tool => "tool",
            }
            .to_string(),
            created_at_ms: value.created_at_ms,
        }
    }
}

impl From<AgentMessageResponse> for ConnectorMessageResponse {
    fn from(value: AgentMessageResponse) -> Self {
        Self {
            id: value.id,
            agent_id: value.agent_id,
            room_id: value.room_id,
            content: value.content,
            role: value.role,
            created_at_ms: value.created_at_ms,
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectorMessagesEnvelope {
    pub(crate) messages: Vec<ConnectorMessageResponse>,
    pub(crate) next_before: Option<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectorMessageSendResponse {
    pub(crate) messages: Vec<ConnectorMessageResponse>,
    pub(crate) result: TaskResultResponse,
    pub(crate) delivery_queued: bool,
}

impl ConnectorMessageSendResponse {
    pub(crate) fn from_run(run: AgentRunEnvelope, room_id: &str) -> Self {
        let delivery_queued = run.result.status == "success";
        Self {
            messages: run
                .agent
                .messages
                .into_iter()
                .filter(|message| message.room_id == room_id)
                .map(ConnectorMessageResponse::from)
                .collect(),
            result: run.result,
            delivery_queued,
        }
    }
}
