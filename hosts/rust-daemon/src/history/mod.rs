//! History store (spec §13.1): the row-based, append-only record of every
//! committed message and terminal run. The outbox feeds it; session views
//! merge it with the control plane's hot tail.

#[cfg(test)]
pub(crate) mod conformance;
mod memory;
mod outbox;
mod postgres;
mod sqlite;
mod worker;

pub(crate) use memory::MemoryHistoryStore;
pub(crate) use outbox::{HistoryService, SharedHistory};
#[cfg(test)]
pub(crate) use outbox::{HISTORY_FLUSH_INTERVAL, HISTORY_READINESS_GRACE_MS};
pub(crate) use postgres::PostgresHistoryStore;
pub(crate) use sqlite::SqliteHistoryStore;
pub(crate) use worker::{HistoryWorker, HistoryWorkerOwner};

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex as StdMutex, MutexGuard};

use anima_core::{Message, MessageRole};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::runs::RunRecord;

/// Rows per in-memory table in ephemeral mode (spec §13.1).
pub(crate) const EPHEMERAL_HISTORY_MAX_ROWS: usize = 100_000;
/// Searches use at most this many query words.
pub(crate) const MAX_SEARCH_TOKENS: usize = 8;
/// Search snippets hold at most this many characters, plus ellipses.
pub(crate) const MAX_SNIPPET_CHARS: usize = 160;
/// The indexed/matched text of one message is capped to this many bytes
/// (final fix wave item B): Postgres's generated `search` column
/// (`to_tsvector('simple', text)`) fails with "string is too long for
/// tsvector" past roughly 1 MB of distinct words, and a single huge message
/// — a large `web_fetch` or `read_file` result — must not fail the whole
/// outbox batch forever. `record` (the full message) is never capped.
pub(crate) const MAX_INDEXED_TEXT_BYTES: usize = 64 * 1024;

/// One committed transcript message as the history store keeps it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HistoryMessage {
    pub(crate) agent_id: String,
    pub(crate) session_id: String,
    /// Part of a silent check-in turn (spec §3.3); excluded unless asked for.
    pub(crate) hidden: bool,
    pub(crate) message: Message,
}

impl HistoryMessage {
    pub(crate) fn order(&self) -> MessageOrder {
        MessageOrder::of(&self.message)
    }
}

/// Transcript order within a session: creation time, then the runtime's
/// per-process message counter (messages of one session are created one at
/// a time), then the id.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct MessageOrder {
    pub(crate) created_at_ms: u64,
    pub(crate) ordinal: u64,
    pub(crate) id: String,
}

impl MessageOrder {
    pub(crate) fn of(message: &Message) -> Self {
        Self {
            created_at_ms: message.created_at_ms,
            ordinal: message_ordinal(&message.id),
            id: message.id.clone(),
        }
    }
}

/// The counter of a runtime message id (`msg-<ms>-<n>`); 0 for other ids.
pub(crate) fn message_ordinal(id: &str) -> u64 {
    id.rsplit_once('-')
        .filter(|(prefix, _)| prefix.starts_with("msg-"))
        .and_then(|(_, counter)| counter.parse().ok())
        .unwrap_or(0)
}

/// One page of a session's messages, newest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MessagePageQuery {
    pub(crate) agent_id: String,
    pub(crate) session_id: String,
    /// Only messages strictly older than this one.
    pub(crate) before: Option<MessageOrder>,
    pub(crate) limit: usize,
    pub(crate) include_hidden: bool,
}

/// A history deletion the control plane has saved and the store may not have
/// applied yet: one session, or without `session_id` the whole agent. The
/// control plane keeps these as `pendingHistoryDeletions`, so a restart
/// replays them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HistoryDeletion {
    pub(crate) agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) session_id: Option<String>,
}

impl HistoryDeletion {
    pub(crate) fn session(agent_id: &str, session_id: &str) -> Self {
        Self {
            agent_id: agent_id.to_string(),
            session_id: Some(session_id.to_string()),
        }
    }

    pub(crate) fn agent(agent_id: &str) -> Self {
        Self {
            agent_id: agent_id.to_string(),
            session_id: None,
        }
    }
}

