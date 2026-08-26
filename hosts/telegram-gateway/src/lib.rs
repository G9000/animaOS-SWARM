//! Pure logic for the anima Telegram gateway host.
//!
//! This crate is a thin client bridge between the Telegram Bot API and the
//! anima daemon HTTP API. It contains no agent/engine logic. Everything here
//! is deliberately free of network I/O so it can be unit-tested; `main.rs`
//! holds the wiring.

use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Map, Value};

/// Maximum length of a single Telegram message body.
pub const TELEGRAM_MAX_MESSAGE_LEN: usize = 4000;

/// Default daemon base URL.
pub const DEFAULT_DAEMON_URL: &str = "http://127.0.0.1:8080";
/// Default assistant agent name to resolve on the daemon.
pub const DEFAULT_ASSISTANT_NAME: &str = "assistant";
/// Default outbox poll interval in seconds.
pub const DEFAULT_OUTBOX_POLL_SECS: u64 = 15;

/// Gateway configuration sourced from the environment.
#[derive(Debug, Clone)]
pub struct Config {
    /// Telegram bot token (required, from `TELEGRAM_BOT_TOKEN`).
    pub bot_token: String,
    /// Base URL of the anima daemon HTTP API (no trailing slash).
    pub daemon_url: String,
    /// Name of the agent to resolve and run on the daemon.
    pub assistant_name: String,
    /// Allowlisted Telegram chat ids. Empty means "no chat is authorized".
    pub allowed_chat_ids: Vec<i64>,
    /// Interval between assistant outbox polls.
    pub outbox_poll: Duration,
}

impl Config {
    /// Load configuration from process environment variables.
    pub fn from_env() -> Result<Self, String> {
        Self::from_getter(|key| std::env::var(key).ok())
    }

    /// Load configuration from an arbitrary lookup function. This indirection
    /// keeps parsing unit-testable without touching process env vars.
    pub fn from_getter(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let bot_token = get("TELEGRAM_BOT_TOKEN")
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                "TELEGRAM_BOT_TOKEN is required: create a bot with @BotFather and set it"
                    .to_string()
            })?;

        let daemon_url = get("ANIMAOS_RS_DAEMON_URL")
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_DAEMON_URL.to_string());
        let daemon_url = daemon_url.trim_end_matches('/').to_string();

        let assistant_name = get("ANIMAOS_RS_ASSISTANT_NAME")
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_ASSISTANT_NAME.to_string());

        let allowed_chat_ids = match get("TELEGRAM_ALLOWED_CHAT_IDS") {
            Some(raw) => parse_allowed_chat_ids(&raw)?,
            None => Vec::new(),
        };

        let outbox_poll_secs = match get("TELEGRAM_OUTBOX_POLL_SECS") {
            Some(raw) => {
                let parsed = raw.trim().parse::<u64>().map_err(|_| {
                    "TELEGRAM_OUTBOX_POLL_SECS must be a positive integer".to_string()
                })?;
                if parsed == 0 {
                    return Err("TELEGRAM_OUTBOX_POLL_SECS must be a positive integer".to_string());
                }
                parsed
            }
            None => DEFAULT_OUTBOX_POLL_SECS,
        };

        Ok(Config {
            bot_token,
            daemon_url,
            assistant_name,
            allowed_chat_ids,
            outbox_poll: Duration::from_secs(outbox_poll_secs),
        })
    }

    /// Base URL of the Telegram Bot API for this bot.
    pub fn telegram_api_base(&self) -> String {
        format!("https://api.telegram.org/bot{}", self.bot_token)
    }
}

/// Parse the comma-separated `TELEGRAM_ALLOWED_CHAT_IDS` allowlist.
pub fn parse_allowed_chat_ids(raw: &str) -> Result<Vec<i64>, String> {
    let mut ids = Vec::new();
    for part in raw.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let id = trimmed.parse::<i64>().map_err(|_| {
            format!("TELEGRAM_ALLOWED_CHAT_IDS entry '{trimmed}' is not a valid chat id (i64)")
        })?;
        ids.push(id);
    }
    Ok(ids)
}

/// Split `text` into chunks of at most `max_len` bytes, never breaking a
/// UTF-8 character boundary. Empty input yields no chunks.
pub fn chunk_message(text: &str, max_len: usize) -> Vec<String> {
    assert!(max_len > 0, "max_len must be positive");
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + max_len).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        if end == start {
            // A single character is larger than max_len; emit it anyway.
            end = start + text[start..].chars().next().map_or(1, char::len_utf8);
        }
        chunks.push(text[start..end].to_string());
        start = end;
    }
    chunks
}

/// Response body of Telegram `getUpdates`.
#[derive(Debug, Deserialize)]
pub struct GetUpdatesResponse {
    pub ok: bool,
    #[serde(default)]
    pub result: Vec<Update>,
    #[serde(default)]
    pub description: Option<String>,
}

