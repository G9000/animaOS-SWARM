//! History store (spec §13.1): the row-based, append-only record of every
//! committed message and terminal run. The outbox feeds it; session views
//! merge it with the control plane's hot tail.

#[cfg(test)]
pub(crate) mod conformance;
mod memory;
mod outbox;
mod postgres;
mod sqlite;

pub(crate) use memory::MemoryHistoryStore;
pub(crate) use outbox::{HistoryDeletion, HistoryService, HistoryWorker, SharedHistory};
#[cfg(test)]
pub(crate) use outbox::{HISTORY_FLUSH_INTERVAL, HISTORY_READINESS_GRACE_MS};
pub(crate) use postgres::PostgresHistoryStore;
pub(crate) use sqlite::SqliteHistoryStore;

use std::collections::{HashMap, HashSet};

use anima_core::{Message, MessageRole};
use async_trait::async_trait;

use crate::runs::RunRecord;

/// Rows per in-memory table in ephemeral mode (spec §13.1).
pub(crate) const EPHEMERAL_HISTORY_MAX_ROWS: usize = 100_000;
/// Searches use at most this many query words.
pub(crate) const MAX_SEARCH_TOKENS: usize = 8;
/// Search snippets hold at most this many characters, plus ellipses.
pub(crate) const MAX_SNIPPET_CHARS: usize = 160;

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
}
