//! Upgrading older control planes to sessions (spec §13.3): relabel legacy
//! per-tick check-in rooms, give every room a session record, map ledger
//! session ids, and grant tools added after agents already existed.

use std::collections::HashMap;

use anima_core::{AgentConfig, AgentRuntimeSnapshot, DataValue, Message, MessageRole};

use super::{
    connector_id_of_room, delegating_agent_id, hidden_message_ids, is_checkin_message,
    job_id_of_room, kind_for_room, peer_sender_of_room, schedule_id_of_room, schedule_room_id,
    session_id_for_room, session_title, SessionKind, SessionRecord, SessionRegistry, TitleContext,
};
use crate::connectors::TelegramConnectorRecord;
use crate::jobs::AgentJobRecord;
use crate::runs::{RunRecord, RunSource};
use crate::schedules::ScheduledPromptRecord;

/// Tools added after agents existed, granted once to the agents of that time
/// the way the web access profiles grant them (spec §13.3 step 5).
#[derive(Clone, Copy, Debug)]
pub(crate) struct ToolGrantSet {
    /// Recorded in the control plane once applied.
    pub(crate) id: &'static str,
    /// Granted to every non-helper agent that has a tool list.
    pub(crate) read_class: &'static [&'static str],
    /// Granted to those agents that already have `write_file`.
    pub(crate) write_class: &'static [&'static str],
}

/// M2 adds no tools. M3 (`search_conversations`), M5 (`load_skill`,
/// `propose_skill`), and M6 (`list_automations`, `create_automation`,
/// `pause_automation`) append their grant sets here.
pub(crate) const TOOL_GRANTS: &[ToolGrantSet] = &[];

/// Legacy check-ins ran in a fresh `room-*` room per tick. Those rooms become
/// the automation's `schedule:<id>` session, and so do their ledger runs;
/// nothing else references them. A run a restart interrupted or a rollback
/// undid may have committed no message to key off, so a schedule-sourced run
/// whose session id still looks like a per-tick room is also relabelled from
/// its own `sourceRef`, whether or not its room holds any message. Callers
/// only run this against a snapshot older than the current store version — a
/// room a live run just created this boot must be left alone, or its
/// already-mirrored history would be stranded under the old id. Returns how
/// many messages moved and how many runs were relabelled.
pub(crate) fn relabel_legacy_checkin_rooms(
    agents: &mut [AgentRuntimeSnapshot],
    runs: &mut [RunRecord],
) -> (usize, usize) {
    let mut moved = 0;
    let mut relabelled: HashMap<(String, String), String> = HashMap::new();
    for agent in agents.iter_mut() {
        let mut targets: HashMap<String, String> = HashMap::new();
        for message in &agent.messages {
            if !message.room_id.starts_with("room-") || targets.contains_key(&message.room_id) {
                continue;
            }
            if let Some(schedule_id) = checkin_schedule_id(message) {
                targets.insert(message.room_id.clone(), schedule_room_id(schedule_id));
            }
        }
        if targets.is_empty() {
            continue;
        }
        for message in agent.messages.iter_mut() {
            if let Some(room) = targets.get(&message.room_id) {
                message.room_id = room.clone();
                moved += 1;
            }
        }
        for (old, new) in targets {
            relabelled.insert((agent.state.id.clone(), old), new);
        }
    }
    let mut runs_relabelled = 0;
    for run in runs.iter_mut() {
        if let Some(room) = relabelled.get(&(run.agent_id.clone(), run.session_id.clone())) {
            run.session_id = room.clone();
            runs_relabelled += 1;
        } else if run.source == RunSource::Schedule && run.session_id.starts_with("room-") {
            let source_ref = run
                .source_ref
                .as_deref()
                .filter(|schedule_id| !schedule_id.trim().is_empty());
            if let Some(schedule_id) = source_ref {
                run.session_id = schedule_room_id(schedule_id);
                runs_relabelled += 1;
            }
        }
    }
    (moved, runs_relabelled)
}

fn checkin_schedule_id(message: &Message) -> Option<&str> {
    if !is_checkin_message(message) {
        return None;
    }
    match message.content.metadata.as_ref()?.get("id") {
        Some(DataValue::String(id)) if !id.trim().is_empty() => Some(id),
        _ => None,
    }
}

/// Ledger session ids that were written as raw room ids take the room's
/// session id (M1 carry-forward F17). Returns how many changed.
pub(crate) fn map_ledger_session_ids(runs: &mut [RunRecord]) -> usize {
    let mut mapped = 0;
    for run in runs {
        let session_id = session_id_for_room(&run.session_id);
        if session_id != run.session_id {
            run.session_id = session_id;
            mapped += 1;
        }
    }
    mapped
}

/// One agent's transcript, as the migration reads it.
pub(crate) struct LegacyAgent<'a> {
    pub(crate) agent_id: &'a str,
    pub(crate) config: &'a AgentConfig,
    pub(crate) messages: &'a [Message],
}

