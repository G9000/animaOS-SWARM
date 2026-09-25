//! Session route bodies (spec §3.3).

use std::collections::BTreeMap;

use anima_core::{AttachmentType, MessageRole};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use super::shared::data_value_to_json;
use crate::sessions::views::{MessagePage, PageMessage, SessionPage, SessionView};
use crate::sessions::SessionCapabilities;

/// Message metadata the session routes expose: spec §3.3's list plus `kind`
/// (check-in prompts) and `source` (Telegram turns), and what a historical
/// tool card or step label shows: a tool result's `toolStatus` and
/// `toolDurationMs`, and `incomplete` on a model call's unfinished text.
/// `taskResult` stays hidden: it repeats the whole tool result.
pub(crate) const EXPOSED_MESSAGE_METADATA: [&str; 15] = [
    "toolCalls",
    "toolCallId",
    "stepId",
    "runId",
    "toolStatus",
    "toolDurationMs",
    "incomplete",
    "stopped",
    "revised",
    "steer",
    "skill",
    "clientRequestId",
    "communication",
    "kind",
    "source",
];

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionCapabilitiesResponse {
    pub(crate) send: bool,
    pub(crate) steer: bool,
    pub(crate) stop: bool,
    pub(crate) rename: bool,
    pub(crate) archive: bool,
    pub(crate) delete: bool,
    pub(crate) compact: bool,
    pub(crate) export: bool,
}

