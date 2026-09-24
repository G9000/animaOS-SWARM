//! Per-response session views (spec §3.2, §3.3): derived fields, lists with
//! search and cursors, and message pages that merge the history store with
//! the control plane's hot tail. The state lock is never held across a
//! history-store call, and an unreadable store degrades a view to the hot tail.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};

use anima_core::{Message, MessageRole};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use tracing::warn;

use super::{
    hidden_message_ids, is_inbound_message, preview_text, schedule_id_of_room, SessionCapabilities,
    SessionKind, SessionRecord,
};
use crate::agent_runs::config_helper_parent;
use crate::app::SharedDaemonState;
use crate::history::{
    search_snippet, search_tokens, searchable_text, text_matches, HistoryStore, MessageOrder,
    MessagePageQuery,
};
use crate::state::DaemonState;

/// Sessions per list page by default and at most (spec §3.3).
pub(crate) const DEFAULT_SESSION_PAGE: usize = 50;
pub(crate) const MAX_SESSION_PAGE: usize = 200;
/// Messages per page by default and at most.
pub(crate) const DEFAULT_MESSAGE_PAGE: usize = 50;
pub(crate) const MAX_MESSAGE_PAGE: usize = 200;
/// The longest accepted search query, in characters.
pub(crate) const MAX_SEARCH_QUERY_CHARS: usize = 200;
/// Sessions one search returns at most (Controller ruling, M2 pre-flight
/// audit): the store groups by session before this limit applies, so it caps
/// sessions, not the rows the store scans to find them.
const SEARCH_ROW_LIMIT: usize = 500;
/// History rows read for a preview when the hot tail has none.
const PREVIEW_ROW_LIMIT: usize = 20;

/// The filters of `GET /api/agents/{id}/sessions`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionListQuery {
    pub(crate) kind: Option<SessionKind>,
    /// `false` lists unarchived sessions; `true` lists only archived ones.
    pub(crate) archived: bool,
    pub(crate) q: Option<String>,
    pub(crate) cursor: Option<SessionCursor>,
    pub(crate) limit: usize,
    /// Adds helper and delegated sessions whose `parentAgentId` is the agent.
    pub(crate) include_helpers: bool,
}

impl Default for SessionListQuery {
    fn default() -> Self {
        Self {
            kind: None,
            archived: false,
            q: None,
            cursor: None,
            limit: DEFAULT_SESSION_PAGE,
            include_helpers: true,
        }
    }
}

/// A position in the list order: newest activity first, then agent and id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionCursor {
    pub(crate) last_activity_at_ms: u64,
    pub(crate) agent_id: String,
    pub(crate) session_id: String,
}

impl SessionCursor {
    fn of(record: &SessionRecord) -> Self {
        Self {
            last_activity_at_ms: record.last_activity_at_ms,
            agent_id: record.agent_id.clone(),
            session_id: record.id.clone(),
        }
    }

    pub(crate) fn encode(&self) -> String {
        let value = serde_json::json!([self.last_activity_at_ms, self.agent_id, self.session_id]);
        URL_SAFE_NO_PAD.encode(value.to_string())
    }

    pub(crate) fn decode(value: &str) -> Option<Self> {
        let bytes = URL_SAFE_NO_PAD.decode(value).ok()?;
        let (last_activity_at_ms, agent_id, session_id) =
            serde_json::from_slice::<(u64, String, String)>(&bytes).ok()?;
        Some(Self {
            last_activity_at_ms,
            agent_id,
            session_id,
        })
    }

    fn key(&self) -> (Reverse<u64>, &str, &str) {
        (
            Reverse(self.last_activity_at_ms),
            self.agent_id.as_str(),
            self.session_id.as_str(),
        )
    }
}

fn sort_key(record: &SessionRecord) -> (Reverse<u64>, &str, &str) {
    (
        Reverse(record.last_activity_at_ms),
        record.agent_id.as_str(),
        record.id.as_str(),
    )
}

/// Why a list item matched a search; `message_id` is `None` for a title match.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionMatch {
    pub(crate) message_id: Option<String>,
    pub(crate) snippet: String,
}