fn lock<T>(mutex: &StdMutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HistoryError(String);

impl HistoryError {
    pub(crate) fn new(message: impl std::fmt::Display) -> Self {
        Self(message.to_string())
    }

    pub(crate) fn message(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for HistoryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for HistoryError {}

impl From<std::io::Error> for HistoryError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error)
    }
}

impl From<serde_json::Error> for HistoryError {
    fn from(error: serde_json::Error) -> Self {
        Self::new(error)
    }
}

/// The history store (spec §13.1). Every write is idempotent by id.
#[async_trait]
pub(crate) trait HistoryStore: Send + Sync {
    /// `memory`, `sqlite`, or `postgres`.
    fn label(&self) -> &'static str;

    /// In-memory stores lose everything on restart, so hot-tail pruning stays off.
    fn is_ephemeral(&self) -> bool {
        false
    }

    async fn upsert_messages(&self, messages: &[HistoryMessage]) -> Result<(), HistoryError>;

    async fn upsert_runs(&self, runs: &[RunRecord]) -> Result<(), HistoryError>;

    async fn existing_message_ids(&self, ids: &[String]) -> Result<HashSet<String>, HistoryError>;

    async fn get_message(
        &self,
        agent_id: &str,
        session_id: &str,
        message_id: &str,
    ) -> Result<Option<HistoryMessage>, HistoryError>;

    async fn get_run(&self, run_id: &str) -> Result<Option<RunRecord>, HistoryError>;

    /// Newest first, strictly older than `query.before`.
    async fn page_messages(
        &self,
        query: &MessagePageQuery,
    ) -> Result<Vec<HistoryMessage>, HistoryError>;

    /// Visible (non-hidden) messages per session.
    async fn visible_message_counts(
        &self,
        agent_id: &str,
        session_ids: &[String],
    ) -> Result<HashMap<String, usize>, HistoryError>;

    /// Visible messages of these agents in which every query word appears as
    /// a word prefix, newest first.
    async fn search_messages(
        &self,
        agent_ids: &[String],
        query: &str,
        limit: usize,
    ) -> Result<Vec<HistoryMessage>, HistoryError>;

    /// The newest visible matching message per (agent, session) of these
    /// agents, newest session first, at most `limit` sessions (Controller
    /// ruling, M2 pre-flight audit): unlike [`Self::search_messages`], the
    /// session limit is applied after grouping, so a session whose only
    /// match is older than another session's flood of matches is still
    /// returned.
    async fn search_sessions(
        &self,
        agent_ids: &[String],
        query: &str,
        limit: usize,
    ) -> Result<Vec<HistoryMessage>, HistoryError>;

    /// Removes a session's messages, runs, and attachment records.
    async fn delete_session(&self, agent_id: &str, session_id: &str) -> Result<(), HistoryError>;

    /// Removes an agent's messages, runs, and attachment records in every
    /// session; usage rows stay (spec §3.3).
    async fn delete_agent(&self, agent_id: &str) -> Result<(), HistoryError>;
}

/// Converts a millisecond timestamp or counter to a store's signed column type.
fn to_i64(value: u64) -> Result<i64, HistoryError> {
    i64::try_from(value).map_err(|_| HistoryError::new("a timestamp or counter is out of range"))
}

/// The row value a store writes for a message's role.
fn role_name(role: MessageRole) -> &'static str {
    match role {
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::System => "system",
        MessageRole::Tool => "tool",
    }
}

/// The text a person reads for one message: a check-in prompt's text without
/// the scheduler's suffix, otherwise the message's own full text. It is never
/// capped: the export (spec §3.3, the full transcript), previews, and search
/// snippets show it. The suffix (`schedules::wrap_checkin_prompt`) is the
/// scheduler's instruction to the model, not something the owner wrote, and
/// it carries ordinary words ("scheduled", "reply", "exactly"...) that must
/// not make a check-in prompt match every query (review fix, M2 fix round 1).
pub(crate) fn display_text(message: &Message) -> &str {
    if crate::sessions::is_checkin_message(message) {
        crate::schedules::unwrap_checkin_prompt(&message.content.text)
    } else {
        &message.content.text
    }
}

