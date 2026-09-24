//! History store (spec §13.1): the row-based, append-only record of every
//! committed message and terminal run. The outbox feeds it; session views
//! merge it with the control plane's hot tail.

#[cfg(test)]
pub(crate) mod conformance;
mod memory;

#[allow(unused_imports)] // Tasks 4-6 add stores that consume this.
pub(crate) use memory::MemoryHistoryStore;

use std::collections::{HashMap, HashSet};

use anima_core::Message;
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

/// Whether every token occurs in `text`, ignoring case.
pub(crate) fn text_matches(text: &str, tokens: &[String]) -> bool {
    let lowered = text.to_lowercase();
    !tokens.is_empty() && tokens.iter().all(|token| lowered.contains(token.as_str()))
}

/// A single-line excerpt around the first matching token.
pub(crate) fn search_snippet(text: &str, tokens: &[String]) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars = collapsed.chars().collect::<Vec<_>>();
    let lowered = chars
        .iter()
        .map(|character| character.to_lowercase().next().unwrap_or(*character))
        .collect::<Vec<_>>();
    let position = tokens
        .iter()
        .filter_map(|token| {
            let needle = token.chars().collect::<Vec<_>>();
            if needle.is_empty() || needle.len() > lowered.len() {
                return None;
            }
            lowered
                .windows(needle.len())
                .position(|window| window == needle.as_slice())
        })
        .min()
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
}
