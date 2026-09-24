//! Hot-tail pruning (spec §13.2): the control plane keeps each session's
//! newest messages and anything recent; older messages the history store
//! already holds leave the snapshot every ten minutes.

use std::collections::{HashMap, HashSet};

use anima_core::{AgentRuntimeSnapshot, Message};
use tracing::warn;

use super::{hidden_message_ids, session_id_for_room};
use crate::app::SharedDaemonState;
use crate::connectors::OutboundDeliveryState;
use crate::state::DaemonState;

/// Each session keeps at least its newest 200 visible messages (spec §16).
/// Hidden messages -- silent check-in pairs (spec §3.3) -- take no place
/// among them (Controller ruling 1, M2 pre-flight audit).
pub(crate) const HOT_TAIL_MESSAGES: usize = 200;
/// Messages created in the last 24 hours always stay.
pub(crate) const HOT_TAIL_MIN_AGE_MS: u64 = 24 * 60 * 60 * 1_000;
/// How often the history worker prunes.
pub(crate) const PRUNE_INTERVAL_MS: u64 = 10 * 60 * 1_000;

/// What one pruning pass changed, so a failed save can put it back.
#[derive(Debug, Default)]
pub(crate) struct PruneUndo {
    agents: Vec<AgentRuntimeSnapshot>,
    marked_outbound: Vec<String>,
    pub(crate) message_ids: Vec<String>,
}

impl DaemonState {
    /// Removes the hot messages that may leave the control plane: mirrored,
    /// outside their session's newest 200 visible messages, older than 24
    /// hours, not referenced by an undelivered Telegram record, and not in a
    /// session with an active run. Delivered records of pruned messages are
    /// marked `messagePruned`. `None` when nothing may be pruned (an
    /// ephemeral store, a store not yet reconciled since startup, or no
    /// candidates).
    pub(crate) fn prune_hot_tail(&mut self, now_ms: u64) -> Option<PruneUndo> {
        if self.history.is_ephemeral() || !self.history.reconciled() {
            return None;
        }
        let cutoff = now_ms.saturating_sub(HOT_TAIL_MIN_AGE_MS);
        let undelivered_references = self
            .outbound
            .values()
            .filter(|record| record.delivery_state != OutboundDeliveryState::Delivered)
            .map(|record| record.assistant_message_id.as_str())
            .collect::<HashSet<_>>();
        let active_sessions = self.runs.active_sessions();
        let mut prunable_by_agent = Vec::new();
        for (agent_id, runtime) in &self.agents {
            let mut rooms: HashMap<&str, Vec<&Message>> = HashMap::new();
            for message in runtime.messages() {
                rooms
                    .entry(message.room_id.as_str())
                    .or_default()
                    .push(message);
            }
            let mut prunable = HashSet::new();
            for (room_id, messages) in rooms {
                // A room no longer than the window has no candidates.
                if messages.len() <= HOT_TAIL_MESSAGES
                    || active_sessions.contains(&(agent_id.clone(), session_id_for_room(room_id)))
                {
                    continue;
                }
                prunable.extend(
                    outside_newest_visible(&messages)
                        .into_iter()
                        .filter(|message| {
                            message.created_at_ms <= cutoff
                                && !undelivered_references.contains(message.id.as_str())
                                && self.history.is_mirrored(&message.id)
                        })
                        .map(|message| message.id.clone()),
                );
            }
            if !prunable.is_empty() {
                prunable_by_agent.push((agent_id.clone(), prunable));
            }
        }
        if prunable_by_agent.is_empty() {
            return None;
        }
        let mut undo = PruneUndo::default();
        for (agent_id, prunable) in prunable_by_agent {
            let runtime = self
                .agents
                .get_mut(&agent_id)
                .expect("the agent was just read");
            undo.agents.push(runtime.snapshot());
            undo.message_ids.extend(
                runtime
                    .retain_messages(|message| !prunable.contains(&message.id))
                    .into_iter()
                    .map(|message| message.id),
            );
        }
        let pruned = undo
            .message_ids
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        for (id, record) in self.outbound.iter_mut() {
            if record.delivery_state == OutboundDeliveryState::Delivered
                && !record.message_pruned
                && pruned.contains(record.assistant_message_id.as_str())
            {
                record.message_pruned = true;
                undo.marked_outbound.push(id.clone());
            }
        }
        Some(undo)
    }

