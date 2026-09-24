//! Sessions (spec §3): one conversation room of one agent plus its record.
//! A session id equals its room id, except for legacy rooms whose id is not a
//! valid session id; those map to a stable `legacy-room:<hash>` id and keep
//! their room on the record.

pub(crate) mod migration;

use std::collections::{HashMap, HashSet, VecDeque};

use anima_core::{DataValue, Message, MessageRole};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::runs::RunSource;

/// Session ids are at most 200 bytes of `[A-Za-z0-9._:-]` (spec §3.1).
pub(crate) const MAX_SESSION_ID_BYTES: usize = 200;
/// Titles are 1–120 characters of plain text (spec §3.2).
pub(crate) const MAX_SESSION_TITLE_CHARS: usize = 120;
/// Titles derived from a message stay short enough for the sidebar.
pub(crate) const DERIVED_TITLE_CHARS: usize = 60;
/// `preview` is the last visible message, at most 160 characters (spec §3.2).
pub(crate) const MAX_SESSION_PREVIEW_CHARS: usize = 160;
/// The title of a chat created before its first message.
pub(crate) const DEFAULT_CHAT_TITLE: &str = "New chat";
/// Prefix of the ids that stand in for invalid legacy room ids.
pub(crate) const LEGACY_ROOM_SESSION_PREFIX: &str = "legacy-room:";
/// Session creations per agent per minute (spec §14).
pub(crate) const MAX_SESSION_CREATIONS_PER_MINUTE: usize = 60;
/// Workspace check-ins run in `schedule:<id>` (spec §3.1, §9.2).
pub(crate) const SCHEDULE_ROOM_PREFIX: &str = "schedule:";
const SESSION_CREATION_WINDOW_MS: u64 = 60_000;
const DELEGATED_TASK_PREFIX: &str = "Task delegated by workspace manager ";

/// What a session is (spec §3.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SessionKind {
    Chat,
    Telegram,
    Checkin,
    Job,
    Helper,
}

impl SessionKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Telegram => "telegram",
            Self::Checkin => "checkin",
            Self::Job => "job",
            Self::Helper => "helper",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "chat" => Some(Self::Chat),
            "telegram" => Some(Self::Telegram),
            "checkin" => Some(Self::Checkin),
            "job" => Some(Self::Job),
            "helper" => Some(Self::Helper),
            _ => None,
        }
    }

    /// The capability table of spec §3.2. A check-in session can be deleted
    /// only once its automation is gone.
    pub(crate) const fn capabilities(self, schedule_exists: bool) -> SessionCapabilities {
        let (send, steer, rename, delete, compact) = match self {
            Self::Chat => (true, true, true, true, true),
            Self::Telegram => (true, false, true, false, true),
            Self::Checkin => (true, true, true, !schedule_exists, true),
            Self::Job | Self::Helper => (false, false, false, false, false),
        };
        SessionCapabilities {
            send,
            steer,
            stop: true,
            rename,
            archive: true,
            delete,
            compact,
            export: true,
        }
    }
}

/// Where a session came from (spec §3.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SessionOrigin {
    Web,
    Api,
    Telegram,
    Schedule,
    Job,
    Delegation,
    Peer,
}

impl SessionOrigin {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Web => "web",
            Self::Api => "api",
            Self::Telegram => "telegram",
            Self::Schedule => "schedule",
            Self::Job => "job",
            Self::Delegation => "delegation",
            Self::Peer => "peer",
        }
    }
}

/// Who set the title (spec §3.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TitleSource {
    FirstMessage,
    Generated,
    Owner,
    System,
}

impl TitleSource {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::FirstMessage => "first_message",
            Self::Generated => "generated",
            Self::Owner => "owner",
            Self::System => "system",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SessionCapabilities {
    pub(crate) send: bool,
    pub(crate) steer: bool,
    pub(crate) stop: bool,
    pub(crate) rename: bool,
    pub(crate) archive: bool,
    pub(crate) delete: bool,
    pub(crate) compact: bool,
    pub(crate) export: bool,
}

/// A compaction summary (spec §5.4); written from M3 on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionSummary {
    pub(crate) text: String,
    pub(crate) through_message_id: String,
    pub(crate) created_at_ms: u64,
    pub(crate) source_message_count: usize,
}

/// The latest turn outside the model's view (spec §5.3); written from M3 on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionContextTrimmed {
    pub(crate) dropped_through_message_id: String,
    pub(crate) at_ms: u64,
}

/// The stored session record (spec §3.2). Derived fields are computed per
/// response and never stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionRecord {
    pub(crate) id: String,
    pub(crate) agent_id: String,
    pub(crate) kind: SessionKind,
    pub(crate) origin: SessionOrigin,
    pub(crate) title: String,
    pub(crate) title_source: TitleSource,
    pub(crate) created_at_ms: u64,
    pub(crate) last_activity_at_ms: u64,
    #[serde(default)]
    pub(crate) last_read_at_ms: Option<u64>,
    #[serde(default)]
    pub(crate) archived: bool,
    #[serde(default)]
    pub(crate) parent_session_id: Option<String>,
    #[serde(default)]
    pub(crate) parent_run_id: Option<String>,
    #[serde(default)]
    pub(crate) parent_agent_id: Option<String>,
    #[serde(default)]
    pub(crate) summary: Option<SessionSummary>,
    #[serde(default)]
    pub(crate) context_trimmed: Option<SessionContextTrimmed>,
    /// The transcript room when it differs from `id`: a legacy room whose id
    /// is not a valid session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) room_id: Option<String>,
}