/// A session record with its derived fields (spec §3.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionView {
    pub(crate) record: SessionRecord,
    pub(crate) message_count: usize,
    pub(crate) preview: Option<String>,
    pub(crate) active_runs: usize,
    pub(crate) unread: bool,
    pub(crate) capabilities: SessionCapabilities,
    pub(crate) matched: Option<SessionMatch>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionPage {
    pub(crate) sessions: Vec<SessionView>,
    pub(crate) next_cursor: Option<String>,
}

/// `GET …/sessions/{sid}/messages` parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MessagePageRequest {
    pub(crate) before: Option<String>,
    pub(crate) limit: usize,
    pub(crate) include_hidden: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PageMessage {
    pub(crate) message: Message,
    /// Part of a silent check-in turn.
    pub(crate) hidden: bool,
}

/// One page of a session's messages, oldest first.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MessagePage {
    pub(crate) messages: Vec<PageMessage>,
    pub(crate) next_before: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MessagePageError {
    NotFound,
    BeforeNotFound,
    /// The page needs the history store and it could not be read.
    Unavailable,
}

/// What the state lock yields for one session; the rest is added without it.
struct Candidate {
    record: SessionRecord,
    /// Visible hot messages the store is not known to hold.
    unmirrored_visible: usize,
    /// Every visible hot message (the count when the store cannot be read).
    hot_visible: usize,
    preview: Option<String>,
    unread: bool,
    active_runs: usize,
    capabilities: SessionCapabilities,
    matched: Option<SessionMatch>,
}

fn preview_from_newest<'a>(newest_first: impl Iterator<Item = &'a Message>) -> Option<String> {
    newest_first
        .filter(|message| matches!(message.role, MessageRole::User | MessageRole::Assistant))
        .find_map(|message| preview_text(searchable_text(message)))
}

fn hot_rooms<'a>(state: &'a DaemonState, agent_id: &str) -> HashMap<&'a str, Vec<&'a Message>> {
    let mut rooms: HashMap<&str, Vec<&Message>> = HashMap::new();
    if let Some(runtime) = state.agents.get(agent_id) {
        for message in runtime.messages() {
            rooms
                .entry(message.room_id.as_str())
                .or_default()
                .push(message);
        }
    }
    rooms
}

fn candidate(
    state: &DaemonState,
    record: &SessionRecord,
    hot: &[&Message],
    tokens: &[String],
) -> Candidate {
    let hidden_ids = hidden_message_ids(hot.iter().copied());
    let visible = hot
        .iter()
        .copied()
        .filter(|message| !hidden_ids.contains(&message.id))
        .collect::<Vec<_>>();
    let read_through = record.last_read_at_ms.unwrap_or(0);
    let schedule_exists = record.kind == SessionKind::Checkin
        && schedule_id_of_room(record.room_id()).is_some_and(|id| state.schedules.contains_key(id));
    let matched = if tokens.is_empty() {
        None
    } else {
        visible
            .iter()
            .rev()
            .find(|message| text_matches(searchable_text(message), tokens))
            .map(|message| SessionMatch {
                message_id: Some(message.id.clone()),
                snippet: search_snippet(searchable_text(message), tokens),
            })
    };
    Candidate {
        record: record.clone(),
        unmirrored_visible: visible
            .iter()
            .filter(|message| !state.history.is_mirrored(&message.id))
            .count(),
        hot_visible: visible.len(),
        preview: preview_from_newest(visible.iter().rev().copied()),
        unread: visible.iter().any(|message| {
            (message.role == MessageRole::Assistant || is_inbound_message(message))
                && message.created_at_ms > read_through
        }),
        active_runs: state
            .runs
            .active_count_for_session(&record.agent_id, &record.id),
        capabilities: record.capabilities(schedule_exists),
        matched,
    }
}