/// A single Telegram update. Only `message` is relevant to this gateway.
#[derive(Debug, Deserialize)]
pub struct Update {
    pub update_id: i64,
    pub message: Option<TelegramMessage>,
}

/// A Telegram message (subset of fields we consume).
#[derive(Debug, Deserialize)]
pub struct TelegramMessage {
    pub chat: TelegramChat,
    pub from: Option<TelegramUser>,
    pub text: Option<String>,
}

/// A Telegram chat reference.
#[derive(Debug, Deserialize)]
pub struct TelegramChat {
    pub id: i64,
}

/// A Telegram user reference.
#[derive(Debug, Deserialize)]
pub struct TelegramUser {
    pub id: i64,
    pub first_name: Option<String>,
    pub username: Option<String>,
}

/// A normalized incoming text message extracted from a Telegram update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingMessage {
    pub chat_id: i64,
    pub user_id: Option<i64>,
    pub user_name: Option<String>,
    pub text: String,
}

/// Extract an actionable incoming message from an update, ignoring updates
/// without `message.text` (malformed or non-text updates).
pub fn extract_message(update: &Update) -> Option<IncomingMessage> {
    let message = update.message.as_ref()?;
    let text = message.text.as_ref().filter(|text| !text.is_empty())?;
    let user = message.from.as_ref();
    let user_name = user.and_then(|user| {
        user.username
            .clone()
            .filter(|name| !name.is_empty())
            .or_else(|| user.first_name.clone())
    });
    Some(IncomingMessage {
        chat_id: message.chat.id,
        user_id: user.map(|user| user.id),
        user_name,
        text: text.clone(),
    })
}

/// Advance the long-poll offset past a consumed update.
pub fn advance_offset(current: i64, update_id: i64) -> i64 {
    current.max(update_id + 1)
}

/// Notice sent to chats while the allowlist is empty. Includes the chat id so
/// the owner can authorize it.
pub fn unauthorized_notice(chat_id: i64) -> String {
    format!(
        "This chat is not authorized to use this bot.\n\
         Your chat id is: {chat_id}\n\
         Ask the bot owner to add it to TELEGRAM_ALLOWED_CHAT_IDS."
    )
}

/// Response body of the daemon `GET /api/agents`.
#[derive(Debug, Deserialize)]
pub struct AgentsResponse {
    #[serde(default)]
    pub agents: Vec<AgentEntry>,
}

/// A single agent entry; only `state.id` and `state.name` are consumed.
#[derive(Debug, Deserialize)]
pub struct AgentEntry {
    pub state: AgentState,
}

/// Agent state subset.
#[derive(Debug, Deserialize)]
pub struct AgentState {
    pub id: String,
    pub name: String,
    #[serde(flatten)]
    pub extra: Value,
}

/// Build the request body for `POST /api/agents/{agent_id}/run`.
pub fn build_run_request(message: &IncomingMessage) -> Value {
    let mut metadata = Map::new();
    if let Some(user_id) = message.user_id {
        metadata.insert("userId".to_string(), json!(user_id.to_string()));
    }
    if let Some(user_name) = &message.user_name {
        metadata.insert("userName".to_string(), json!(user_name));
    }
    metadata.insert("channel".to_string(), json!("telegram"));
    metadata.insert("chatId".to_string(), json!(message.chat_id.to_string()));
    json!({ "text": message.text, "metadata": metadata })
}

/// Response body of the daemon run endpoint.
#[derive(Debug, Deserialize)]
pub struct RunResponse {
    pub result: RunResult,
}

/// Run result subset.
#[derive(Debug, Deserialize)]
pub struct RunResult {
    pub status: String,
    pub data: Option<RunData>,
    pub error: Option<String>,
}

/// Successful run payload.
#[derive(Debug, Deserialize)]
pub struct RunData {
    pub text: String,
}

/// Render a run response into the text sent back to the Telegram chat.
pub fn reply_text(response: &RunResponse) -> String {
    if response.result.status == "success" {
        response
            .result
            .data
            .as_ref()
            .map(|data| data.text.clone())
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| "(no response)".to_string())
    } else {
        let detail = response
            .result
            .error
            .clone()
            .unwrap_or_else(|| "unknown error".to_string());
        format!("Error: {detail}")
    }
}

/// Response body of the daemon `GET /api/assistant/outbox`.
#[derive(Debug, Deserialize)]
pub struct OutboxResponse {
    #[serde(default)]
    pub messages: Vec<OutboxMessage>,
}

/// A proactive assistant message from the outbox.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxMessage {
    pub id: String,
    pub job: String,
    pub text: String,
    pub created_at_ms: u64,
}