impl SessionRecord {
    pub(crate) fn new(
        agent_id: &str,
        room_id: &str,
        kind: SessionKind,
        origin: SessionOrigin,
        title: String,
        title_source: TitleSource,
        now_ms: u64,
    ) -> Self {
        let id = session_id_for_room(room_id);
        let room = (id != room_id).then(|| room_id.to_string());
        Self {
            id,
            agent_id: agent_id.to_string(),
            kind,
            origin,
            title,
            title_source,
            created_at_ms: now_ms,
            last_activity_at_ms: now_ms,
            last_read_at_ms: None,
            archived: false,
            parent_session_id: None,
            parent_run_id: None,
            parent_agent_id: None,
            summary: None,
            context_trimmed: None,
            room_id: room,
        }
    }

    /// The transcript room this session reads and writes.
    pub(crate) fn room_id(&self) -> &str {
        self.room_id.as_deref().unwrap_or(&self.id)
    }

    pub(crate) fn capabilities(&self, schedule_exists: bool) -> SessionCapabilities {
        self.kind.capabilities(schedule_exists)
    }
}

pub(crate) fn is_valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_SESSION_ID_BYTES
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

/// The session id of a transcript room: the room id itself when it is a valid
/// session id, otherwise a stable `legacy-room:<hash>` id (spec §3.1, F17).
pub(crate) fn session_id_for_room(room_id: &str) -> String {
    if is_valid_session_id(room_id) {
        return room_id.to_string();
    }
    let digest = Sha256::digest(room_id.as_bytes());
    let hex = digest
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("{LEGACY_ROOM_SESSION_PREFIX}{hex}")
}

/// A new web chat id, `chat:<uuid-v4>` (spec §3.1).
pub(crate) fn new_chat_session_id() -> String {
    format!("chat:{}", uuid::Uuid::new_v4())
}

pub(crate) fn schedule_room_id(schedule_id: &str) -> String {
    format!("{SCHEDULE_ROOM_PREFIX}{schedule_id}")
}

pub(crate) fn schedule_id_of_room(room_id: &str) -> Option<&str> {
    room_id.strip_prefix(SCHEDULE_ROOM_PREFIX)
}

pub(crate) fn connector_id_of_room(room_id: &str) -> Option<&str> {
    room_id.strip_prefix("telegram:")
}

pub(crate) fn job_id_of_room(room_id: &str) -> Option<&str> {
    room_id.strip_prefix("job:")
}

/// The sending agent of a `peer:<sender>:<recipient>` room.
pub(crate) fn peer_sender_of_room(room_id: &str) -> Option<&str> {
    room_id
        .strip_prefix("peer:")?
        .split(':')
        .next()
        .filter(|sender| !sender.is_empty())
}

/// Kind and origin of a room by the rules of spec §3.1.
pub(crate) fn kind_for_room(
    room_id: &str,
    source: Option<RunSource>,
    helper_agent: bool,
) -> (SessionKind, SessionOrigin) {
    if room_id.starts_with("telegram:") {
        return (SessionKind::Telegram, SessionOrigin::Telegram);
    }
    if room_id.starts_with(SCHEDULE_ROOM_PREFIX) {
        return (SessionKind::Checkin, SessionOrigin::Schedule);
    }
    if room_id.starts_with("job:") {
        return (SessionKind::Job, SessionOrigin::Job);
    }
    if room_id.starts_with("peer:") || source == Some(RunSource::Peer) {
        return (SessionKind::Helper, SessionOrigin::Peer);
    }
    if helper_agent || source == Some(RunSource::Delegation) {
        return (SessionKind::Helper, SessionOrigin::Delegation);
    }
    if room_id.starts_with("chat:")
        || room_id.starts_with("direct:")
        || source == Some(RunSource::Web)
    {
        return (SessionKind::Chat, SessionOrigin::Web);
    }
    (SessionKind::Chat, SessionOrigin::Api)
}

/// `text` cut to `max_chars` characters, ending in "…" when shortened.
pub(crate) fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut kept = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    while kept.ends_with(char::is_whitespace) {
        kept.pop();
    }
    kept.push('…');
    kept
}

fn single_line(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|character| !character.is_control())
        .collect()
}

/// A title from the first non-empty line of a message.
pub(crate) fn derived_title(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    let cleaned = single_line(line);
    (!cleaned.is_empty()).then(|| truncate_chars(&cleaned, DERIVED_TITLE_CHARS))
}

/// An owner-supplied title as stored: one line of plain text, 1–120 characters.
pub(crate) fn clean_owner_title(title: &str) -> Result<String, &'static str> {
    let cleaned = single_line(title);
    let chars = cleaned.chars().count();
    if chars == 0 || chars > MAX_SESSION_TITLE_CHARS {
        return Err("title must be 1 to 120 characters");
    }
    Ok(cleaned)
}

/// The sidebar preview of a message (spec §3.2).
pub(crate) fn preview_text(text: &str) -> Option<String> {
    let cleaned = single_line(text);
    (!cleaned.is_empty()).then(|| truncate_chars(&cleaned, MAX_SESSION_PREVIEW_CHARS))
}

/// The owner's task inside a delegated run's input.
pub(crate) fn delegated_task_text(text: &str) -> &str {
    if text.starts_with(DELEGATED_TASK_PREFIX) {
        text.split_once("\n\n")
            .map(|(_, task)| task)
            .unwrap_or(text)
    } else {
        text
    }
}

