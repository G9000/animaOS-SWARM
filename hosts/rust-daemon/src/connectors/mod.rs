use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TelegramConnectorRecord {
    pub(crate) id: String,
    pub(crate) agent_id: String,
    pub(crate) room_id: String,
    pub(crate) bot_identity: TelegramBotIdentity,
    #[serde(default)]
    pub(crate) approved_chat: Option<TelegramChatMetadata>,
    #[serde(default)]
    pub(crate) latest_pending_pairing: Option<TelegramPendingPairing>,
    #[serde(default)]
    pub(crate) next_update_id: i64,
    #[serde(default = "default_enabled")]
    pub(crate) enabled: bool,
    pub(crate) created_at_ms: i64,
    pub(crate) updated_at_ms: i64,
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
    pub(crate) requested_at_ms: i64,
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
    pub(crate) received_at_ms: i64,
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
    pub(crate) created_at_ms: i64,
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