fn collect_candidates(
    state: &DaemonState,
    agent_id: &str,
    include_helpers: bool,
    select: impl Fn(&SessionRecord) -> bool,
    tokens: &[String],
) -> Vec<Candidate> {
    let helpers = if include_helpers {
        state
            .agents
            .iter()
            .filter(|(_, runtime)| config_helper_parent(runtime.config()) == Some(agent_id))
            .map(|(id, _)| id.as_str())
            .collect::<HashSet<_>>()
    } else {
        HashSet::new()
    };
    let records = state
        .sessions
        .records()
        .filter(|record| {
            state.agents.contains_key(&record.agent_id)
                && (record.agent_id == agent_id
                    || (include_helpers
                        && (record.parent_agent_id.as_deref() == Some(agent_id)
                            || helpers.contains(record.agent_id.as_str()))))
                && select(record)
        })
        .collect::<Vec<_>>();
    let mut rooms_by_agent: HashMap<&str, HashMap<&str, Vec<&Message>>> = HashMap::new();
    for record in &records {
        rooms_by_agent
            .entry(record.agent_id.as_str())
            .or_insert_with(|| hot_rooms(state, &record.agent_id));
    }
    records
        .into_iter()
        .map(|record| {
            let hot = rooms_by_agent
                .get(record.agent_id.as_str())
                .and_then(|rooms| rooms.get(record.room_id()))
                .map(Vec::as_slice)
                .unwrap_or_default();
            candidate(state, record, hot, tokens)
        })
        .collect()
}

/// The newest history match per session, keyed by `(agentId, sessionId)`.
async fn store_matches(
    store: &dyn HistoryStore,
    candidates: &[Candidate],
    query: &str,
    tokens: &[String],
) -> HashMap<(String, String), SessionMatch> {
    let mut agent_ids = candidates
        .iter()
        .map(|candidate| candidate.record.agent_id.clone())
        .collect::<Vec<_>>();
    agent_ids.sort();
    agent_ids.dedup();
    if agent_ids.is_empty() {
        return HashMap::new();
    }
    let rows = match store
        .search_sessions(&agent_ids, query, SEARCH_ROW_LIMIT)
        .await
    {
        Ok(rows) => rows,
        Err(error) => {
            warn!(error = %error, "session search could not read the history store; searching the hot tail only");
            return HashMap::new();
        }
    };
    let mut matches = HashMap::new();
    for row in rows {
        matches
            .entry((row.agent_id.clone(), row.session_id.clone()))
            .or_insert_with(|| SessionMatch {
                message_id: Some(row.message.id.clone()),
                snippet: search_snippet(searchable_text(&row.message), tokens),
            });
    }
    matches
}

async fn stored_preview(store: &dyn HistoryStore, record: &SessionRecord) -> Option<String> {
    let rows = store
        .page_messages(&MessagePageQuery {
            agent_id: record.agent_id.clone(),
            session_id: record.id.clone(),
            before: None,
            limit: PREVIEW_ROW_LIMIT,
            include_hidden: false,
        })
        .await
        .ok()?;
    preview_from_newest(rows.iter().map(|row| &row.message))
}

/// Adds the store's counts to the hot tail's and finds previews the hot tail lacks.
async fn complete(store: &dyn HistoryStore, candidates: Vec<Candidate>) -> Vec<SessionView> {
    let mut by_agent: HashMap<String, Vec<String>> = HashMap::new();
    for candidate in &candidates {
        by_agent
            .entry(candidate.record.agent_id.clone())
            .or_default()
            .push(candidate.record.id.clone());
    }
    let mut counts: HashMap<String, HashMap<String, usize>> = HashMap::new();
    for (agent_id, session_ids) in by_agent {
        match store.visible_message_counts(&agent_id, &session_ids).await {
            Ok(agent_counts) => {
                counts.insert(agent_id, agent_counts);
            }
            Err(error) => {
                warn!(agent_id = %agent_id, error = %error, "session counts could not read the history store; showing the hot tail");
            }
        }
    }
    let mut views = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let stored = counts
            .get(&candidate.record.agent_id)
            .map(|agent_counts| agent_counts.get(&candidate.record.id).copied().unwrap_or(0));
        let message_count = match stored {
            Some(stored) => stored + candidate.unmirrored_visible,
            None => candidate.hot_visible,
        };
        let mut preview = candidate.preview;
        if preview.is_none() && stored.unwrap_or(0) > 0 {
            preview = stored_preview(store, &candidate.record).await;
        }
        views.push(SessionView {
            record: candidate.record,
            message_count,
            preview,
            active_runs: candidate.active_runs,
            unread: candidate.unread,
            capabilities: candidate.capabilities,
            matched: candidate.matched,
        });
    }
    views
}