/// The manager id in a delegated task's preamble ("… manager <name> (<id>). …").
pub(crate) fn delegating_agent_id(text: &str) -> Option<&str> {
    let rest = text.strip_prefix(DELEGATED_TASK_PREFIX)?;
    let (head, _) = rest.split_once("). ")?;
    let (_, id) = head.rsplit_once(" (")?;
    (!id.trim().is_empty()).then_some(id)
}

/// What a session title can be derived from.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TitleContext<'a> {
    pub(crate) first_user_text: Option<&'a str>,
    pub(crate) schedule_prompt: Option<&'a str>,
    pub(crate) job_title: Option<&'a str>,
    pub(crate) bot_username: Option<&'a str>,
    pub(crate) peer_sender_name: Option<&'a str>,
}

/// The initial title of a session by kind (spec §3.2 `titleSource`).
pub(crate) fn session_title(
    kind: SessionKind,
    origin: SessionOrigin,
    context: &TitleContext<'_>,
) -> (String, TitleSource) {
    let labelled = |label: &str, detail: Option<String>| {
        detail
            .map(|detail| truncate_chars(&format!("{label} · {detail}"), MAX_SESSION_TITLE_CHARS))
            .unwrap_or_else(|| label.to_string())
    };
    match kind {
        SessionKind::Chat => (
            context
                .first_user_text
                .and_then(derived_title)
                .unwrap_or_else(|| DEFAULT_CHAT_TITLE.to_string()),
            TitleSource::FirstMessage,
        ),
        SessionKind::Telegram => (
            labelled(
                "Telegram",
                context
                    .bot_username
                    .map(|name| format!("@{}", name.trim_start_matches('@'))),
            ),
            TitleSource::System,
        ),
        SessionKind::Checkin => (
            labelled(
                "Check-in",
                context
                    .schedule_prompt
                    .or(context
                        .first_user_text
                        .map(crate::schedules::unwrap_checkin_prompt))
                    .and_then(derived_title),
            ),
            TitleSource::System,
        ),
        SessionKind::Job => (
            labelled("Job", context.job_title.and_then(derived_title)),
            TitleSource::System,
        ),
        SessionKind::Helper if origin == SessionOrigin::Peer => (
            context
                .peer_sender_name
                .and_then(derived_title)
                .map(|name| {
                    truncate_chars(&format!("Messages from {name}"), MAX_SESSION_TITLE_CHARS)
                })
                .unwrap_or_else(|| "Agent messages".to_string()),
            TitleSource::System,
        ),
        SessionKind::Helper => (
            context
                .first_user_text
                .map(delegated_task_text)
                .and_then(derived_title)
                .unwrap_or_else(|| "Helper task".to_string()),
            TitleSource::System,
        ),
    }
}

fn metadata_str<'a>(message: &'a Message, key: &str) -> Option<&'a str> {
    match message.content.metadata.as_ref()?.get(key) {
        Some(DataValue::String(value)) => Some(value),
        _ => None,
    }
}

/// A scheduled check-in prompt (tagged by the scheduler).
pub(crate) fn is_checkin_message(message: &Message) -> bool {
    message.role == MessageRole::User && metadata_str(message, "kind") == Some("checkin")
}

/// A message that arrived from a Telegram chat.
pub(crate) fn is_inbound_message(message: &Message) -> bool {
    message.role == MessageRole::User && metadata_str(message, "source") == Some("telegram")
}

/// A Telegram owner turn sent from the web console.
pub(crate) fn is_owner_web_turn(message: &Message) -> bool {
    message.role == MessageRole::User && metadata_str(message, "source") == Some("telegramThread")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GroupStart {
    Missing,
    Checkin,
    Other,
}

/// Messages hidden from session views: every message of a check-in turn
/// whose final reply is the silent sentinel (spec §3.3 "silent check-in
/// pairs"). A turn whose opening message was pruned hides only a bare
/// sentinel reply. `messages` are one room's messages in transcript order.
pub(crate) fn hidden_message_ids<'a>(
    messages: impl IntoIterator<Item = &'a Message>,
) -> HashSet<String> {
    let mut hidden = HashSet::new();
    let mut group: Vec<&Message> = Vec::new();
    let mut start = GroupStart::Missing;
    for message in messages {
        if message.role == MessageRole::User {
            close_group(&group, start, &mut hidden);
            group.clear();
            start = if is_checkin_message(message) {
                GroupStart::Checkin
            } else {
                GroupStart::Other
            };
        }
        group.push(message);
    }
    close_group(&group, start, &mut hidden);
    hidden
}

fn close_group(group: &[&Message], start: GroupStart, hidden: &mut HashSet<String>) {
    let Some(reply) = group
        .iter()
        .rev()
        .find(|message| message.role == MessageRole::Assistant)
    else {
        return;
    };
    if !crate::schedules::is_silent_checkin_reply(&reply.content.text) {
        return;
    }
    match start {
        GroupStart::Checkin => hidden.extend(group.iter().map(|message| message.id.clone())),
        GroupStart::Missing => {
            hidden.insert(reply.id.clone());
        }
        GroupStart::Other => {}
    }
}

/// What `record_commit` changed, so a rolled-back commit restores it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionCommitUndo {
    agent_id: String,
    session_id: String,
    last_activity_at_ms: u64,
    last_read_at_ms: Option<u64>,
    title: String,
    title_source: TitleSource,
}