/// The text a search indexes and matches for one message: [`display_text`]
/// capped to [`MAX_INDEXED_TEXT_BYTES`] on a char boundary. The cap (final
/// fix wave item B) keeps one oversized message — e.g. a large `web_fetch` or
/// `read_file` result — from failing Postgres's generated tsvector column.
/// Only search indexing and matching use it (the persisted stores' `text`
/// column, the in-memory store's matching, and the hot-tail matcher), so they
/// all agree on what is searchable; anything shown to a person uses
/// [`display_text`] instead (residual round R1). `record` (the full message)
/// is never capped.
pub(crate) fn searchable_text(message: &Message) -> &str {
    cap_at_byte_boundary(display_text(message), MAX_INDEXED_TEXT_BYTES)
}

/// `text` cut to at most `max_bytes` bytes, backing up to the nearest char
/// boundary so a multi-byte UTF-8 character is never split.
fn cap_at_byte_boundary(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Lowercase query words, at most `MAX_SEARCH_TOKENS`.
pub(crate) fn search_tokens(query: &str) -> Vec<String> {
    query
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_lowercase)
        .take(MAX_SEARCH_TOKENS)
        .collect()
}

/// Whether every token is a case-insensitive prefix of some word in `text`,
/// where words are split the same way `search_tokens` splits a query.
pub(crate) fn text_matches(text: &str, tokens: &[String]) -> bool {
    if tokens.is_empty() {
        return false;
    }
    let words = text
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    tokens
        .iter()
        .all(|token| words.iter().any(|word| word.starts_with(token.as_str())))
}