/// `GET /api/agents/{id}/sessions`; `None` when the agent does not exist.
pub(crate) async fn list_sessions(
    state: &SharedDaemonState,
    agent_id: &str,
    query: &SessionListQuery,
) -> Option<SessionPage> {
    let tokens = query.q.as_deref().map(search_tokens).unwrap_or_default();
    let (mut candidates, store) = {
        let guard = state.read().await;
        if !guard.agents.contains_key(agent_id) {
            return None;
        }
        let select = |record: &SessionRecord| {
            record.archived == query.archived && query.kind.map_or(true, |kind| record.kind == kind)
        };
        (
            collect_candidates(&guard, agent_id, query.include_helpers, select, &tokens),
            guard.history.store(),
        )
    };
    if let Some(q) = query.q.as_deref() {
        let mut stored = if tokens.is_empty() {
            HashMap::new()
        } else {
            store_matches(&*store, &candidates, q, &tokens).await
        };
        candidates.retain_mut(|candidate| {
            let key = (
                candidate.record.agent_id.clone(),
                candidate.record.id.clone(),
            );
            let found = candidate
                .matched
                .take()
                .or_else(|| stored.remove(&key))
                .or_else(|| {
                    text_matches(&candidate.record.title, &tokens).then(|| SessionMatch {
                        message_id: None,
                        snippet: candidate.record.title.clone(),
                    })
                });
            candidate.matched = found;
            candidate.matched.is_some()
        });
    }
    candidates.sort_by(|left, right| sort_key(&left.record).cmp(&sort_key(&right.record)));
    if let Some(cursor) = &query.cursor {
        let after = cursor.key();
        candidates.retain(|candidate| sort_key(&candidate.record) > after);
    }
    let has_more = candidates.len() > query.limit;
    candidates.truncate(query.limit);
    let next_cursor = if has_more {
        candidates
            .last()
            .map(|candidate| SessionCursor::of(&candidate.record).encode())
    } else {
        None
    };
    Some(SessionPage {
        sessions: complete(&*store, candidates).await,
        next_cursor,
    })
}

/// One session with its derived fields; `None` when the agent or session is missing.
pub(crate) async fn session_view(
    state: &SharedDaemonState,
    agent_id: &str,
    session_id: &str,
) -> Option<SessionView> {
    let (candidate, store) = {
        let guard = state.read().await;
        let runtime = guard.agents.get(agent_id)?;
        let record = guard.sessions.get(agent_id, session_id)?;
        let hot = runtime
            .messages()
            .iter()
            .filter(|message| message.room_id == record.room_id())
            .collect::<Vec<_>>();
        (candidate(&guard, record, &hot, &[]), guard.history.store())
    };
    complete(&*store, vec![candidate]).await.pop()
}