/// Session records keyed by `(agentId, id)` (spec §3.2).
#[derive(Clone, Debug, Default)]
pub(crate) struct SessionRegistry {
    records: HashMap<(String, String), SessionRecord>,
}

impl SessionRegistry {
    fn key(agent_id: &str, session_id: &str) -> (String, String) {
        (agent_id.to_string(), session_id.to_string())
    }

    pub(crate) fn get(&self, agent_id: &str, session_id: &str) -> Option<&SessionRecord> {
        self.records.get(&Self::key(agent_id, session_id))
    }

    pub(crate) fn get_mut(
        &mut self,
        agent_id: &str,
        session_id: &str,
    ) -> Option<&mut SessionRecord> {
        self.records.get_mut(&Self::key(agent_id, session_id))
    }

    pub(crate) fn contains(&self, agent_id: &str, session_id: &str) -> bool {
        self.records.contains_key(&Self::key(agent_id, session_id))
    }

    pub(crate) fn insert(&mut self, record: SessionRecord) -> Option<SessionRecord> {
        self.records
            .insert((record.agent_id.clone(), record.id.clone()), record)
    }

    pub(crate) fn remove(&mut self, agent_id: &str, session_id: &str) -> Option<SessionRecord> {
        self.records.remove(&Self::key(agent_id, session_id))
    }

    pub(crate) fn records(&self) -> impl Iterator<Item = &SessionRecord> {
        self.records.values()
    }

    pub(crate) fn len(&self) -> usize {
        self.records.len()
    }

    /// Records to save, sorted, without sessions of agents that no longer exist.
    pub(crate) fn snapshot_records(&self, live_agents: &HashSet<String>) -> Vec<SessionRecord> {
        let mut records = self
            .records
            .values()
            .filter(|record| live_agents.contains(&record.agent_id))
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by(|left, right| {
            left.agent_id
                .cmp(&right.agent_id)
                .then_with(|| left.id.cmp(&right.id))
        });
        records
    }

    pub(crate) fn validate(records: &[SessionRecord]) -> Result<(), String> {
        let mut keys = HashSet::new();
        for record in records {
            if !is_valid_session_id(&record.id) {
                return Err(format!("session id '{}' is invalid", record.id));
            }
            if record.agent_id.trim().is_empty() {
                return Err(format!("session '{}' has an empty agent id", record.id));
            }
            if !keys.insert((record.agent_id.as_str(), record.id.as_str())) {
                return Err(format!(
                    "duplicate session '{}' for agent '{}'",
                    record.id, record.agent_id
                ));
            }
            let title_chars = record.title.chars().count();
            if title_chars == 0 || title_chars > MAX_SESSION_TITLE_CHARS {
                return Err(format!("session '{}' has an invalid title", record.id));
            }
            if let Some(room) = &record.room_id {
                if room.is_empty() || session_id_for_room(room) != record.id {
                    return Err(format!(
                        "session '{}' has a room that does not map to it",
                        record.id
                    ));
                }
            }
        }
        Ok(())
    }

    /// The registry after a restart: sessions of missing agents are dropped.
    pub(crate) fn restored(records: Vec<SessionRecord>, live_agents: &HashSet<String>) -> Self {
        let mut registry = Self::default();
        for record in records {
            if live_agents.contains(&record.agent_id) {
                registry.insert(record);
            }
        }
        registry
    }

    /// Advances a session for a committed run: activity moves to the newest
    /// *visible* message (a silent check-in exchange carries none, so it
    /// never bumps a heartbeat session to the top of the list — spec §3.3),
    /// the owner's own message marks the session read up to itself, and a
    /// placeholder chat title becomes the first message's title.
    pub(crate) fn record_commit(
        &mut self,
        agent_id: &str,
        session_id: &str,
        messages: &[Message],
        owner_authored: bool,
    ) -> Option<SessionCommitUndo> {
        let record = self.records.get_mut(&Self::key(agent_id, session_id))?;
        let undo = SessionCommitUndo {
            agent_id: agent_id.to_string(),
            session_id: session_id.to_string(),
            last_activity_at_ms: record.last_activity_at_ms,
            last_read_at_ms: record.last_read_at_ms,
            title: record.title.clone(),
            title_source: record.title_source,
        };
        let hidden = hidden_message_ids(messages.iter());
        if let Some(latest) = messages
            .iter()
            .filter(|message| !hidden.contains(&message.id))
            .map(|message| message.created_at_ms)
            .max()
        {
            record.last_activity_at_ms = record.last_activity_at_ms.max(latest);
        }
        let first_user = messages
            .iter()
            .find(|message| message.role == MessageRole::User);
        if owner_authored {
            if let Some(first_user) = first_user {
                record.last_read_at_ms = Some(
                    record
                        .last_read_at_ms
                        .unwrap_or(0)
                        .max(first_user.created_at_ms),
                );
            }
        }
        if record.kind == SessionKind::Chat
            && record.title_source == TitleSource::FirstMessage
            && record.title == DEFAULT_CHAT_TITLE
        {
            if let Some(title) = first_user.and_then(|message| derived_title(&message.content.text))
            {
                record.title = title;
            }
        }
        Some(undo)
    }

    pub(crate) fn revert_commit(&mut self, undo: SessionCommitUndo) {
        if let Some(record) = self
            .records
            .get_mut(&Self::key(&undo.agent_id, &undo.session_id))
        {
            record.last_activity_at_ms = undo.last_activity_at_ms;
            record.last_read_at_ms = undo.last_read_at_ms;
            record.title = undo.title;
            record.title_source = undo.title_source;
        }
    }
}

