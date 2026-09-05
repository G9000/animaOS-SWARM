use serde::{Deserialize, Serialize};

pub(crate) mod credentials;
pub(crate) mod gcalendar;
pub(crate) mod mail;
pub(crate) mod oauth_apps;
pub(crate) mod runtime;
pub(crate) mod telegram;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TelegramConnectorRecord {
    pub(crate) id: String,
    pub(crate) agent_id: String,
    pub(crate) room_id: String,
    pub(crate) bot: TelegramBotIdentity,
    #[serde(default)]
    pub(crate) approved_chat: Option<TelegramChatMetadata>,
    #[serde(default)]
    pub(crate) pending_pairing: Option<TelegramPendingPairing>,
    #[serde(default)]
    pub(crate) next_update_id: i64,
    #[serde(default = "default_enabled")]
    pub(crate) enabled: bool,
    #[serde(default)]
    pub(crate) deleted_at_ms: Option<u64>,
    pub(crate) created_at_ms: u64,
    pub(crate) updated_at_ms: u64,
}

/// Durable, non-secret evidence that a generated vault account may need
/// cleanup before the connector can be published safely.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TelegramCredentialCleanupIntent {
    pub(crate) connector_id: String,
    pub(crate) created_at_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TelegramBotIdentity {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) username: Option<String>,
    #[serde(default)]
    pub(crate) display_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TelegramChatMetadata {
    pub(crate) id: String,
    pub(crate) kind: TelegramChatKind,
    #[serde(default)]
    pub(crate) title: Option<String>,
    #[serde(default)]
    pub(crate) username: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum TelegramChatKind {
    Private,
    Group,
    Supergroup,
    Channel,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TelegramPendingPairing {
    pub(crate) chat: TelegramChatMetadata,
    pub(crate) requested_at_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TelegramSenderMetadata {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) username: Option<String>,
    #[serde(default)]
    pub(crate) display_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TelegramInboundRecord {
    pub(crate) connector_id: String,
    pub(crate) update_id: i64,
    pub(crate) agent_id: String,
    pub(crate) room_id: String,
    pub(crate) normalized_text: String,
    pub(crate) sender: TelegramSenderMetadata,
    pub(crate) chat: TelegramChatMetadata,
    pub(crate) received_at_ms: u64,
    pub(crate) processing_state: InboundProcessingState,
    pub(crate) run_idempotency_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum InboundProcessingState {
    Received,
    Processing,
    Processed,
    Rejected,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TelegramOutboundRecord {
    pub(crate) id: String,
    pub(crate) connector_id: String,
    pub(crate) agent_id: String,
    pub(crate) room_id: String,
    pub(crate) assistant_message_id: String,
    pub(crate) text: String,
    pub(crate) created_at_ms: u64,
    #[serde(default)]
    pub(crate) delivered_at_ms: Option<u64>,
    #[serde(default)]
    pub(crate) attempts: u32,
    pub(crate) delivery_state: OutboundDeliveryState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum OutboundDeliveryState {
    Pending,
    Delivered,
    Failed,
}

fn default_enabled() -> bool {
    true
}

impl TelegramConnectorRecord {
    pub(crate) fn is_active(&self) -> bool {
        self.enabled && self.deleted_at_ms.is_none()
    }
}