/// `GET …/sessions/{sid}/messages`: the history store's page merged with the
/// hot tail by id (spec §3.3).
pub(crate) async fn session_messages(
    state: &SharedDaemonState,
    agent_id: &str,
    session_id: &str,
    request: &MessagePageRequest,
) -> Result<MessagePage, MessagePageError> {
    let (hot, store) = {
        let guard = state.read().await;
        let runtime = guard
            .agents
            .get(agent_id)
            .ok_or(MessagePageError::NotFound)?;
        let record = guard
            .sessions
            .get(agent_id, session_id)
            .ok_or(MessagePageError::NotFound)?;
        let hot = runtime
            .messages()
            .iter()
            .filter(|message| message.room_id == record.room_id())
            .cloned()
            .collect::<Vec<_>>();
        (hot, guard.history.store())
    };
    let hidden_ids = hidden_message_ids(hot.iter());
    let before = match request.before.as_deref() {
        None => None,
        Some(before_id) => match hot.iter().find(|message| message.id == before_id) {
            Some(message) => Some(MessageOrder::of(message)),
            None => match store.get_message(agent_id, session_id, before_id).await {
                Ok(Some(row)) => Some(row.order()),
                Ok(None) => return Err(MessagePageError::BeforeNotFound),
                Err(error) => {
                    warn!(error = %error, "a message page could not read the history store");
                    return Err(MessagePageError::Unavailable);
                }
            },
        },
    };
    let hot_ids = hot
        .iter()
        .map(|message| message.id.clone())
        .collect::<HashSet<_>>();
    let mut page = hot
        .into_iter()
        .filter(|message| {
            before
                .as_ref()
                .map_or(true, |before| MessageOrder::of(message) < *before)
        })
        .map(|message| PageMessage {
            hidden: hidden_ids.contains(&message.id),
            message,
        })
        .filter(|entry| request.include_hidden || !entry.hidden)
        .collect::<Vec<_>>();
    let query = MessagePageQuery {
        agent_id: agent_id.to_string(),
        session_id: session_id.to_string(),
        before,
        limit: request.limit + 1,
        include_hidden: request.include_hidden,
    };
    match store.page_messages(&query).await {
        Ok(rows) => page.extend(
            rows.into_iter()
                .filter(|row| !hot_ids.contains(&row.message.id))
                .map(|row| PageMessage {
                    hidden: row.hidden,
                    message: row.message,
                }),
        ),
        Err(error) => {
            warn!(error = %error, "a message page could not read the history store; showing the hot tail");
        }
    }
    page.sort_by(|left, right| {
        MessageOrder::of(&right.message).cmp(&MessageOrder::of(&left.message))
    });
    let has_more = page.len() > request.limit;
    page.truncate(request.limit);
    page.reverse();
    let next_before = if has_more {
        page.first().map(|entry| entry.message.id.clone())
    } else {
        None
    };
    Ok(MessagePage {
        messages: page,
        next_before,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use anima_core::MessageRole;
    use tokio::sync::RwLock;

    use super::*;
    use crate::history::conformance::{history_message, FlakyHistoryStore};
    use crate::history::{HistoryService, HistoryStore};
    use crate::sessions::test_support::{agent_config, checkin_prompt, message, seed_messages};
    use crate::sessions::{SessionKind, SessionOrigin, SessionRecord, TitleSource};
    use crate::state::DaemonState;

    fn session(
        agent_id: &str,
        room_id: &str,
        kind: SessionKind,
        title: &str,
        at: u64,
    ) -> SessionRecord {
        let origin = match kind {
            SessionKind::Chat => SessionOrigin::Web,
            SessionKind::Telegram => SessionOrigin::Telegram,
            SessionKind::Checkin => SessionOrigin::Schedule,
            SessionKind::Job => SessionOrigin::Job,
            SessionKind::Helper => SessionOrigin::Delegation,
        };
        SessionRecord::new(
            agent_id,
            room_id,
            kind,
            origin,
            title.into(),
            TitleSource::System,
            at,
        )
    }

    fn ids(page: &MessagePage) -> Vec<String> {
        page.messages
            .iter()
            .map(|entry| entry.message.id.clone())
            .collect()
    }

    fn first_page(include_hidden: bool) -> MessagePageRequest {
        MessagePageRequest {
            before: None,
            limit: 10,
            include_hidden,
        }
    }

    #[tokio::test]
    async fn lists_sessions_newest_first_with_derived_fields_and_cursor_pages() {
        let mut daemon = DaemonState::new();
        let agent = daemon
            .create_agent(agent_config("companion"))
            .unwrap()
            .state
            .id;
        let mut plans = session(&agent, "chat:plans", SessionKind::Chat, "Plans", 10);
        plans.last_activity_at_ms = 30;
        plans.last_read_at_ms = Some(21);
        let mut bot = session(
            &agent,
            "telegram:bot",
            SessionKind::Telegram,
            "Telegram · @bot",
            5,
        );
        bot.last_activity_at_ms = 20;
        let mut old = session(&agent, "chat:old", SessionKind::Chat, "Old", 1);
        old.archived = true;
        for record in [plans, bot, old] {
            daemon.sessions.insert(record);
        }
        seed_messages(
            &mut daemon,
            &agent,
            vec![
                message(
                    &agent,
                    "p1",
                    "chat:plans",
                    MessageRole::User,
                    "Plan the offsite",
                    21,
                ),
                message(
                    &agent,
                    "p2",
                    "chat:plans",
                    MessageRole::Assistant,
                    "Here is a plan for the offsite",
                    30,
                ),
                message(
                    &agent,
                    "t1",
                    "telegram:bot",
                    MessageRole::Assistant,
                    "Morning!",
                    20,
                ),
            ],
        );
        let state = Arc::new(RwLock::new(daemon));

        let first = list_sessions(
            &state,
            &agent,
            &SessionListQuery {
                limit: 1,
                ..SessionListQuery::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(first.sessions.len(), 1);
        let plans = &first.sessions[0];
        assert_eq!(plans.record.id, "chat:plans");
        assert_eq!(plans.message_count, 2);
        assert_eq!(
            plans.preview.as_deref(),
            Some("Here is a plan for the offsite")
        );
        assert!(plans.unread, "the reply is newer than lastReadAtMs");
        assert!(plans.capabilities.delete);
        let cursor = SessionCursor::decode(first.next_cursor.as_deref().unwrap()).unwrap();

        let second = list_sessions(
            &state,
            &agent,
            &SessionListQuery {
                limit: 1,
                cursor: Some(cursor),
                ..SessionListQuery::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            second
                .sessions
                .iter()
                .map(|view| view.record.id.as_str())
                .collect::<Vec<_>>(),
            ["telegram:bot"]
        );
        assert!(!second.sessions[0].capabilities.delete);
        assert_eq!(
            second.next_cursor, None,
            "archived sessions are listed separately"
        );

        let archived = list_sessions(
            &state,
            &agent,
            &SessionListQuery {
                archived: true,
                ..SessionListQuery::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            archived
                .sessions
                .iter()
                .map(|view| view.record.id.as_str())
                .collect::<Vec<_>>(),
            ["chat:old"]
        );
        let telegram = list_sessions(
            &state,
            &agent,
            &SessionListQuery {
                kind: Some(SessionKind::Telegram),
                ..SessionListQuery::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(telegram.sessions.len(), 1);
        assert!(
            list_sessions(&state, "missing", &SessionListQuery::default())
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn lists_include_helper_sessions_of_the_agent_unless_excluded() {
        let mut daemon = DaemonState::new();
        let companion = daemon
            .create_agent(agent_config("companion"))
            .unwrap()
            .state
            .id;
        let specialist = daemon
            .create_agent(agent_config("specialist"))
            .unwrap()
            .state
            .id;
        daemon.sessions.insert(session(
            &companion,
            "chat:plans",
            SessionKind::Chat,
            "Plans",
            10,
        ));
        let mut delegated = session(
            &specialist,
            "room-9",
            SessionKind::Helper,
            "Draft a plan",
            12,
        );
        delegated.parent_agent_id = Some(companion.clone());
        delegated.parent_session_id = Some("chat:plans".into());
        daemon.sessions.insert(delegated);
        daemon.sessions.insert(session(
            &specialist,
            "chat:own",
            SessionKind::Chat,
            "Own chat",
            11,
        ));
        let state = Arc::new(RwLock::new(daemon));

        let with_helpers = list_sessions(&state, &companion, &SessionListQuery::default())
            .await
            .unwrap();
        assert_eq!(
            with_helpers
                .sessions
                .iter()
                .map(|view| (view.record.agent_id.as_str(), view.record.id.as_str()))
                .collect::<Vec<_>>(),
            [
                (specialist.as_str(), "room-9"),
                (companion.as_str(), "chat:plans")
            ]
        );
        assert!(
            !with_helpers.sessions[0].capabilities.send,
            "helper sessions are read-only"
        );
        let own = list_sessions(
            &state,
            &companion,
            &SessionListQuery {
                include_helpers: false,
                ..SessionListQuery::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(own.sessions.len(), 1);
    }

    #[tokio::test]
    async fn search_matches_hot_messages_then_the_history_store_then_titles() {
        let mut daemon = DaemonState::new();
        let agent = daemon
            .create_agent(agent_config("companion"))
            .unwrap()
            .state
            .id;
        for (room, title, at) in [
            ("chat:hot", "Groceries", 30),
            ("chat:cold", "Travel", 20),
            ("chat:title", "Budget review", 10),
            ("chat:none", "Nothing", 5),
        ] {
            daemon
                .sessions
                .insert(session(&agent, room, SessionKind::Chat, title, at));
        }
        seed_messages(
            &mut daemon,
            &agent,
            vec![message(
                &agent,
                "h1",
                "chat:hot",
                MessageRole::User,
                "Buy budget apples",
                30,
            )],
        );
        daemon
            .history
            .store()
            .upsert_messages(&[history_message(
                "c1",
                &agent,
                "chat:cold",
                MessageRole::Assistant,
                "The budget for Lisbon",
                20,
            )])
            .await
            .unwrap();
        let state = Arc::new(RwLock::new(daemon));

        let page = list_sessions(
            &state,
            &agent,
            &SessionListQuery {
                q: Some("budget".into()),
                ..SessionListQuery::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            page.sessions
                .iter()
                .map(|view| (
                    view.record.id.as_str(),
                    view.matched
                        .as_ref()
                        .map(|found| found.message_id.as_deref())
                ))
                .collect::<Vec<_>>(),
            [
                ("chat:hot", Some(Some("h1"))),
                ("chat:cold", Some(Some("c1"))),
                ("chat:title", Some(None)),
            ]
        );
        assert_eq!(
            page.sessions[0].matched.as_ref().unwrap().snippet,
            "Buy budget apples"
        );
        let wordless = list_sessions(
            &state,
            &agent,
            &SessionListQuery {
                q: Some("!!".into()),
                ..SessionListQuery::default()
            },
        )
        .await
        .unwrap();
        assert!(
            wordless.sessions.is_empty(),
            "a query without words matches nothing"
        );
    }

    #[tokio::test]
    async fn search_still_finds_a_session_whose_only_match_is_older_than_the_row_limit_elsewhere() {
        // Controller ruling 2 (M2 pre-flight audit): session search ranks
        // sessions by their newest matching message, so a session whose only
        // match is older than SEARCH_ROW_LIMIT matches elsewhere still shows.
        let mut daemon = DaemonState::new();
        let agent = daemon
            .create_agent(agent_config("companion"))
            .unwrap()
            .state
            .id;
        daemon.sessions.insert(session(
            &agent,
            "chat:busy",
            SessionKind::Chat,
            "Busy",
            2_000,
        ));
        daemon
            .sessions
            .insert(session(&agent, "chat:quiet", SessionKind::Chat, "Quiet", 1));
        let busy_rows = (0..=SEARCH_ROW_LIMIT as u64)
            .map(|n| {
                history_message(
                    &format!("busy-{n}"),
                    &agent,
                    "chat:busy",
                    MessageRole::User,
                    "deploy the build",
                    1_000 + n,
                )
            })
            .collect::<Vec<_>>();
        daemon
            .history
            .store()
            .upsert_messages(&busy_rows)
            .await
            .unwrap();
        daemon
            .history
            .store()
            .upsert_messages(&[history_message(
                "quiet-1",
                &agent,
                "chat:quiet",
                MessageRole::User,
                "deploy notes",
                1,
            )])
            .await
            .unwrap();
        let state = Arc::new(RwLock::new(daemon));

        let page = list_sessions(
            &state,
            &agent,
            &SessionListQuery {
                q: Some("deploy".into()),
                ..SessionListQuery::default()
            },
        )
        .await
        .unwrap();

        assert_eq!(
            page.sessions.iter().map(|view| view.record.id.as_str()).collect::<Vec<_>>(),
            ["chat:busy", "chat:quiet"],
            "a session with more than SEARCH_ROW_LIMIT matches must not crowd out an older session's only match"
        );
    }

    #[tokio::test]
    async fn message_pages_merge_the_store_with_the_hot_tail_without_duplicates() {
        let mut daemon = DaemonState::new();
        let agent = daemon
            .create_agent(agent_config("companion"))
            .unwrap()
            .state
            .id;
        daemon
            .sessions
            .insert(session(&agent, "chat:plans", SessionKind::Chat, "Plans", 1));
        // m1 and m2 were pruned: only the store has them.
        daemon
            .history
            .store()
            .upsert_messages(&[
                history_message("m1", &agent, "chat:plans", MessageRole::User, "one", 1),
                history_message("m2", &agent, "chat:plans", MessageRole::Assistant, "two", 2),
            ])
            .await
            .unwrap();
        seed_messages(
            &mut daemon,
            &agent,
            vec![
                message(&agent, "m3", "chat:plans", MessageRole::User, "three", 3),
                message(
                    &agent,
                    "m4",
                    "chat:plans",
                    MessageRole::Assistant,
                    "four",
                    4,
                ),
            ],
        );
        let state = Arc::new(RwLock::new(daemon));
        let history = state.read().await.history.clone();
        history
            .flush_once(&state, &tokio::sync::Mutex::new(()), 5)
            .await
            .unwrap(); // m3 and m4 are now in both places
        seed_messages(
            &mut *state.write().await,
            &agent,
            vec![
                message(&agent, "m5", "chat:plans", MessageRole::User, "five", 5),
                message(&agent, "m6", "chat:plans", MessageRole::Assistant, "six", 6),
            ],
        );

        let newest = session_messages(
            &state,
            &agent,
            "chat:plans",
            &MessagePageRequest {
                before: None,
                limit: 3,
                include_hidden: false,
            },
        )
        .await
        .unwrap();
        assert_eq!(ids(&newest), ["m4", "m5", "m6"]);
        assert_eq!(newest.next_before.as_deref(), Some("m4"));
        let older = session_messages(
            &state,
            &agent,
            "chat:plans",
            &MessagePageRequest {
                before: newest.next_before.clone(),
                limit: 3,
                include_hidden: false,
            },
        )
        .await
        .unwrap();
        assert_eq!(ids(&older), ["m1", "m2", "m3"]);
        assert_eq!(older.next_before, None);
        let view = session_view(&state, &agent, "chat:plans").await.unwrap();
        assert_eq!(
            view.message_count, 6,
            "store rows plus unmirrored hot messages, each once"
        );
        assert_eq!(
            session_messages(
                &state,
                &agent,
                "chat:plans",
                &MessagePageRequest {
                    before: Some("missing".into()),
                    limit: 3,
                    include_hidden: false,
                },
            )
            .await,
            Err(MessagePageError::BeforeNotFound)
        );
        assert_eq!(
            session_messages(&state, &agent, "chat:missing", &first_page(false)).await,
            Err(MessagePageError::NotFound)
        );
    }

    #[tokio::test]
    async fn silent_checkins_are_hidden_and_an_unreadable_store_leaves_the_hot_tail() {
        let flaky = Arc::new(FlakyHistoryStore::new());
        let mut daemon = DaemonState::new();
        daemon.set_history(HistoryService::new(flaky.clone()));
        let agent = daemon
            .create_agent(agent_config("companion"))
            .unwrap()
            .state
            .id;
        daemon.sessions.insert(session(
            &agent,
            "schedule:s1",
            SessionKind::Checkin,
            "Check-in · Check status",
            1,
        ));
        flaky
            .upsert_messages(&[history_message(
                "old",
                &agent,
                "schedule:s1",
                MessageRole::Assistant,
                "Earlier update",
                1,
            )])
            .await
            .unwrap();
        seed_messages(
            &mut daemon,
            &agent,
            vec![
                checkin_prompt(&agent, "c1", "schedule:s1", "s1", "Check status", 2),
                message(
                    &agent,
                    "c2",
                    "schedule:s1",
                    MessageRole::Assistant,
                    "CHECKIN_OK",
                    3,
                ),
                checkin_prompt(&agent, "c3", "schedule:s1", "s1", "Check status", 4),
                message(
                    &agent,
                    "c4",
                    "schedule:s1",
                    MessageRole::Assistant,
                    "Two tasks are overdue",
                    5,
                ),
            ],
        );
        let state = Arc::new(RwLock::new(daemon));

        let visible = session_messages(&state, &agent, "schedule:s1", &first_page(false))
            .await
            .unwrap();
        assert_eq!(ids(&visible), ["old", "c3", "c4"]);
        let everything = session_messages(&state, &agent, "schedule:s1", &first_page(true))
            .await
            .unwrap();
        assert_eq!(ids(&everything), ["old", "c1", "c2", "c3", "c4"]);
        assert!(everything.messages[1].hidden && everything.messages[2].hidden);

        flaky.set_failing(true);
        let hot_only = session_messages(&state, &agent, "schedule:s1", &first_page(false))
            .await
            .unwrap();
        assert_eq!(
            ids(&hot_only),
            ["c3", "c4"],
            "an unreadable store leaves the hot tail"
        );
        assert_eq!(
            session_messages(
                &state,
                &agent,
                "schedule:s1",
                &MessagePageRequest {
                    before: Some("old".into()),
                    limit: 10,
                    include_hidden: false,
                },
            )
            .await,
            Err(MessagePageError::Unavailable)
        );
        let view = session_view(&state, &agent, "schedule:s1").await.unwrap();
        assert_eq!(
            view.message_count, 2,
            "counts fall back to the visible hot messages"
        );
        assert_eq!(view.preview.as_deref(), Some("Two tasks are overdue"));
    }
}