/// Session creations per agent per minute (spec §14); not persisted.
#[derive(Debug, Default)]
pub(crate) struct SessionCreateLimiter {
    windows: HashMap<String, VecDeque<u64>>,
}

impl SessionCreateLimiter {
    pub(crate) fn try_acquire(&mut self, agent_id: &str, now_ms: u64) -> bool {
        let window = self.windows.entry(agent_id.to_string()).or_default();
        while window
            .front()
            .is_some_and(|at| now_ms.saturating_sub(*at) >= SESSION_CREATION_WINDOW_MS)
        {
            window.pop_front();
        }
        if window.len() >= MAX_SESSION_CREATIONS_PER_MINUTE {
            return false;
        }
        window.push_back(now_ms);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anima_core::Content;
    use std::collections::BTreeMap;

    fn message(
        id: &str,
        role: MessageRole,
        text: &str,
        metadata: &[(&str, &str)],
        created_at_ms: u64,
    ) -> Message {
        Message {
            id: id.into(),
            agent_id: "agent-1".into(),
            room_id: "room-a".into(),
            content: Content {
                text: text.into(),
                attachments: None,
                metadata: (!metadata.is_empty()).then(|| {
                    metadata
                        .iter()
                        .map(|(key, value)| (key.to_string(), DataValue::String(value.to_string())))
                        .collect::<BTreeMap<_, _>>()
                }),
            },
            role,
            created_at_ms,
        }
    }

    fn chat_record(agent_id: &str, room_id: &str, now_ms: u64) -> SessionRecord {
        SessionRecord::new(
            agent_id,
            room_id,
            SessionKind::Chat,
            SessionOrigin::Web,
            DEFAULT_CHAT_TITLE.into(),
            TitleSource::FirstMessage,
            now_ms,
        )
    }

    fn empty_context() -> TitleContext<'static> {
        TitleContext {
            first_user_text: None,
            schedule_prompt: None,
            job_title: None,
            bot_username: None,
            peer_sender_name: None,
        }
    }

    #[test]
    fn session_ids_follow_the_pattern_and_invalid_rooms_map_to_stable_legacy_ids() {
        let longest = "a".repeat(MAX_SESSION_ID_BYTES);
        let too_long = "a".repeat(MAX_SESSION_ID_BYTES + 1);
        for valid in [
            "chat:5b1d",
            "direct:agent-1",
            "room-1700-3",
            "telegram:telegram-1",
            "schedule:s_1.2",
            longest.as_str(),
        ] {
            assert!(is_valid_session_id(valid), "{valid}");
            assert_eq!(session_id_for_room(valid), valid);
        }
        for invalid in ["", "has space", "slash/room", "ünïcode", too_long.as_str()] {
            assert!(!is_valid_session_id(invalid), "{invalid}");
            let mapped = session_id_for_room(invalid);
            assert!(mapped.starts_with(LEGACY_ROOM_SESSION_PREFIX), "{mapped}");
            assert_eq!(mapped.len(), LEGACY_ROOM_SESSION_PREFIX.len() + 32);
            assert!(is_valid_session_id(&mapped));
            assert_eq!(
                session_id_for_room(invalid),
                mapped,
                "the mapping is stable"
            );
        }
        assert_ne!(
            session_id_for_room("room one"),
            session_id_for_room("room two")
        );
        let chat = new_chat_session_id();
        let uuid = chat
            .strip_prefix("chat:")
            .expect("new chats use the chat prefix");
        assert_eq!(uuid::Uuid::parse_str(uuid).unwrap().get_version_num(), 4);
    }

    #[test]
    fn room_prefixes_name_their_owners() {
        assert_eq!(schedule_room_id("schedule-1"), "schedule:schedule-1");
        assert_eq!(
            schedule_id_of_room("schedule:schedule-1"),
            Some("schedule-1")
        );
        assert_eq!(
            connector_id_of_room("telegram:telegram-1"),
            Some("telegram-1")
        );
        assert_eq!(job_id_of_room("job:job-1"), Some("job-1"));
        assert_eq!(peer_sender_of_room("peer:alice:bob"), Some("alice"));
        assert_eq!(peer_sender_of_room("chat:x"), None);
        assert_eq!(
            crate::schedules::unwrap_checkin_prompt(&crate::schedules::wrap_checkin_prompt(
                "  Check goals "
            )),
            "Check goals"
        );
        assert_eq!(crate::schedules::unwrap_checkin_prompt("plain"), "plain");
    }

    #[test]
    fn rooms_map_to_kinds_and_origins() {
        let cases = [
            (
                "telegram:telegram-1",
                None,
                false,
                SessionKind::Telegram,
                SessionOrigin::Telegram,
            ),
            (
                "schedule:schedule-1",
                Some(RunSource::Schedule),
                false,
                SessionKind::Checkin,
                SessionOrigin::Schedule,
            ),
            (
                "job:job-1",
                Some(RunSource::Job),
                false,
                SessionKind::Job,
                SessionOrigin::Job,
            ),
            (
                "peer:alice:bob",
                Some(RunSource::Peer),
                false,
                SessionKind::Helper,
                SessionOrigin::Peer,
            ),
            (
                "room-1-1",
                Some(RunSource::Delegation),
                false,
                SessionKind::Helper,
                SessionOrigin::Delegation,
            ),
            (
                "room-1-2",
                Some(RunSource::Api),
                true,
                SessionKind::Helper,
                SessionOrigin::Delegation,
            ),
            (
                "chat:abc",
                Some(RunSource::Api),
                false,
                SessionKind::Chat,
                SessionOrigin::Web,
            ),
            (
                "direct:agent-1",
                None,
                false,
                SessionKind::Chat,
                SessionOrigin::Web,
            ),
            (
                "room-1-3",
                Some(RunSource::Api),
                false,
                SessionKind::Chat,
                SessionOrigin::Api,
            ),
            (
                "custom-room",
                None,
                false,
                SessionKind::Chat,
                SessionOrigin::Api,
            ),
        ];
        for (room, source, helper, kind, origin) in cases {
            assert_eq!(
                kind_for_room(room, source, helper),
                (kind, origin),
                "{room}"
            );
        }
    }