    /// Puts back what `prune_hot_tail` removed after its save failed.
    pub(crate) fn revert_prune(&mut self, undo: PruneUndo) {
        for snapshot in undo.agents {
            let agent_id = snapshot.state.id.clone();
            if let Err(error) = self.restore_removed_agent(snapshot) {
                warn!(agent_id = %agent_id, error = %error, "could not restore an agent's hot tail after a failed prune");
            }
        }
        for id in undo.marked_outbound {
            if let Some(record) = self.outbound.get_mut(&id) {
                record.message_pruned = false;
            }
        }
    }
}

/// One room's messages that are not among its newest `HOT_TAIL_MESSAGES`
/// visible ones, newest first; `messages` are the room's messages in
/// transcript order. Only visible messages take a place (Controller ruling 1,
/// M2 pre-flight audit), so a hidden message stays while fewer than
/// `HOT_TAIL_MESSAGES` visible messages are newer than it.
fn outside_newest_visible<'a>(messages: &[&'a Message]) -> Vec<&'a Message> {
    let hidden = hidden_message_ids(messages.iter().copied());
    let mut newer_visible = 0;
    let mut outside = Vec::new();
    for message in messages.iter().rev().copied() {
        if newer_visible >= HOT_TAIL_MESSAGES {
            outside.push(message);
        }
        if !hidden.contains(&message.id) {
            newer_visible += 1;
        }
    }
    outside
}

/// One pruning pass inside a control-plane transaction; returns how many
/// messages left the hot tail. A failed save puts everything back. The
/// history worker does not call this: it takes the transaction itself,
/// re-checks under it that an owner is still alive, and then calls
/// `prune_in_transaction`.
#[cfg(test)]
pub(crate) async fn prune_once(
    state: &SharedDaemonState,
    transactions: &std::sync::Arc<tokio::sync::Mutex<()>>,
    now_ms: u64,
) -> Result<usize, String> {
    let _transaction = transactions.lock().await;
    prune_in_transaction(state, now_ms).await
}