/// A single-line excerpt around the first matching token. Prefers a match
/// that starts a word over one buried inside another word.
pub(crate) fn search_snippet(text: &str, tokens: &[String]) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars = collapsed.chars().collect::<Vec<_>>();
    let lowered = chars
        .iter()
        .map(|character| character.to_lowercase().next().unwrap_or(*character))
        .collect::<Vec<_>>();
    let starts_word = |index: usize| index == 0 || !lowered[index - 1].is_alphanumeric();
    let occurrences = |token: &String| {
        let needle = token.chars().collect::<Vec<_>>();
        if needle.is_empty() || needle.len() > lowered.len() {
            return Vec::new();
        }
        lowered
            .windows(needle.len())
            .enumerate()
            .filter_map(|(index, window)| (window == needle.as_slice()).then_some(index))
            .collect::<Vec<_>>()
    };
    let all_positions = tokens.iter().flat_map(occurrences).collect::<Vec<_>>();
    let position = all_positions
        .iter()
        .copied()
        .filter(|&index| starts_word(index))
        .min()
        .or_else(|| all_positions.iter().copied().min())
        .unwrap_or(0);
    let start = position.saturating_sub(MAX_SNIPPET_CHARS / 3);
    let end = (start + MAX_SNIPPET_CHARS).min(chars.len());
    let mut snippet = chars[start..end].iter().collect::<String>();
    if start > 0 {
        snippet.insert(0, '…');
    }
    if end < chars.len() {
        snippet.push('…');
    }
    snippet
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_order_uses_the_runtime_counter_within_a_millisecond() {
        assert_eq!(message_ordinal("msg-1700-9"), 9);
        assert_eq!(message_ordinal("msg-1700-10"), 10);
        assert_eq!(message_ordinal("message-1"), 0);
        assert_eq!(message_ordinal("msg-x"), 0);
        let nine = MessageOrder {
            created_at_ms: 1_700,
            ordinal: 9,
            id: "msg-1700-9".into(),
        };
        let ten = MessageOrder {
            created_at_ms: 1_700,
            ordinal: 10,
            id: "msg-1700-10".into(),
        };
        assert!(nine < ten, "string order would put -10 first");
        assert!(
            ten < MessageOrder {
                created_at_ms: 1_701,
                ordinal: 0,
                id: "a".into(),
            }
        );
    }

    #[test]
    fn search_tokens_are_lowercase_words() {
        assert_eq!(
            search_tokens("  Deploy, the BUILD! "),
            ["deploy", "the", "build"]
        );
        assert!(search_tokens("!!! ...").is_empty());
        assert_eq!(search_tokens(&"word ".repeat(20)).len(), MAX_SEARCH_TOKENS);
        let tokens = search_tokens("build deploy");
        assert!(text_matches("We deployed the Build.", &tokens));
        assert!(!text_matches("We deployed it.", &tokens));
        assert!(!text_matches("anything", &[]));
        assert!(
            !text_matches("underdeployment", &search_tokens("deploy")),
            "a mid-word occurrence is not a word prefix"
        );
        assert!(text_matches("Re-deploy tonight", &search_tokens("deploy")));
    }

    #[test]
    fn snippets_center_on_the_first_match() {
        let text = format!("{}needle here{}", "a ".repeat(100), " b".repeat(100));
        let snippet = search_snippet(&text, &search_tokens("NEEDLE"));
        assert!(snippet.contains("needle here"), "{snippet}");
        assert!(
            snippet.starts_with('…') && snippet.ends_with('…'),
            "{snippet}"
        );
        assert!(snippet.chars().count() <= MAX_SNIPPET_CHARS + 2);
        assert_eq!(
            search_snippet("short\ntext", &search_tokens("short")),
            "short text"
        );
        assert_eq!(
            search_snippet("no match here", &search_tokens("zebra")),
            "no match here"
        );
    }

    #[test]
    fn snippet_prefers_a_word_start_match_over_an_earlier_mid_word_occurrence() {
        // "deploy" occurs mid-word in "underdeploy" near the start, and again
        // as a whole word after more than MAX_SNIPPET_CHARS filler.
        let text = format!("underdeploy {}real deploy here", "filler ".repeat(40));
        let snippet = search_snippet(&text, &search_tokens("deploy"));
        assert!(snippet.contains("real deploy here"), "{snippet}");
        assert!(!snippet.contains("underdeploy"), "{snippet}");
    }

    fn message_with_text(text: &str) -> Message {
        use anima_core::Content;
        Message {
            id: "msg-1-1".into(),
            agent_id: "agent-1".into(),
            room_id: "chat:a".into(),
            content: Content {
                text: text.into(),
                ..Content::default()
            },
            role: MessageRole::User,
            created_at_ms: 1,
        }
    }

    #[test]
    fn searchable_text_caps_at_the_byte_limit_on_a_char_boundary() {
        let short = message_with_text("short text");
        assert_eq!(searchable_text(&short), "short text");

        let over = message_with_text(&"a".repeat(MAX_INDEXED_TEXT_BYTES + 10));
        assert_eq!(
            searchable_text(&over).len(),
            MAX_INDEXED_TEXT_BYTES,
            "text past the cap is dropped"
        );

        // "é" is 2 bytes (0xC3 0xA9), placed so the cap falls in the middle
        // of it; the boundary search must back up rather than split it.
        let straddling = format!("{}é", "a".repeat(MAX_INDEXED_TEXT_BYTES - 1));
        let straddling_message = message_with_text(&straddling);
        let capped = searchable_text(&straddling_message);
        assert_eq!(
            capped,
            "a".repeat(MAX_INDEXED_TEXT_BYTES - 1),
            "a character split by the cap is dropped whole, not corrupted"
        );
        assert!(capped.len() < MAX_INDEXED_TEXT_BYTES);
    }

    #[test]
    fn display_text_is_never_capped_and_drops_the_checkin_suffix() {
        // Residual round R1: the export, previews, and snippets show the
        // whole message; only search indexing and matching use the cap.
        let long = "a".repeat(MAX_INDEXED_TEXT_BYTES + 10);
        assert_eq!(display_text(&message_with_text(&long)), long);

        let prompt = format!("Check {}", "b".repeat(MAX_INDEXED_TEXT_BYTES));
        let checkin = crate::sessions::test_support::checkin_prompt(
            "agent-1",
            "msg-1-1",
            "schedule:s1",
            "s1",
            &prompt,
            1,
        );
        assert_eq!(display_text(&checkin), prompt);
        assert_eq!(
            searchable_text(&checkin),
            &prompt[..MAX_INDEXED_TEXT_BYTES],
            "the index keeps the capped, suffix-free prompt"
        );
    }
}