    #[test]
    fn capabilities_follow_the_kind_table() {
        let chat = SessionKind::Chat.capabilities(false);
        assert!(
            chat.send
                && chat.steer
                && chat.stop
                && chat.rename
                && chat.archive
                && chat.delete
                && chat.compact
                && chat.export
        );
        let telegram = SessionKind::Telegram.capabilities(false);
        assert!(telegram.send && !telegram.steer && telegram.rename && !telegram.delete);
        assert!(telegram.compact && telegram.export);
        assert!(
            !SessionKind::Checkin.capabilities(true).delete,
            "a check-in whose automation still exists stays"
        );
        assert!(SessionKind::Checkin.capabilities(false).delete);
        for kind in [SessionKind::Job, SessionKind::Helper] {
            let caps = kind.capabilities(false);
            assert!(
                !caps.send
                    && !caps.steer
                    && caps.stop
                    && !caps.rename
                    && caps.archive
                    && !caps.delete
                    && !caps.compact
                    && caps.export,
                "{kind:?}"
            );
        }
    }

    #[test]
    fn titles_are_short_single_line_plain_text() {
        assert_eq!(
            derived_title("\n  Plan my   week\nwith details").as_deref(),
            Some("Plan my week")
        );
        assert_eq!(derived_title("   \n\t"), None);
        let long = derived_title(&"word ".repeat(40)).unwrap();
        assert_eq!(long.chars().count(), DERIVED_TITLE_CHARS);
        assert!(long.ends_with('…'));
        assert_eq!(
            clean_owner_title("  Trip\nplanning  "),
            Ok("Trip planning".to_string())
        );
        assert_eq!(
            clean_owner_title(" \n "),
            Err("title must be 1 to 120 characters")
        );
        assert_eq!(
            clean_owner_title(&"x".repeat(MAX_SESSION_TITLE_CHARS + 1)),
            Err("title must be 1 to 120 characters")
        );
        assert_eq!(
            clean_owner_title(&"x".repeat(MAX_SESSION_TITLE_CHARS)).map(|title| title.len()),
            Ok(MAX_SESSION_TITLE_CHARS)
        );
        let preview = preview_text(&format!("{}\nend", "y".repeat(200))).unwrap();
        assert_eq!(preview.chars().count(), MAX_SESSION_PREVIEW_CHARS);
        assert_eq!(preview_text("  "), None);
    }

    #[test]
    fn titles_follow_the_session_kind() {
        let chat = TitleContext {
            first_user_text: Some("Hello there\nmore"),
            ..empty_context()
        };
        assert_eq!(
            session_title(SessionKind::Chat, SessionOrigin::Web, &chat),
            ("Hello there".to_string(), TitleSource::FirstMessage)
        );
        assert_eq!(
            session_title(SessionKind::Chat, SessionOrigin::Api, &empty_context()),
            (DEFAULT_CHAT_TITLE.to_string(), TitleSource::FirstMessage)
        );
        let telegram = TitleContext {
            bot_username: Some("anima_bot"),
            ..empty_context()
        };
        assert_eq!(
            session_title(SessionKind::Telegram, SessionOrigin::Telegram, &telegram),
            ("Telegram · @anima_bot".to_string(), TitleSource::System)
        );
        assert_eq!(
            session_title(
                SessionKind::Telegram,
                SessionOrigin::Telegram,
                &empty_context()
            )
            .0,
            "Telegram"
        );
        let wrapped = crate::schedules::wrap_checkin_prompt("Review open tasks");
        let checkin = TitleContext {
            first_user_text: Some(&wrapped),
            ..empty_context()
        };
        assert_eq!(
            session_title(SessionKind::Checkin, SessionOrigin::Schedule, &checkin).0,
            "Check-in · Review open tasks"
        );
        let scheduled = TitleContext {
            schedule_prompt: Some("Morning brief"),
            ..checkin
        };
        assert_eq!(
            session_title(SessionKind::Checkin, SessionOrigin::Schedule, &scheduled),
            ("Check-in · Morning brief".to_string(), TitleSource::System)
        );
        let job = TitleContext {
            job_title: Some("Prepare brief"),
            ..empty_context()
        };
        assert_eq!(
            session_title(SessionKind::Job, SessionOrigin::Job, &job),
            ("Job · Prepare brief".to_string(), TitleSource::System)
        );
        let peer = TitleContext {
            peer_sender_name: Some("Beta"),
            ..empty_context()
        };
        assert_eq!(
            session_title(SessionKind::Helper, SessionOrigin::Peer, &peer).0,
            "Messages from Beta"
        );
        let task = "Task delegated by workspace manager Anima (agent-7). Return the result and any blockers. Do not delegate further.\n\nCompare vendors";
        let delegated = TitleContext {
            first_user_text: Some(task),
            ..empty_context()
        };
        assert_eq!(
            session_title(SessionKind::Helper, SessionOrigin::Delegation, &delegated),
            ("Compare vendors".to_string(), TitleSource::System)
        );
        assert_eq!(delegating_agent_id(task), Some("agent-7"));
        assert_eq!(delegating_agent_id("Compare vendors"), None);
        assert_eq!(
            session_title(
                SessionKind::Helper,
                SessionOrigin::Delegation,
                &empty_context()
            )
            .0,
            "Helper task"
        );
    }