/// One pruning pass for a caller that already holds the control-plane
/// transaction; returns how many messages left the hot tail. Call it only
/// while holding that transaction, so no commit, deletion, or other save
/// interleaves with the prune, its save, or its revert. The state lock is
/// never held across the save; a failed save puts everything back, and a
/// saved prune forgets the pruned ids in the mirrored set.
pub(crate) async fn prune_in_transaction(
    state: &SharedDaemonState,
    now_ms: u64,
) -> Result<usize, String> {
    let (undo, persist) = {
        let mut guard = state.write().await;
        let Some(undo) = guard.prune_hot_tail(now_ms) else {
            return Ok(0);
        };
        (undo, guard.control_plane_persist_request())
    };
    if let Err(error) = persist.save().await {
        state.write().await.revert_prune(undo);
        return Err(error.to_string());
    }
    let history = state.read().await.history.clone();
    history.forget_mirrored(undo.message_ids.iter().map(String::as_str));
    Ok(undo.message_ids.len())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use anima_core::{Message, MessageRole};
    use tokio::sync::{Mutex, RwLock};

    use super::*;
    use crate::connectors::{TelegramBotIdentity, TelegramConnectorRecord, TelegramOutboundRecord};
    use crate::history::conformance::FlakyHistoryStore;
    use crate::history::HistoryService;
    use crate::runs::{RunRecord, RunSource, RunStart};
    use crate::sessions::test_support::{agent_config, checkin_prompt, message, seed_messages};

    const DAY_MS: u64 = 24 * 60 * 60 * 1_000;
    const NOW_MS: u64 = 10 * DAY_MS;

    fn room(agent_id: &str, room_id: &str, prefix: &str, count: usize) -> Vec<Message> {
        (0..count)
            .map(|index| {
                let role = if index % 2 == 0 {
                    MessageRole::User
                } else {
                    MessageRole::Assistant
                };
                message(
                    agent_id,
                    &format!("{prefix}{index:03}"),
                    room_id,
                    role,
                    "old",
                    1_000 + index as u64,
                )
            })
            .collect()
    }

    /// A non-ephemeral store holding every seeded message, reconciled.
    async fn mirrored_state(
        seed: impl FnOnce(&str) -> Vec<Message>,
    ) -> (SharedDaemonState, String) {
        let mut daemon = DaemonState::new();
        daemon.set_history(HistoryService::new(Arc::new(FlakyHistoryStore::new())));
        let agent = daemon
            .create_agent(agent_config("companion"))
            .unwrap()
            .state
            .id;
        let messages = seed(&agent);
        seed_messages(&mut daemon, &agent, messages);
        let state = Arc::new(RwLock::new(daemon));
        let history = state.read().await.history.clone();
        history
            .flush_once(&state, &Mutex::new(()), NOW_MS)
            .await
            .unwrap();
        (state, agent)
    }

    fn outbound(
        agent_id: &str,
        id: &str,
        message_id: &str,
        delivery_state: OutboundDeliveryState,
    ) -> TelegramOutboundRecord {
        TelegramOutboundRecord {
            id: id.into(),
            connector_id: "telegram-a".into(),
            agent_id: agent_id.into(),
            room_id: "chat:a".into(),
            assistant_message_id: message_id.into(),
            text: "old".into(),
            created_at_ms: 1_000,
            delivered_at_ms: None,
            attempts: 1,
            delivery_state,
            message_pruned: false,
        }
    }

    #[tokio::test]
    async fn pruning_keeps_each_sessions_newest_recent_unmirrored_and_referenced_messages() {
        let (state, agent) = mirrored_state(|agent| {
            let mut messages = room(agent, "chat:a", "a", 207);
            messages[1].created_at_ms = NOW_MS - 1_000;
            messages.extend(room(agent, "chat:c", "c", 201));
            messages.extend(room(agent, "chat:b", "b", 3));
            messages
        })
        .await;
        let mut guard = state.write().await;
        guard.history.forget_mirrored(["a003"]);
        for (id, message_id, delivery_state) in [
            ("pending", "a004", OutboundDeliveryState::Pending),
            ("delivered", "a005", OutboundDeliveryState::Delivered),
            ("failed", "a006", OutboundDeliveryState::Failed),
        ] {
            guard
                .outbound
                .insert(id.into(), outbound(&agent, id, message_id, delivery_state));
        }
        guard.runs.insert(RunRecord::running(
            RunStart {
                agent_id: agent.clone(),
                session_id: "chat:c".into(),
                source: RunSource::Web,
                source_ref: None,
                idempotency_key: None,
                text: "still working".into(),
                model: "gpt-5.4".into(),
                provider: None,
                parent_run_id: None,
            },
            NOW_MS,
        ));

        let undo = guard
            .prune_hot_tail(NOW_MS)
            .expect("old mirrored messages leave the hot tail");

        let mut pruned = undo.message_ids.clone();
        pruned.sort();
        assert_eq!(pruned, ["a000", "a002", "a005"]);
        let hot = guard.get_agent(&agent).unwrap().messages;
        assert_eq!(hot.len(), 207 + 201 + 3 - 3);
        assert!(
            hot.iter().any(|message| message.id == "c000"),
            "a session with an active run keeps its whole transcript"
        );
        assert!(guard.outbound["delivered"].message_pruned);
        assert!(!guard.outbound["pending"].message_pruned);
        assert!(!guard.outbound["failed"].message_pruned);
        guard.revert_prune(undo);
        assert_eq!(
            guard.get_agent(&agent).unwrap().messages.len(),
            207 + 201 + 3
        );
        assert!(!guard.outbound["delivered"].message_pruned);
    }

    /// One check-in turn in `schedule:s1`: the scheduler's prompt, then the
    /// agent's reply.
    fn checkin_turn(agent_id: &str, turn: &str, reply: &str, created_at_ms: u64) -> [Message; 2] {
        [
            checkin_prompt(
                agent_id,
                &format!("{turn}-prompt"),
                "schedule:s1",
                "s1",
                "Anything new?",
                created_at_ms,
            ),
            message(
                agent_id,
                &format!("{turn}-reply"),
                "schedule:s1",
                MessageRole::Assistant,
                reply,
                created_at_ms + 1,
            ),
        ]
    }

    #[tokio::test]
    async fn silent_checkin_pairs_do_not_take_a_place_among_a_sessions_newest_200() {
        // Controller ruling 1 (M2 pre-flight audit): only visible messages
        // count toward the newest 200.
        let (state, agent) = mirrored_state(|agent| {
            let mut messages = Vec::new();
            let mut at = 1_000;
            let mut turn = |messages: &mut Vec<Message>, turn: String, reply: &str| {
                messages.extend(checkin_turn(agent, &turn, reply, at));
                at += 2;
            };
            for index in 0..3 {
                turn(&mut messages, format!("silent-{index}"), "CHECKIN_OK");
            }
            for index in 0..100 {
                turn(
                    &mut messages,
                    format!("visible-{index:03}"),
                    "Here is an update.",
                );
                if index >= 95 {
                    turn(
                        &mut messages,
                        format!("recent-silent-{index}"),
                        "CHECKIN_OK",
                    );
                }
            }
            messages
        })
        .await;
        let mut guard = state.write().await;

        let undo = guard
            .prune_hot_tail(NOW_MS)
            .expect("the silent pairs older than the newest 200 visible messages leave");

        let mut pruned = undo.message_ids.clone();
        pruned.sort();
        assert_eq!(
            pruned,
            [
                "silent-0-prompt",
                "silent-0-reply",
                "silent-1-prompt",
                "silent-1-reply",
                "silent-2-prompt",
                "silent-2-reply",
            ]
        );
        let hot = guard.get_agent(&agent).unwrap().messages;
        assert_eq!(
            hot.iter()
                .filter(|message| message.id.starts_with("visible-"))
                .count(),
            200,
            "silent pairs never push a visible message out of the newest 200"
        );
        assert_eq!(
            hot.len(),
            210,
            "silent pairs among the newest 200 visible messages stay with them"
        );
    }

    #[tokio::test]
    async fn pruning_is_off_for_ephemeral_stores_and_until_reconciled() {
        let mut ephemeral = DaemonState::new();
        let agent = ephemeral
            .create_agent(agent_config("companion"))
            .unwrap()
            .state
            .id;
        seed_messages(&mut ephemeral, &agent, room(&agent, "chat:a", "a", 201));
        let ephemeral = Arc::new(RwLock::new(ephemeral));
        let history = ephemeral.read().await.history.clone();
        history
            .flush_once(&ephemeral, &Mutex::new(()), NOW_MS)
            .await
            .unwrap();
        assert!(
            ephemeral.write().await.prune_hot_tail(NOW_MS).is_none(),
            "an ephemeral store keeps every message hot"
        );

        let mut unreconciled = DaemonState::new();
        unreconciled.set_history(HistoryService::new(Arc::new(FlakyHistoryStore::new())));
        let agent = unreconciled
            .create_agent(agent_config("companion"))
            .unwrap()
            .state
            .id;
        seed_messages(&mut unreconciled, &agent, room(&agent, "chat:a", "a", 201));
        assert!(
            unreconciled.prune_hot_tail(NOW_MS).is_none(),
            "nothing is pruned before the store was reconciled"
        );
    }

    #[tokio::test]
    async fn a_failed_prune_save_restores_the_hot_tail_and_a_saved_prune_forgets_mirrored_ids() {
        let (state, agent) = mirrored_state(|agent| room(agent, "chat:a", "a", 201)).await;
        let transactions = Arc::new(Mutex::new(()));
        let gate = state
            .write()
            .await
            .install_test_control_plane_save_gate(true);
        gate.release.add_permits(1);

        let error = prune_once(&state, &transactions, NOW_MS)
            .await
            .expect_err("the save failed");
        assert_eq!(error, "injected control-plane save failure");
        assert_eq!(
            state.read().await.get_agent(&agent).unwrap().messages.len(),
            201
        );
        assert!(state.read().await.history.is_mirrored("a000"));

        assert_eq!(prune_once(&state, &transactions, NOW_MS).await, Ok(1));
        let guard = state.read().await;
        let hot = guard.get_agent(&agent).unwrap().messages;
        assert_eq!(hot.len(), 200);
        assert_eq!(hot[0].id, "a001");
        assert!(
            !guard.history.is_mirrored("a000"),
            "pruned ids are no longer hot"
        );
    }

    #[tokio::test]
    async fn a_prune_waits_for_the_transaction_and_saves_without_the_state_lock() {
        let (state, agent) = mirrored_state(|agent| room(agent, "chat:a", "a", 201)).await;
        let transactions = Arc::new(Mutex::new(()));
        let held = transactions.lock().await;
        let prune = tokio::spawn({
            let state = Arc::clone(&state);
            let transactions = Arc::clone(&transactions);
            async move { prune_once(&state, &transactions, NOW_MS).await }
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            !prune.is_finished(),
            "a commit or deletion holding the transaction keeps the prune waiting"
        );
        assert_eq!(
            state.read().await.get_agent(&agent).unwrap().messages.len(),
            201
        );

        let gate = state
            .write()
            .await
            .install_test_control_plane_save_gate(false);
        drop(held);
        gate.entered.acquire().await.unwrap().forget();
        {
            let guard = state
                .try_read()
                .expect("the state lock is free while the prune saves");
            assert_eq!(guard.get_agent(&agent).unwrap().messages.len(), 200);
        }
        gate.release.add_permits(1);
        assert_eq!(prune.await.unwrap(), Ok(1));
    }

    #[tokio::test]
    async fn a_snapshot_saved_after_pruning_a_delivered_reply_still_restores() {
        let (state, agent) =
            mirrored_state(|agent| room(agent, "telegram:telegram-a", "t", 202)).await;
        {
            let mut guard = state.write().await;
            guard.connectors.insert(
                "telegram-a".into(),
                TelegramConnectorRecord {
                    id: "telegram-a".into(),
                    agent_id: agent.clone(),
                    room_id: "telegram:telegram-a".into(),
                    bot: TelegramBotIdentity {
                        id: "bot-1".into(),
                        username: Some("test_bot".into()),
                        display_name: None,
                    },
                    approved_chat: None,
                    pending_pairing: None,
                    next_update_id: 0,
                    enabled: true,
                    deleted_at_ms: None,
                    created_at_ms: 1,
                    updated_at_ms: 1,
                },
            );
            let mut record = outbound(
                &agent,
                "delivered",
                "t001",
                OutboundDeliveryState::Delivered,
            );
            record.room_id = "telegram:telegram-a".into();
            record.delivered_at_ms = Some(2_000);
            guard.outbound.insert("delivered".into(), record);
        }

        assert_eq!(
            prune_once(&state, &Arc::new(Mutex::new(())), NOW_MS).await,
            Ok(2)
        );

        let snapshot = serde_json::to_value(state.read().await.control_plane_snapshot()).unwrap();
        assert_eq!(snapshot["outbound"][0]["messagePruned"], true);
        let mut restored = DaemonState::new();
        restored
            .restore_control_plane_snapshot(serde_json::from_value(snapshot).unwrap())
            .expect("a delivered reply that left the hot tail does not block a restart");
        assert!(restored.outbound["delivered"].message_pruned);
        assert_eq!(restored.get_agent(&agent).unwrap().messages.len(), 200);
    }

    #[test]
    fn message_pruned_is_serialized_only_once_set() {
        let mut record = outbound(
            "agent-1",
            "delivered",
            "m-1",
            OutboundDeliveryState::Delivered,
        );
        let json = serde_json::to_value(&record).unwrap();
        assert!(json.get("messagePruned").is_none(), "omitted while false");
        assert_eq!(
            serde_json::from_value::<TelegramOutboundRecord>(json).unwrap(),
            record,
            "a record saved before pruning existed reads as not pruned"
        );

        record.message_pruned = true;
        let json = serde_json::to_value(&record).unwrap();
        assert_eq!(json["messagePruned"], true);
        assert_eq!(
            serde_json::from_value::<TelegramOutboundRecord>(json).unwrap(),
            record
        );
    }

    #[tokio::test]
    async fn after_pruning_list_agents_and_every_agent_read_carry_only_the_hot_tail() {
        // Controller ruling 2 (M2 pre-flight audit): agent reads come from
        // the canonical runtimes, so a restored agent's boot-time transcript
        // is neither kept nor listed beside its pruned hot tail.
        let mut source = DaemonState::new();
        let agent = source
            .create_agent(agent_config("companion"))
            .unwrap()
            .state
            .id;
        seed_messages(&mut source, &agent, room(&agent, "chat:a", "a", 201));
        let mut daemon = DaemonState::new();
        daemon.set_history(HistoryService::new(Arc::new(FlakyHistoryStore::new())));
        daemon
            .restore_control_plane_snapshot(source.control_plane_snapshot())
            .unwrap();
        let state = Arc::new(RwLock::new(daemon));
        let history = state.read().await.history.clone();
        history
            .flush_once(&state, &Mutex::new(()), NOW_MS)
            .await
            .unwrap();

        assert_eq!(
            prune_once(&state, &Arc::new(Mutex::new(())), NOW_MS).await,
            Ok(1)
        );

        let guard = state.read().await;
        let listed = guard.list_agents();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].message_count, HOT_TAIL_MESSAGES);
        assert_eq!(listed[0].messages.len(), HOT_TAIL_MESSAGES);
        assert_eq!(listed[0].messages[0].id, "a001");
        assert_eq!(
            guard.get_agent(&agent).unwrap().messages.len(),
            HOT_TAIL_MESSAGES
        );
        assert_eq!(
            guard.control_plane_snapshot().agents[0].messages.len(),
            HOT_TAIL_MESSAGES
        );
        assert_eq!(guard.agent_summaries()[0].message_count, HOT_TAIL_MESSAGES);
    }
}