impl From<SessionCapabilities> for SessionCapabilitiesResponse {
    fn from(value: SessionCapabilities) -> Self {
        Self {
            send: value.send,
            steer: value.steer,
            stop: value.stop,
            rename: value.rename,
            archive: value.archive,
            delete: value.delete,
            compact: value.compact,
            export: value.export,
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionSummaryResponse {
    pub(crate) text: String,
    pub(crate) through_message_id: String,
    pub(crate) created_at_ms: u64,
    pub(crate) source_message_count: usize,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionContextTrimmedResponse {
    pub(crate) dropped_through_message_id: String,
    pub(crate) at_ms: u64,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionMatchResponse {
    /// `null` when only the title matched.
    pub(crate) message_id: Option<String>,
    pub(crate) snippet: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionResponse {
    pub(crate) id: String,
    pub(crate) agent_id: String,
    /// The transcript room: the id itself except for mapped legacy rooms.
    pub(crate) room_id: String,
    pub(crate) kind: String,
    pub(crate) origin: String,
    pub(crate) title: String,
    pub(crate) title_source: String,
    pub(crate) created_at_ms: u64,
    pub(crate) last_activity_at_ms: u64,
    pub(crate) last_read_at_ms: Option<u64>,
    pub(crate) archived: bool,
    pub(crate) parent_session_id: Option<String>,
    pub(crate) parent_run_id: Option<String>,
    pub(crate) parent_agent_id: Option<String>,
    pub(crate) summary: Option<SessionSummaryResponse>,
    pub(crate) context_trimmed: Option<SessionContextTrimmedResponse>,
    pub(crate) message_count: usize,
    pub(crate) preview: Option<String>,
    pub(crate) active_runs: usize,
    /// Always 0 until approvals exist (M4).
    pub(crate) pending_approvals: usize,
    pub(crate) unread: bool,
    pub(crate) capabilities: SessionCapabilitiesResponse,
    #[serde(rename = "match", skip_serializing_if = "Option::is_none")]
    pub(crate) matched: Option<SessionMatchResponse>,
}

impl From<&SessionView> for SessionResponse {
    fn from(view: &SessionView) -> Self {
        let record = &view.record;
        Self {
            id: record.id.clone(),
            agent_id: record.agent_id.clone(),
            room_id: record.room_id().to_string(),
            kind: record.kind.as_str().into(),
            origin: record.origin.as_str().into(),
            title: record.title.clone(),
            title_source: record.title_source.as_str().into(),
            created_at_ms: record.created_at_ms,
            last_activity_at_ms: record.last_activity_at_ms,
            last_read_at_ms: record.last_read_at_ms,
            archived: record.archived,
            parent_session_id: record.parent_session_id.clone(),
            parent_run_id: record.parent_run_id.clone(),
            parent_agent_id: record.parent_agent_id.clone(),
            summary: record
                .summary
                .as_ref()
                .map(|summary| SessionSummaryResponse {
                    text: summary.text.clone(),
                    through_message_id: summary.through_message_id.clone(),
                    created_at_ms: summary.created_at_ms,
                    source_message_count: summary.source_message_count,
                }),
            context_trimmed: record.context_trimmed.as_ref().map(|trimmed| {
                SessionContextTrimmedResponse {
                    dropped_through_message_id: trimmed.dropped_through_message_id.clone(),
                    at_ms: trimmed.at_ms,
                }
            }),
            message_count: view.message_count,
            preview: view.preview.clone(),
            active_runs: view.active_runs,
            pending_approvals: 0,
            unread: view.unread,
            capabilities: view.capabilities.into(),
            matched: view.matched.as_ref().map(|found| SessionMatchResponse {
                message_id: found.message_id.clone(),
                snippet: found.snippet.clone(),
            }),
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionsEnvelope {
    pub(crate) sessions: Vec<SessionResponse>,
    pub(crate) next_cursor: Option<String>,
}

impl From<&SessionPage> for SessionsEnvelope {
    fn from(page: &SessionPage) -> Self {
        Self {
            sessions: page.sessions.iter().map(SessionResponse::from).collect(),
            next_cursor: page.next_cursor.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct SessionEnvelope {
    pub(crate) session: SessionResponse,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct SessionAttachmentResponse {
    #[serde(rename = "type")]
    pub(crate) attachment_type: String,
    pub(crate) name: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionMessageResponse {
    pub(crate) id: String,
    pub(crate) role: String,
    pub(crate) text: String,
    /// Metadata only; attachment contents are not repeated here.
    pub(crate) attachments: Vec<SessionAttachmentResponse>,
    /// Only `toolCalls`, `toolCallId`, `stepId`, `runId`, `toolStatus`
    /// (`success` or `error`), `toolDurationMs`, `incomplete`, `stopped`,
    /// `revised`, `steer`, `skill`, `clientRequestId`, `communication`,
    /// `kind`, and `source`; other keys are not exposed.
    pub(crate) metadata: BTreeMap<String, Value>,
    pub(crate) created_at_ms: u64,
    /// Present (true) only on silent check-in messages, with `includeHidden=true`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) hidden: bool,
}

impl From<&PageMessage> for SessionMessageResponse {
    fn from(entry: &PageMessage) -> Self {
        let message = &entry.message;
        Self {
            id: message.id.clone(),
            role: match message.role {
                MessageRole::User => "user",
                MessageRole::Assistant => "assistant",
                MessageRole::System => "system",
                MessageRole::Tool => "tool",
            }
            .into(),
            text: message.content.text.clone(),
            attachments: message
                .content
                .attachments
                .iter()
                .flatten()
                .map(|attachment| SessionAttachmentResponse {
                    attachment_type: match attachment.attachment_type {
                        AttachmentType::File => "file",
                        AttachmentType::Image => "image",
                        AttachmentType::Url => "url",
                    }
                    .into(),
                    name: attachment.name.clone(),
                })
                .collect(),
            metadata: message
                .content
                .metadata
                .iter()
                .flatten()
                .filter(|(key, _)| EXPOSED_MESSAGE_METADATA.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), data_value_to_json(value)))
                .collect(),
            created_at_ms: message.created_at_ms,
            hidden: entry.hidden,
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionMessagesEnvelope {
    pub(crate) messages: Vec<SessionMessageResponse>,
    pub(crate) next_before: Option<String>,
}

impl From<&MessagePage> for SessionMessagesEnvelope {
    fn from(page: &MessagePage) -> Self {
        Self {
            messages: page
                .messages
                .iter()
                .map(SessionMessageResponse::from)
                .collect(),
            next_before: page.next_before.clone(),
        }
    }
}

/// `POST /api/agents/{id}/sessions`; an empty body means `{}`.
#[derive(Clone, Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionCreateRequest {
    #[serde(default)]
    pub(crate) title: Option<String>,
}

/// `PATCH /api/agents/{id}/sessions/{sid}`; at least one field.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SessionUpdateRequest {
    #[serde(default)]
    pub(crate) title: Option<String>,
    #[serde(default)]
    pub(crate) archived: Option<bool>,
    #[serde(default)]
    pub(crate) last_read_at_ms: Option<u64>,
}