    #[test]
    fn silent_checkin_groups_are_hidden_and_spoken_ones_are_not() {
        let messages = vec![
            message("u1", MessageRole::User, "Hi", &[], 1),
            message("a1", MessageRole::Assistant, "Hello", &[], 2),
            message(
                "c1",
                MessageRole::User,
                "Check status",
                &[("kind", "checkin"), ("id", "s1")],
                3,
            ),
            message("t1", MessageRole::Assistant, "", &[], 4),
            message("r1", MessageRole::Tool, "done", &[], 5),
            message("s1", MessageRole::Assistant, "CHECKIN_OK", &[], 6),
            message(
                "c2",
                MessageRole::User,
                "Check status",
                &[("kind", "checkin"), ("id", "s1")],
                7,
            ),
            message(
                "s2",
                MessageRole::Assistant,
                "You have two overdue tasks",
                &[],
                8,
            ),
            message("u2", MessageRole::User, "Say CHECKIN_OK", &[], 9),
            message("a2", MessageRole::Assistant, "CHECKIN_OK", &[], 10),
        ];
        let mut hidden = hidden_message_ids(messages.iter())
            .into_iter()
            .collect::<Vec<_>>();
        hidden.sort();
        assert_eq!(hidden, ["c1", "r1", "s1", "t1"]);

        // A pruned transcript that starts mid-group hides only the bare sentinel.
        let tail = vec![
            message("t9", MessageRole::Tool, "x", &[], 1),
            message("s9", MessageRole::Assistant, " CHECKIN_OK ", &[], 2),
        ];
        assert_eq!(
            hidden_message_ids(tail.iter()),
            HashSet::from(["s9".to_string()])
        );
        assert!(is_checkin_message(&messages[2]));
        assert!(!is_checkin_message(&messages[0]));
        assert!(is_inbound_message(&message(
            "i",
            MessageRole::User,
            "hey",
            &[("source", "telegram")],
            1
        )));
        assert!(is_owner_web_turn(&message(
            "o",
            MessageRole::User,
            "hey",
            &[("source", "telegramThread")],
            1
        )));
    }

    #[test]
    fn commits_advance_activity_read_state_and_the_first_message_title_and_revert_exactly() {
        let mut registry = SessionRegistry::default();
        registry.insert(chat_record("agent-1", "chat:one", 10));
        let turn = vec![
            message("u1", MessageRole::User, "Plan the offsite\nsoon", &[], 20),
            message("a1", MessageRole::Assistant, "Sure", &[], 25),
        ];

        let undo = registry
            .record_commit("agent-1", "chat:one", &turn, true)
            .expect("the session exists");
        let record = registry.get("agent-1", "chat:one").unwrap();
        assert_eq!(record.last_activity_at_ms, 25);
        assert_eq!(
            record.last_read_at_ms,
            Some(20),
            "the owner's own message is read, the reply is not"
        );
        assert_eq!(record.title, "Plan the offsite");
        assert_eq!(record.title_source, TitleSource::FirstMessage);

        registry.revert_commit(undo);
        let record = registry.get("agent-1", "chat:one").unwrap();
        assert_eq!(record.last_activity_at_ms, 10);
        assert_eq!(record.last_read_at_ms, None);
        assert_eq!(record.title, DEFAULT_CHAT_TITLE);

        let undo = registry
            .record_commit("agent-1", "chat:one", &turn, false)
            .unwrap();
        assert_eq!(
            registry.get("agent-1", "chat:one").unwrap().last_read_at_ms,
            None,
            "another source's turn leaves the read state alone"
        );
        registry.revert_commit(undo);
        registry
            .get_mut("agent-1", "chat:one")
            .unwrap()
            .title_source = TitleSource::Owner;
        registry
            .record_commit("agent-1", "chat:one", &turn, false)
            .unwrap();
        assert_eq!(
            registry.get("agent-1", "chat:one").unwrap().title,
            DEFAULT_CHAT_TITLE,
            "owner titles are never replaced"
        );
        assert!(registry
            .record_commit("agent-1", "chat:missing", &turn, true)
            .is_none());
    }