/// Records a session title can come from.
pub(crate) struct LegacySessionContext<'a> {
    pub(crate) schedules: &'a HashMap<String, ScheduledPromptRecord>,
    pub(crate) jobs: &'a HashMap<String, AgentJobRecord>,
    pub(crate) connectors: &'a HashMap<String, TelegramConnectorRecord>,
    pub(crate) agent_names: &'a HashMap<String, String>,
}

/// Session records for the rooms that have none, by the rules of spec §3.1
/// (spec §13.3 step 2). Upgraded history counts as read.
pub(crate) fn derive_sessions_for_legacy_rooms(
    registry: &SessionRegistry,
    agents: &[LegacyAgent<'_>],
    context: &LegacySessionContext<'_>,
) -> Vec<SessionRecord> {
    let mut derived = Vec::new();
    for agent in agents {
        let helper_parent = crate::agent_runs::config_helper_parent(agent.config);
        let mut rooms: Vec<(&str, Vec<&Message>)> = Vec::new();
        let mut index: HashMap<&str, usize> = HashMap::new();
        for message in agent.messages {
            // An empty or whitespace-only room id would derive an invalid
            // session and refuse the next boot's validation; skip it.
            if message.room_id.trim().is_empty() {
                continue;
            }
            let slot = *index.entry(message.room_id.as_str()).or_insert_with(|| {
                rooms.push((message.room_id.as_str(), Vec::new()));
                rooms.len() - 1
            });
            rooms[slot].1.push(message);
        }
        for (room_id, messages) in rooms {
            if registry.contains(agent.agent_id, &session_id_for_room(room_id)) {
                continue;
            }
            derived.push(legacy_session(
                agent.agent_id,
                helper_parent,
                room_id,
                &messages,
                context,
            ));
        }
    }
    derived
}

fn legacy_session(
    agent_id: &str,
    helper_parent: Option<&str>,
    room_id: &str,
    messages: &[&Message],
    context: &LegacySessionContext<'_>,
) -> SessionRecord {
    let first_user = messages
        .iter()
        .find(|message| message.role == MessageRole::User)
        .map(|message| message.content.text.as_str());
    let delegated_by = first_user.and_then(delegating_agent_id);
    let source = delegated_by.map(|_| RunSource::Delegation);
    let (kind, origin) = kind_for_room(room_id, source, helper_parent.is_some());
    let peer_sender = peer_sender_of_room(room_id);
    let title_context = TitleContext {
        first_user_text: first_user,
        schedule_prompt: schedule_id_of_room(room_id)
            .and_then(|id| context.schedules.get(id))
            .map(|schedule| schedule.prompt.as_str()),
        job_title: job_id_of_room(room_id)
            .and_then(|id| context.jobs.get(id))
            .map(|job| job.title.as_str()),
        bot_username: connector_id_of_room(room_id)
            .and_then(|id| context.connectors.get(id))
            .and_then(|connector| connector.bot.username.as_deref()),
        peer_sender_name: peer_sender
            .and_then(|id| context.agent_names.get(id))
            .map(String::as_str),
    };
    let (title, title_source) = session_title(kind, origin, &title_context);
    let first_at = messages
        .first()
        .map(|message| message.created_at_ms)
        .unwrap_or(0);
    // last_read_at_ms covers every message, visible or not, so upgraded
    // history is never unread. last_activity_at_ms follows the live rule and
    // counts only visible messages, so a room upgraded straight from a
    // silent check-in pair does not jump to the top of the sidebar.
    let last_read_at = messages
        .iter()
        .map(|message| message.created_at_ms)
        .max()
        .unwrap_or(first_at);
    let hidden = hidden_message_ids(messages.iter().copied());
    let last_activity_at = messages
        .iter()
        .filter(|message| !hidden.contains(&message.id))
        .map(|message| message.created_at_ms)
        .max()
        .unwrap_or(first_at);
    let mut record = SessionRecord::new(
        agent_id,
        room_id,
        kind,
        origin,
        title,
        title_source,
        first_at,
    );
    record.last_activity_at_ms = last_activity_at;
    record.last_read_at_ms = Some(last_read_at);
    if kind == SessionKind::Helper {
        record.parent_agent_id = helper_parent
            .or(delegated_by)
            .or(peer_sender)
            .map(str::to_string);
    }
    record
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::{RunStart, RunStatus};
    use crate::sessions::{SessionOrigin, TitleSource};
    use crate::state::DaemonState;
    use anima_core::{AgentSettings, Content};
    use std::collections::{BTreeMap, HashSet};

    fn message(
        id: &str,
        room: &str,
        role: MessageRole,
        text: &str,
        metadata: &[(&str, &str)],
        at: u64,
    ) -> Message {
        Message {
            id: id.into(),
            agent_id: "agent".into(),
            room_id: room.into(),
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
            created_at_ms: at,
        }
    }

    fn run_in(agent_id: &str, session_id: &str) -> RunRecord {
        let mut run = RunRecord::running(
            RunStart {
                agent_id: agent_id.into(),
                session_id: session_id.into(),
                source: RunSource::Schedule,
                source_ref: None,
                idempotency_key: None,
                text: "tick".into(),
                model: "deterministic".into(),
                provider: None,
                parent_run_id: None,
            },
            10,
        );
        run.finish(RunStatus::Completed, None, 11);
        run
    }

    fn config(name: &str, additional: &[(&str, &str)]) -> AgentConfig {
        AgentConfig {
            name: name.into(),
            model: "deterministic".into(),
            bio: None,
            lore: None,
            knowledge: None,
            topics: None,
            adjectives: None,
            style: None,
            provider: None,
            system: None,
            tools: None,
            plugins: None,
            settings: Some(AgentSettings {
                additional: additional
                    .iter()
                    .map(|(key, value)| (key.to_string(), DataValue::String(value.to_string())))
                    .collect(),
                ..AgentSettings::default()
            }),
        }
    }

    fn agent_snapshot(name: &str, messages: Vec<Message>) -> AgentRuntimeSnapshot {
        let mut snapshot = anima_core::AgentRuntime::new(
            config(name, &[]),
            std::sync::Arc::new(crate::model::DeterministicModelAdapter),
        )
        .snapshot();
        let agent_id = snapshot.state.id.clone();
        snapshot.messages = messages
            .into_iter()
            .map(|mut message| {
                message.agent_id = agent_id.clone();
                message
            })
            .collect();
        snapshot.message_count = snapshot.messages.len();
        snapshot
    }

    #[test]
    fn relabels_only_legacy_checkin_rooms_and_their_ledger_runs() {
        let wrapped = crate::schedules::wrap_checkin_prompt("Review open tasks");
        let tagged = [("kind", "checkin"), ("id", "schedule-1")];
        let mut messages = Vec::new();
        for (room, id, at) in [("room-100-1", "a", 100), ("room-200-2", "b", 200)] {
            messages.push(message(
                &format!("{id}-prompt"),
                room,
                MessageRole::User,
                &wrapped,
                &tagged,
                at,
            ));
            messages.push(message(
                &format!("{id}-reply"),
                room,
                MessageRole::Assistant,
                "CHECKIN_OK",
                &[],
                at + 1,
            ));
        }
        messages.push(message(
            "chat-user",
            "room-300-3",
            MessageRole::User,
            "Summarize",
            &[],
            300,
        ));
        messages.push(message(
            "telegram-checkin",
            "telegram:t1",
            MessageRole::User,
            &wrapped,
            &[("kind", "checkin"), ("id", "schedule-2")],
            400,
        ));
        let mut agents = vec![agent_snapshot("companion", messages)];
        let agent_id = agents[0].state.id.clone();
        let mut runs = vec![
            run_in(&agent_id, "room-100-1"),
            run_in(&agent_id, "room-300-3"),
        ];

        let (moved, runs_relabelled) = relabel_legacy_checkin_rooms(&mut agents, &mut runs);

        assert_eq!(moved, 4);
        assert_eq!(
            runs_relabelled, 1,
            "only the room-100-1 run had a message-based relabel; room-300-3 was never a check-in"
        );
        let rooms = agents[0]
            .messages
            .iter()
            .map(|message| (message.id.as_str(), message.room_id.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            rooms,
            [
                ("a-prompt", "schedule:schedule-1"),
                ("a-reply", "schedule:schedule-1"),
                ("b-prompt", "schedule:schedule-1"),
                ("b-reply", "schedule:schedule-1"),
                ("chat-user", "room-300-3"),
                ("telegram-checkin", "telegram:t1"),
            ]
        );
        assert_eq!(runs[0].session_id, "schedule:schedule-1");
        assert_eq!(runs[1].session_id, "room-300-3");
        assert_eq!(
            relabel_legacy_checkin_rooms(&mut agents, &mut runs),
            (0, 0),
            "relabeling is idempotent"
        );
    }

    #[test]
    fn a_mixed_checkin_and_chat_room_moves_whole() {
        let wrapped = crate::schedules::wrap_checkin_prompt("Review open tasks");
        let messages = vec![
            message(
                "m1",
                "room-1-1",
                MessageRole::User,
                "Unrelated chat",
                &[],
                10,
            ),
            message("m2", "room-1-1", MessageRole::Assistant, "Sure", &[], 11),
            message(
                "c1",
                "room-1-1",
                MessageRole::User,
                &wrapped,
                &[("kind", "checkin"), ("id", "schedule-1")],
                20,
            ),
            message(
                "c2",
                "room-1-1",
                MessageRole::Assistant,
                "CHECKIN_OK",
                &[],
                21,
            ),
        ];
        let mut agents = vec![agent_snapshot("companion", messages)];
        let mut runs: Vec<RunRecord> = Vec::new();

        let (moved, _) = relabel_legacy_checkin_rooms(&mut agents, &mut runs);

        assert_eq!(
            moved, 4,
            "every message in the room moves, not only the check-in pair"
        );
        assert!(agents[0]
            .messages
            .iter()
            .all(|message| message.room_id == "schedule:schedule-1"));
    }

    #[test]
    fn ticks_of_different_schedules_go_to_their_own_rooms() {
        let wrapped = crate::schedules::wrap_checkin_prompt("Review open tasks");
        let messages = vec![
            message(
                "a1",
                "room-1-1",
                MessageRole::User,
                &wrapped,
                &[("kind", "checkin"), ("id", "schedule-1")],
                10,
            ),
            message(
                "a2",
                "room-1-1",
                MessageRole::Assistant,
                "CHECKIN_OK",
                &[],
                11,
            ),
            message(
                "b1",
                "room-2-1",
                MessageRole::User,
                &wrapped,
                &[("kind", "checkin"), ("id", "schedule-2")],
                20,
            ),
            message(
                "b2",
                "room-2-1",
                MessageRole::Assistant,
                "CHECKIN_OK",
                &[],
                21,
            ),
        ];
        let mut agents = vec![agent_snapshot("companion", messages)];
        let mut runs: Vec<RunRecord> = Vec::new();

        let (moved, _) = relabel_legacy_checkin_rooms(&mut agents, &mut runs);

        assert_eq!(moved, 4);
        let rooms = agents[0]
            .messages
            .iter()
            .map(|message| message.room_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            rooms,
            [
                "schedule:schedule-1",
                "schedule:schedule-1",
                "schedule:schedule-2",
                "schedule:schedule-2",
            ],
            "each tick's room follows its own schedule id, never another one's"
        );
    }

    #[test]
    fn another_agents_run_with_the_same_room_id_stays_untouched() {
        let wrapped = crate::schedules::wrap_checkin_prompt("Review open tasks");
        let messages = vec![
            message(
                "c1",
                "room-1-1",
                MessageRole::User,
                &wrapped,
                &[("kind", "checkin"), ("id", "schedule-1")],
                10,
            ),
            message(
                "c2",
                "room-1-1",
                MessageRole::Assistant,
                "CHECKIN_OK",
                &[],
                11,
            ),
        ];
        let mut agents = vec![agent_snapshot("companion", messages)];
        let mut runs = vec![run_in("some-other-agent", "room-1-1")];

        relabel_legacy_checkin_rooms(&mut agents, &mut runs);

        assert_eq!(
            runs[0].session_id, "room-1-1",
            "a different agent's run with the same room string is untouched"
        );
    }

    #[test]
    fn an_interrupted_schedule_run_with_no_messages_still_relabels_from_its_source_ref() {
        let mut agents = vec![agent_snapshot("companion", Vec::new())];
        let agent_id = agents[0].state.id.clone();
        let mut run = RunRecord::running(
            RunStart {
                agent_id: agent_id.clone(),
                session_id: "room-9-1".into(),
                source: RunSource::Schedule,
                source_ref: Some("schedule-9".into()),
                idempotency_key: None,
                text: "tick".into(),
                model: "deterministic".into(),
                provider: None,
                parent_run_id: None,
            },
            10,
        );
        run.finish(RunStatus::Interrupted, None, 11);
        let mut runs = vec![run];

        let (moved, runs_relabelled) = relabel_legacy_checkin_rooms(&mut agents, &mut runs);

        assert_eq!(moved, 0, "the room never got any messages");
        assert_eq!(runs_relabelled, 1);
        assert_eq!(runs[0].session_id, "schedule:schedule-9");
    }

    #[test]
    fn relabel_only_runs_on_a_snapshot_older_than_the_current_version() {
        let mut source = DaemonState::new();
        let agent_id = source
            .create_agent(config("companion", &[]))
            .unwrap()
            .state
            .id;
        let wrapped = crate::schedules::wrap_checkin_prompt("Review open tasks");
        let checkin_messages = || -> Vec<Message> {
            vec![
                message(
                    "c1",
                    "room-1-1",
                    MessageRole::User,
                    &wrapped,
                    &[("kind", "checkin"), ("id", "schedule-1")],
                    10,
                ),
                message(
                    "c2",
                    "room-1-1",
                    MessageRole::Assistant,
                    "CHECKIN_OK",
                    &[],
                    11,
                ),
            ]
            .into_iter()
            .map(|mut message| {
                message.agent_id = agent_id.clone();
                message
            })
            .collect()
        };

        // A run just created this boot's tagged room-* room; a current-version
        // snapshot must leave it alone (its history may already be mirrored
        // under the old id).
        let mut current = source.control_plane_snapshot();
        current.agents[0].messages = checkin_messages();
        current.agents[0].message_count = 2;
        let mut restored_current = DaemonState::new();
        restored_current
            .restore_control_plane_snapshot(current)
            .unwrap();
        assert!(restored_current
            .get_agent(&agent_id)
            .unwrap()
            .messages
            .iter()
            .all(|message| message.room_id == "room-1-1"));

        // The same data under an older version still relabels.
        let mut older = source.control_plane_snapshot();
        older.version = 4;
        older.agents[0].messages = checkin_messages();
        older.agents[0].message_count = 2;
        let mut restored_older = DaemonState::new();
        restored_older
            .restore_control_plane_snapshot(older)
            .unwrap();
        assert!(restored_older
            .get_agent(&agent_id)
            .unwrap()
            .messages
            .iter()
            .all(|message| message.room_id == "schedule:schedule-1"));
    }

    #[test]
    fn ledger_session_ids_follow_the_room_mapping() {
        let mut runs = vec![
            run_in("agent", "weird room/1"),
            run_in("agent", "chat:fine"),
        ];
        assert_eq!(map_ledger_session_ids(&mut runs), 1);
        assert_eq!(runs[0].session_id, session_id_for_room("weird room/1"));
        assert_eq!(runs[1].session_id, "chat:fine");
        assert_eq!(map_ledger_session_ids(&mut runs), 0);
    }

    #[test]
    fn derives_a_session_for_every_legacy_room_kind() {
        let companion_config = config("Anima", &[("workspaceRole", "lead")]);
        let specialist_config = config("Specialist", &[]);
        let helper_config = config(
            "Research helper",
            &[
                ("workspaceRole", "helper"),
                ("parentAgentId", "companion-1"),
            ],
        );
        let wrapped = crate::schedules::wrap_checkin_prompt("Review open tasks");
        let companion_messages = vec![
            message(
                "d1",
                "direct:companion-1",
                MessageRole::User,
                "Plan my week\nwith details",
                &[],
                10,
            ),
            message(
                "d2",
                "direct:companion-1",
                MessageRole::Assistant,
                "Sure",
                &[],
                11,
            ),
            message(
                "g1",
                "room-20-1",
                MessageRole::User,
                "Summarize the report",
                &[],
                20,
            ),
            message(
                "c1",
                "schedule:schedule-1",
                MessageRole::User,
                &wrapped,
                &[("kind", "checkin"), ("id", "schedule-1")],
                30,
            ),
            message(
                "t1",
                "telegram:telegram-a",
                MessageRole::User,
                "hello from the phone",
                &[("source", "telegram")],
                40,
            ),
            message(
                "j1",
                "job:job-1",
                MessageRole::User,
                "Prepare the brief",
                &[],
                50,
            ),
            message("w1", "weird room/1", MessageRole::User, "Odd room", &[], 60),
            message(
                "w2",
                "weird room/1",
                MessageRole::Assistant,
                "Reply",
                &[],
                65,
            ),
        ];
        let delegated = "Task delegated by workspace manager Anima (companion-1). Return the result and any blockers. Do not delegate further.\n\nCompare vendors";
        let specialist_messages = vec![
            message("s1", "room-70-2", MessageRole::User, delegated, &[], 70),
            message(
                "p1",
                "peer:companion-1:specialist-1",
                MessageRole::User,
                "Can you check this?",
                &[],
                80,
            ),
        ];
        let helper_messages = vec![message(
            "h1",
            "room-90-3",
            MessageRole::User,
            "Find sources",
            &[],
            90,
        )];
        let schedules = HashMap::from([(
            "schedule-1".to_string(),
            serde_json::from_value::<ScheduledPromptRecord>(serde_json::json!({
                "id": "schedule-1",
                "agentId": "companion-1",
                "prompt": "Morning brief",
                "trigger": {"interval": {"intervalMs": 60000}},
                "target": "workspace",
                "nextDueAtMs": 1,
                "createdAtMs": 1,
                "updatedAtMs": 1
            }))
            .unwrap(),
        )]);
        let jobs = HashMap::from([(
            "job-1".to_string(),
            serde_json::from_value::<AgentJobRecord>(serde_json::json!({
                "id": "job-1",
                "agentId": "companion-1",
                "title": "Prepare brief",
                "prompt": "Prepare the brief",
                "requestKey": "brief",
                "status": "completed",
                "revision": 1,
                "attempt": 1,
                "createdAtMs": 1,
                "updatedAtMs": 2
            }))
            .unwrap(),
        )]);
        let connectors = HashMap::from([(
            "telegram-a".to_string(),
            serde_json::from_value::<TelegramConnectorRecord>(serde_json::json!({
                "id": "telegram-a",
                "agentId": "companion-1",
                "roomId": "telegram:telegram-a",
                "bot": {"id": "1", "username": "anima_bot"},
                "createdAtMs": 1,
                "updatedAtMs": 1
            }))
            .unwrap(),
        )]);
        let agent_names = HashMap::from([
            ("companion-1".to_string(), "Anima".to_string()),
            ("specialist-1".to_string(), "Specialist".to_string()),
            ("helper-1".to_string(), "Research helper".to_string()),
        ]);
        let mut registry = SessionRegistry::default();
        registry.insert(SessionRecord::new(
            "companion-1",
            "room-20-1",
            SessionKind::Chat,
            SessionOrigin::Api,
            "Existing".into(),
            TitleSource::Owner,
            1,
        ));
        let agents = [
            LegacyAgent {
                agent_id: "companion-1",
                config: &companion_config,
                messages: &companion_messages,
            },
            LegacyAgent {
                agent_id: "specialist-1",
                config: &specialist_config,
                messages: &specialist_messages,
            },
            LegacyAgent {
                agent_id: "helper-1",
                config: &helper_config,
                messages: &helper_messages,
            },
        ];
        let context = LegacySessionContext {
            schedules: &schedules,
            jobs: &jobs,
            connectors: &connectors,
            agent_names: &agent_names,
        };

        let derived = derive_sessions_for_legacy_rooms(&registry, &agents, &context);

        let by_key = derived
            .iter()
            .map(|record| ((record.agent_id.as_str(), record.id.as_str()), record))
            .collect::<HashMap<_, _>>();
        assert_eq!(derived.len(), 8, "every room except the registered one");
        let direct = by_key[&("companion-1", "direct:companion-1")];
        assert_eq!(
            (direct.kind, direct.origin),
            (SessionKind::Chat, SessionOrigin::Web)
        );
        assert_eq!(
            (direct.title.as_str(), direct.title_source),
            ("Plan my week", TitleSource::FirstMessage)
        );
        assert_eq!((direct.created_at_ms, direct.last_activity_at_ms), (10, 11));
        assert_eq!(
            direct.last_read_at_ms,
            Some(11),
            "upgraded history is not unread"
        );
        assert!(!by_key.contains_key(&("companion-1", "room-20-1")));
        let checkin = by_key[&("companion-1", "schedule:schedule-1")];
        assert_eq!(
            (checkin.kind, checkin.title.as_str()),
            (SessionKind::Checkin, "Check-in · Morning brief")
        );
        assert_eq!(
            by_key[&("companion-1", "telegram:telegram-a")].title,
            "Telegram · @anima_bot"
        );
        assert_eq!(
            by_key[&("companion-1", "job:job-1")].title,
            "Job · Prepare brief"
        );
        let legacy_id = session_id_for_room("weird room/1");
        let legacy = by_key[&("companion-1", legacy_id.as_str())];
        assert_eq!(legacy.room_id(), "weird room/1");
        assert_eq!(
            (legacy.kind, legacy.origin, legacy.title.as_str()),
            (SessionKind::Chat, SessionOrigin::Api, "Odd room")
        );
        assert_eq!(legacy.last_activity_at_ms, 65);
        let delegated_session = by_key[&("specialist-1", "room-70-2")];
        assert_eq!(
            (delegated_session.kind, delegated_session.origin),
            (SessionKind::Helper, SessionOrigin::Delegation)
        );
        assert_eq!(delegated_session.title, "Compare vendors");
        assert_eq!(
            delegated_session.parent_agent_id.as_deref(),
            Some("companion-1")
        );
        let peer = by_key[&("specialist-1", "peer:companion-1:specialist-1")];
        assert_eq!(
            (peer.kind, peer.origin, peer.title.as_str()),
            (
                SessionKind::Helper,
                SessionOrigin::Peer,
                "Messages from Anima"
            )
        );
        assert_eq!(peer.parent_agent_id.as_deref(), Some("companion-1"));
        let helper = by_key[&("helper-1", "room-90-3")];
        assert_eq!(
            (helper.kind, helper.title.as_str()),
            (SessionKind::Helper, "Find sources")
        );
        assert_eq!(helper.parent_agent_id.as_deref(), Some("companion-1"));
    }

    #[test]
    fn a_derived_checkin_session_with_a_trailing_silent_pair_keeps_the_visible_activity_time() {
        let wrapped = crate::schedules::wrap_checkin_prompt("Review open tasks");
        let messages = vec![
            message(
                "c1",
                "schedule:schedule-1",
                MessageRole::User,
                &wrapped,
                &[("kind", "checkin"), ("id", "schedule-1")],
                10,
            ),
            message(
                "s1",
                "schedule:schedule-1",
                MessageRole::Assistant,
                "You have two overdue tasks",
                &[],
                11,
            ),
            message(
                "c2",
                "schedule:schedule-1",
                MessageRole::User,
                &wrapped,
                &[("kind", "checkin"), ("id", "schedule-1")],
                20,
            ),
            message(
                "s2",
                "schedule:schedule-1",
                MessageRole::Assistant,
                "CHECKIN_OK",
                &[],
                21,
            ),
        ];
        let agent_config = config("companion", &[]);
        let agents = [LegacyAgent {
            agent_id: "companion-1",
            config: &agent_config,
            messages: &messages,
        }];
        let context = LegacySessionContext {
            schedules: &HashMap::new(),
            jobs: &HashMap::new(),
            connectors: &HashMap::new(),
            agent_names: &HashMap::new(),
        };

        let derived =
            derive_sessions_for_legacy_rooms(&SessionRegistry::default(), &agents, &context);

        assert_eq!(derived.len(), 1);
        let session = &derived[0];
        assert_eq!(
            session.last_activity_at_ms, 11,
            "the trailing silent check-in pair at 20/21 is hidden from activity, same as the live rule"
        );
        assert_eq!(
            session.last_read_at_ms,
            Some(21),
            "read state still covers every message, visible or not"
        );
    }

    #[test]
    fn messages_with_a_blank_room_id_are_skipped_when_deriving_sessions() {
        let agent_config = config("companion", &[]);
        let messages = vec![
            message("m1", "", MessageRole::User, "stray", &[], 10),
            message("m2", "   ", MessageRole::User, "also stray", &[], 20),
            message("m3", "chat:x", MessageRole::User, "real chat", &[], 30),
        ];
        let agents = [LegacyAgent {
            agent_id: "companion-1",
            config: &agent_config,
            messages: &messages,
        }];
        let context = LegacySessionContext {
            schedules: &HashMap::new(),
            jobs: &HashMap::new(),
            connectors: &HashMap::new(),
            agent_names: &HashMap::new(),
        };

        let derived =
            derive_sessions_for_legacy_rooms(&SessionRegistry::default(), &agents, &context);

        assert_eq!(
            derived.len(),
            1,
            "a blank or whitespace-only room id never derives a session"
        );
        assert_eq!(derived[0].id, "chat:x");
    }

    #[test]
    fn restoring_an_older_snapshot_gives_every_room_a_session_exactly_once() {
        let mut source = DaemonState::new();
        let agent_id = source
            .create_agent(config("companion", &[("workspaceRole", "lead")]))
            .unwrap()
            .state
            .id;
        let wrapped = crate::schedules::wrap_checkin_prompt("Review open tasks");
        let mut snapshot = source.control_plane_snapshot();
        snapshot.version = 4;
        snapshot.agents[0].messages = vec![
            message(
                "d1",
                &format!("direct:{agent_id}"),
                MessageRole::User,
                "Plan my week",
                &[],
                10,
            ),
            message(
                "c1",
                "room-20-1",
                MessageRole::User,
                &wrapped,
                &[("kind", "checkin"), ("id", "schedule-9")],
                20,
            ),
            message(
                "c2",
                "room-20-1",
                MessageRole::Assistant,
                "CHECKIN_OK",
                &[],
                21,
            ),
        ]
        .into_iter()
        .map(|mut message| {
            message.agent_id = agent_id.clone();
            message
        })
        .collect();
        snapshot.agents[0].message_count = 3;
        snapshot.runs = vec![run_in(&agent_id, "room-20-1")];

        let mut restored = DaemonState::new();
        restored
            .restore_control_plane_snapshot(snapshot)
            .expect("an older snapshot restores");

        let direct = restored
            .sessions
            .get(&agent_id, &format!("direct:{agent_id}"))
            .expect("the legacy web chat is a session");
        assert_eq!(direct.title, "Plan my week");
        let checkin = restored
            .sessions
            .get(&agent_id, "schedule:schedule-9")
            .expect("the per-tick room became the automation's session");
        assert_eq!(checkin.kind, SessionKind::Checkin);
        assert_eq!(checkin.title, "Check-in · Review open tasks");
        assert_eq!(restored.sessions.len(), 2);
        assert!(restored
            .get_agent(&agent_id)
            .unwrap()
            .messages
            .iter()
            .filter(|message| message.id.starts_with('c'))
            .all(|message| message.room_id == "schedule:schedule-9"));
        assert_eq!(
            restored.runs.for_agent(&agent_id)[0].session_id,
            "schedule:schedule-9"
        );

        let saved = restored.control_plane_snapshot();
        let mut again = DaemonState::new();
        again.restore_control_plane_snapshot(saved.clone()).unwrap();
        assert_eq!(
            again.control_plane_snapshot().sessions,
            saved.sessions,
            "a second restore derives nothing new"
        );
    }

    #[test]
    fn tool_grant_sets_reach_existing_non_helper_agents_once() {
        const GRANTS: &[ToolGrantSet] = &[ToolGrantSet {
            id: "test-grant",
            read_class: &["todo_read", "not_a_tool"],
            write_class: &["todo_write"],
        }];
        let registry = crate::tools::ToolRegistry::new();
        let with_tools = |name: &str, tools: &[&str], additional: &[(&str, &str)]| {
            let mut agent = config(name, additional);
            agent.tools = Some(registry.resolve_descriptors(tools.iter().copied()).unwrap());
            agent
        };
        let mut state = DaemonState::new();
        let writer = state
            .create_agent(with_tools("writer", &["write_file"], &[]))
            .unwrap()
            .state
            .id;
        let reader = state
            .create_agent(with_tools("reader", &["read_file"], &[]))
            .unwrap()
            .state
            .id;
        let toolless = state
            .create_agent(config("toolless", &[]))
            .unwrap()
            .state
            .id;
        let helper = state
            .create_agent(with_tools(
                "helper",
                &["write_file"],
                &[
                    ("workspaceRole", "helper"),
                    ("parentAgentId", writer.as_str()),
                ],
            ))
            .unwrap()
            .state
            .id;

        let mut changed = state.apply_pending_tool_grants(GRANTS);
        changed.sort();
        let mut expected = vec![writer.clone(), reader.clone()];
        expected.sort();
        assert_eq!(changed, expected);
        let names = |id: &str| {
            state
                .get_agent(id)
                .unwrap()
                .state
                .config
                .tools
                .unwrap_or_default()
                .into_iter()
                .map(|tool| tool.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&writer), ["write_file", "todo_read", "todo_write"]);
        assert_eq!(names(&reader), ["read_file", "todo_read"]);
        assert_eq!(names(&helper), ["write_file"], "helpers never gain tools");
        assert!(
            state
                .get_agent(&toolless)
                .unwrap()
                .state
                .config
                .tools
                .is_none(),
            "an agent created without tools keeps none"
        );
        assert!(
            state.apply_pending_tool_grants(GRANTS).is_empty(),
            "a grant set applies once"
        );
        assert!(state
            .control_plane_snapshot()
            .tool_grants_applied
            .contains(&"test-grant".to_string()));
    }

    #[test]
    fn a_helper_role_agent_without_a_recorded_parent_id_gets_no_grants() {
        // The grant gate must use the same "helper" predicate as the rest of
        // the daemon (workspaceRole alone), not config_helper_parent's
        // stricter one, which also requires a recorded parentAgentId.
        const GRANTS: &[ToolGrantSet] = &[ToolGrantSet {
            id: "orphan-helper-grant",
            read_class: &["todo_read"],
            write_class: &[],
        }];
        let registry = crate::tools::ToolRegistry::new();
        let mut orphan_helper = config("orphan helper", &[("workspaceRole", "helper")]);
        orphan_helper.tools = Some(registry.resolve_descriptors(["read_file"]).unwrap());
        let mut state = DaemonState::new();
        let orphan_helper_id = state.create_agent(orphan_helper).unwrap().state.id;

        let changed = state.apply_pending_tool_grants(GRANTS);

        assert!(
            changed.is_empty(),
            "workspaceRole alone marks a helper, even without a recorded parentAgentId"
        );
        let tools = state
            .get_agent(&orphan_helper_id)
            .unwrap()
            .state
            .config
            .tools
            .unwrap_or_default()
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();
        assert_eq!(tools, ["read_file"]);
    }

    #[test]
    fn tool_grants_applied_survives_a_save_and_restore_round_trip() {
        const GRANTS: &[ToolGrantSet] = &[ToolGrantSet {
            id: "round-trip-grant",
            read_class: &["todo_read"],
            write_class: &[],
        }];
        let registry = crate::tools::ToolRegistry::new();
        let mut reader = config("reader", &[]);
        reader.tools = Some(registry.resolve_descriptors(["read_file"]).unwrap());
        let mut source = DaemonState::new();
        let agent_id = source.create_agent(reader).unwrap().state.id;

        let granted = source.apply_pending_tool_grants(GRANTS);
        assert_eq!(granted, vec![agent_id.clone()]);
        let snapshot = source.control_plane_snapshot();
        assert!(snapshot
            .tool_grants_applied
            .contains(&"round-trip-grant".to_string()));

        let mut restored = DaemonState::new();
        restored.restore_control_plane_snapshot(snapshot).unwrap();

        assert!(restored.tool_grants_applied.contains("round-trip-grant"));
        assert!(
            restored.apply_pending_tool_grants(GRANTS).is_empty(),
            "a grant set already applied before the restart never re-applies"
        );
    }

    #[test]
    fn every_listed_tool_grant_names_a_registered_tool() {
        let registry = crate::tools::ToolRegistry::new();
        let mut ids = HashSet::new();
        for grant in TOOL_GRANTS {
            assert!(ids.insert(grant.id), "duplicate grant id {}", grant.id);
            for name in grant.read_class.iter().chain(grant.write_class) {
                assert!(
                    registry.descriptor(name).is_some(),
                    "{name} is not registered"
                );
            }
        }
    }
}