/// Format an outbox message for delivery to Telegram chats.
pub fn format_outbox_message(message: &OutboxMessage) -> String {
    format!("({}) {}", message.job, message.text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_from(pairs: &[(&str, &str)]) -> Result<Config, String> {
        Config::from_getter(|key| {
            pairs
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| value.to_string())
        })
    }

    #[test]
    fn config_requires_bot_token() {
        let error = config_from(&[]).unwrap_err();
        assert!(error.contains("TELEGRAM_BOT_TOKEN"), "unexpected: {error}");

        let error = config_from(&[("TELEGRAM_BOT_TOKEN", "   ")]).unwrap_err();
        assert!(error.contains("TELEGRAM_BOT_TOKEN"), "unexpected: {error}");
    }

    #[test]
    fn config_applies_defaults() {
        let config = config_from(&[("TELEGRAM_BOT_TOKEN", "token")]).unwrap();
        assert_eq!(config.daemon_url, DEFAULT_DAEMON_URL);
        assert_eq!(config.assistant_name, DEFAULT_ASSISTANT_NAME);
        assert!(config.allowed_chat_ids.is_empty());
        assert_eq!(config.outbox_poll, Duration::from_secs(15));
        assert_eq!(
            config.telegram_api_base(),
            "https://api.telegram.org/bottoken"
        );
    }

    #[test]
    fn config_parses_overrides() {
        let config = config_from(&[
            ("TELEGRAM_BOT_TOKEN", "token"),
            ("ANIMAOS_RS_DAEMON_URL", "http://localhost:9999/"),
            ("ANIMAOS_RS_ASSISTANT_NAME", "helper"),
            ("TELEGRAM_ALLOWED_CHAT_IDS", "1, -2 ,3"),
            ("TELEGRAM_OUTBOX_POLL_SECS", "30"),
        ])
        .unwrap();
        assert_eq!(config.daemon_url, "http://localhost:9999");
        assert_eq!(config.assistant_name, "helper");
        assert_eq!(config.allowed_chat_ids, vec![1, -2, 3]);
        assert_eq!(config.outbox_poll, Duration::from_secs(30));
    }

    #[test]
    fn config_rejects_invalid_poll_secs() {
        let error = config_from(&[
            ("TELEGRAM_BOT_TOKEN", "token"),
            ("TELEGRAM_OUTBOX_POLL_SECS", "nope"),
        ])
        .unwrap_err();
        assert!(
            error.contains("TELEGRAM_OUTBOX_POLL_SECS"),
            "unexpected: {error}"
        );

        let error = config_from(&[
            ("TELEGRAM_BOT_TOKEN", "token"),
            ("TELEGRAM_OUTBOX_POLL_SECS", "0"),
        ])
        .unwrap_err();
        assert!(
            error.contains("TELEGRAM_OUTBOX_POLL_SECS"),
            "unexpected: {error}"
        );
    }

    #[test]
    fn parse_allowed_chat_ids_handles_empty_and_whitespace() {
        assert_eq!(parse_allowed_chat_ids("").unwrap(), Vec::<i64>::new());
        assert_eq!(parse_allowed_chat_ids("  , ,").unwrap(), Vec::<i64>::new());
        assert_eq!(parse_allowed_chat_ids("42").unwrap(), vec![42]);
        assert_eq!(parse_allowed_chat_ids("-7, 8,").unwrap(), vec![-7, 8]);
    }

    #[test]
    fn parse_allowed_chat_ids_rejects_garbage() {
        let error = parse_allowed_chat_ids("12,abc").unwrap_err();
        assert!(error.contains("abc"), "unexpected: {error}");
    }

    #[test]
    fn chunk_message_splits_on_byte_limit() {
        let chunks = chunk_message("abcdefghij", 4);
        assert_eq!(chunks, vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn chunk_message_returns_single_chunk_when_short() {
        assert_eq!(chunk_message("hi", 4000), vec!["hi"]);
    }

    #[test]
    fn chunk_message_returns_no_chunks_for_empty_input() {
        assert!(chunk_message("", 4000).is_empty());
    }

    #[test]
    fn chunk_message_never_breaks_utf8_boundaries() {
        // 'é' is two bytes; a 3-byte limit must yield whole characters only.
        let text = "éééé";
        let chunks = chunk_message(text, 3);
        assert_eq!(chunks, vec!["é", "é", "é", "é"]);
        for chunk in &chunks {
            assert!(chunk.len() <= 3);
        }
        assert_eq!(chunks.concat(), text);
    }

    #[test]
    fn chunk_message_emits_oversized_single_char() {
        let chunks = chunk_message("éa", 1);
        assert_eq!(chunks, vec!["é", "a"]);
    }

    #[test]
    fn extract_message_ignores_updates_without_text() {
        let update: Update = serde_json::from_value(json!({ "update_id": 1 })).unwrap();
        assert!(extract_message(&update).is_none());

        let update: Update = serde_json::from_value(json!({
            "update_id": 2,
            "message": { "chat": { "id": 5 } }
        }))
        .unwrap();
        assert!(extract_message(&update).is_none());

        let update: Update = serde_json::from_value(json!({
            "update_id": 3,
            "message": { "chat": { "id": 5 }, "text": "" }
        }))
        .unwrap();
        assert!(extract_message(&update).is_none());
    }

    #[test]
    fn extract_message_prefers_username_then_first_name() {
        let update: Update = serde_json::from_value(json!({
            "update_id": 10,
            "message": {
                "chat": { "id": 99 },
                "from": { "id": 7, "first_name": "Ada", "username": "ada" },
                "text": "hello"
            }
        }))
        .unwrap();
        let message = extract_message(&update).unwrap();
        assert_eq!(message.chat_id, 99);
        assert_eq!(message.user_id, Some(7));
        assert_eq!(message.user_name.as_deref(), Some("ada"));
        assert_eq!(message.text, "hello");

        let update: Update = serde_json::from_value(json!({
            "update_id": 11,
            "message": {
                "chat": { "id": 99 },
                "from": { "id": 7, "first_name": "Ada" },
                "text": "hello"
            }
        }))
        .unwrap();
        assert_eq!(
            extract_message(&update).unwrap().user_name.as_deref(),
            Some("Ada")
        );

        let update: Update = serde_json::from_value(json!({
            "update_id": 12,
            "message": { "chat": { "id": 99 }, "text": "hello" }
        }))
        .unwrap();
        let message = extract_message(&update).unwrap();
        assert_eq!(message.user_id, None);
        assert_eq!(message.user_name, None);
    }

    #[test]
    fn advance_offset_moves_past_highest_seen_update() {
        assert_eq!(advance_offset(0, 41), 42);
        assert_eq!(advance_offset(100, 41), 100);
        assert_eq!(advance_offset(0, -1), 0);
    }

    #[test]
    fn unauthorized_notice_includes_chat_id() {
        let notice = unauthorized_notice(1234);
        assert!(notice.contains("1234"));
        assert!(notice.contains("TELEGRAM_ALLOWED_CHAT_IDS"));
    }

    #[test]
    fn build_run_request_includes_metadata() {
        let message = IncomingMessage {
            chat_id: -100,
            user_id: Some(7),
            user_name: Some("ada".to_string()),
            text: "hi".to_string(),
        };
        let body = build_run_request(&message);
        assert_eq!(body["text"], json!("hi"));
        assert_eq!(
            body["metadata"],
            json!({
                "userId": "7",
                "userName": "ada",
                "channel": "telegram",
                "chatId": "-100"
            })
        );

        let anonymous = IncomingMessage {
            chat_id: 1,
            user_id: None,
            user_name: None,
            text: "hi".to_string(),
        };
        let body = build_run_request(&anonymous);
        assert_eq!(
            body["metadata"],
            json!({ "channel": "telegram", "chatId": "1" })
        );
    }

    #[test]
    fn reply_text_maps_success_and_error() {
        let success: RunResponse = serde_json::from_value(json!({
            "agent": {},
            "result": { "status": "success", "data": { "text": "pong" } }
        }))
        .unwrap();
        assert_eq!(reply_text(&success), "pong");

        let empty: RunResponse = serde_json::from_value(json!({
            "result": { "status": "success", "data": null, "error": null }
        }))
        .unwrap();
        assert_eq!(reply_text(&empty), "(no response)");

        let failure: RunResponse = serde_json::from_value(json!({
            "result": { "status": "error", "data": null, "error": "boom" }
        }))
        .unwrap();
        assert_eq!(reply_text(&failure), "Error: boom");
    }

    #[test]
    fn agents_response_parses_and_matches_name() {
        let response: AgentsResponse = serde_json::from_value(json!({
            "agents": [
                { "state": { "id": "a1", "name": "assistant", "config": {} } },
                { "state": { "id": "a2", "name": "other", "config": {}, "extra": 1 } }
            ]
        }))
        .unwrap();
        assert_eq!(response.agents.len(), 2);
        let found = response
            .agents
            .iter()
            .find(|entry| entry.state.name == "assistant")
            .unwrap();
        assert_eq!(found.state.id, "a1");
    }

    #[test]
    fn outbox_message_parses_camel_case_and_formats() {
        let response: OutboxResponse = serde_json::from_value(json!({
            "messages": [
                { "id": "m1", "job": "reminder", "text": "stretch", "createdAtMs": 42 }
            ]
        }))
        .unwrap();
        let message = &response.messages[0];
        assert_eq!(message.created_at_ms, 42);
        assert_eq!(format_outbox_message(message), "(reminder) stretch");
    }
}