    #[test]
    fn silent_checkin_commits_do_not_advance_activity_but_spoken_ones_do() {
        // Controller ruling (M2 pre-flight audit): lastActivityAtMs advances
        // only for visible messages, so a silent CHECKIN_OK exchange must not
        // move a heartbeat session to the top of the list.
        let mut registry = SessionRegistry::default();
        let mut record = SessionRecord::new(
            "agent-1",
            "schedule:one",
            SessionKind::Checkin,
            SessionOrigin::Schedule,
            "Check-in".into(),
            TitleSource::System,
            100,
        );
        record.last_activity_at_ms = 100;
        registry.insert(record);

        let silent_turn = vec![
            message(
                "c1",
                MessageRole::User,
                "Check status",
                &[("kind", "checkin"), ("id", "s1")],
                200,
            ),
            message("s1", MessageRole::Assistant, "CHECKIN_OK", &[], 205),
        ];
        registry
            .record_commit("agent-1", "schedule:one", &silent_turn, false)
            .expect("the session exists");
        assert_eq!(
            registry
                .get("agent-1", "schedule:one")
                .unwrap()
                .last_activity_at_ms,
            100,
            "a silent check-in must not move a heartbeat session to the top of the list"
        );

        let spoken_turn = vec![
            message(
                "c2",
                MessageRole::User,
                "Check status",
                &[("kind", "checkin"), ("id", "s1")],
                300,
            ),
            message(
                "s2",
                MessageRole::Assistant,
                "You have two overdue tasks",
                &[],
                305,
            ),
        ];
        registry
            .record_commit("agent-1", "schedule:one", &spoken_turn, false)
            .expect("the session exists");
        assert_eq!(
            registry
                .get("agent-1", "schedule:one")
                .unwrap()
                .last_activity_at_ms,
            305,
            "a spoken check-in still counts as activity"
        );
    }

    #[test]
    fn snapshots_keep_live_agents_sorted_and_validation_rejects_bad_records() {
        let mut registry = SessionRegistry::default();
        registry.insert(chat_record("agent-b", "chat:two", 1));
        registry.insert(chat_record("agent-a", "chat:one", 1));
        registry.insert(chat_record("agent-gone", "chat:three", 1));
        let live = HashSet::from(["agent-a".to_string(), "agent-b".to_string()]);

        let saved = registry.snapshot_records(&live);
        assert_eq!(
            saved
                .iter()
                .map(|record| (record.agent_id.as_str(), record.id.as_str()))
                .collect::<Vec<_>>(),
            [("agent-a", "chat:one"), ("agent-b", "chat:two")]
        );
        assert!(SessionRegistry::validate(&saved).is_ok());
        let everyone = HashSet::from([
            "agent-a".to_string(),
            "agent-b".to_string(),
            "agent-gone".to_string(),
        ]);
        let restored = SessionRegistry::restored(registry.snapshot_records(&everyone), &live);
        assert_eq!(
            restored.len(),
            2,
            "sessions of missing agents are dropped on restore"
        );

        let mut duplicate = saved.clone();
        duplicate.push(saved[0].clone());
        assert!(SessionRegistry::validate(&duplicate).is_err());
        let mut bad_id = saved[0].clone();
        bad_id.id = "not valid".into();
        assert!(SessionRegistry::validate(&[bad_id]).is_err());
        let mut blank_title = saved[0].clone();
        blank_title.title = String::new();
        assert!(SessionRegistry::validate(&[blank_title]).is_err());
        let mut wrong_room = saved[0].clone();
        wrong_room.room_id = Some("other room".into());
        assert!(SessionRegistry::validate(&[wrong_room]).is_err());

        let legacy = chat_record("agent-a", "legacy room/1", 1);
        assert_eq!(legacy.room_id(), "legacy room/1");
        assert!(legacy.id.starts_with(LEGACY_ROOM_SESSION_PREFIX));
        assert!(SessionRegistry::validate(&[legacy]).is_ok());
    }

    #[test]
    fn records_use_the_documented_json_names() {
        let mut record = chat_record("agent-1", "legacy room/1", 5);
        record.parent_agent_id = Some("agent-0".into());
        let value = serde_json::to_value(&record).unwrap();
        assert_eq!(value["kind"], "chat");
        assert_eq!(value["origin"], "web");
        assert_eq!(value["titleSource"], "first_message");
        assert_eq!(value["createdAtMs"], 5);
        assert_eq!(value["lastActivityAtMs"], 5);
        assert_eq!(value["lastReadAtMs"], serde_json::Value::Null);
        assert_eq!(value["parentAgentId"], "agent-0");
        assert_eq!(value["roomId"], "legacy room/1");
        assert_eq!(
            serde_json::to_value(chat_record("agent-1", "chat:x", 1))
                .unwrap()
                .get("roomId"),
            None,
            "valid rooms carry no roomId"
        );
        let minimal: SessionRecord = serde_json::from_value(serde_json::json!({
            "id": "chat:x",
            "agentId": "agent-1",
            "kind": "helper",
            "origin": "peer",
            "title": "T",
            "titleSource": "system",
            "createdAtMs": 1,
            "lastActivityAtMs": 2
        }))
        .unwrap();
        assert!(!minimal.archived);
        assert_eq!(minimal.summary, None);
        assert_eq!(minimal.last_read_at_ms, None);
        assert_eq!(SessionKind::parse("checkin"), Some(SessionKind::Checkin));
        assert_eq!(SessionKind::parse("chats"), None);
        assert_eq!(SessionOrigin::Delegation.as_str(), "delegation");
        assert_eq!(TitleSource::Generated.as_str(), "generated");
    }

    #[test]
    fn session_creation_is_limited_per_agent_per_minute() {
        let mut limiter = SessionCreateLimiter::default();
        for _ in 0..MAX_SESSION_CREATIONS_PER_MINUTE {
            assert!(limiter.try_acquire("agent-1", 1_000));
        }
        assert!(!limiter.try_acquire("agent-1", 1_000 + 59_999));
        assert!(
            limiter.try_acquire("agent-2", 1_000),
            "limits are per agent"
        );
        assert!(
            limiter.try_acquire("agent-1", 1_000 + 60_000),
            "the one-minute window slides"
        );
    }
}
