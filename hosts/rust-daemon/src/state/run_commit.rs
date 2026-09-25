//! Per-run isolation and exact change-set commit/rollback (spec §4.4 items 1–5).

use std::sync::Arc;

use anima_core::primitives::now_millis;
use anima_core::{
    AgentRuntime, AgentRuntimeSnapshot, AgentState, AgentStatus, Message, MessageRole,
    RuntimeRunBase,
};

use super::DaemonState;
use crate::runs::{RunChangeSet, RunError, RunOutcome, RunSource, RunStatus, AGENT_DELETED};
use crate::tools::ToolExecutionContext;

/// Interim `schedule:` room context cap (controller ruling, M2 pre-flight
/// audit finding 5): the newest whole turns a run's history keeps once
/// silent check-in pairs are dropped. M3's context selection replaces this.
const SCHEDULE_ROOM_CONTEXT_TURNS: usize = 10;

impl DaemonState {
    /// A tool context wired to this daemon's memory, workspace, and connectors.
    pub(crate) fn tool_execution_context(&self) -> ToolExecutionContext {
        ToolExecutionContext::new(
            Arc::clone(&self.memory),
            Arc::clone(&self.memory_embeddings),
            self.memory_store.clone(),
            self.tool_registry.clone(),
            Arc::clone(&self.process_manager),
            self.workspace
                .as_ref()
                .map(|workspace| workspace.root_path.clone()),
            self.calendar_manager.clone(),
        )
        .with_mail(self.mail_manager.clone())
    }

    /// An isolated runtime for one run of `agent_id` in `room_id`: the
    /// canonical state and counters with a trimmed copy of that room's
    /// history, the standard providers, evaluators, and database, and no
    /// Running→Failed restore conversion. In every room the copy starts at
    /// the first user message, so it never opens mid-turn (final fix wave
    /// A2); a `schedule:` room's copy also drops silent check-in pairs
    /// and keeps only the newest [`SCHEDULE_ROOM_CONTEXT_TURNS`] turns (the
    /// interim schedule-room cap). The run base counts the trimmed copy, and
    /// the canonical runtime and its transcript are not touched.
    pub(crate) fn build_run_runtime(
        &self,
        agent_id: &str,
        room_id: &str,
    ) -> Option<(AgentRuntime, ToolExecutionContext, RuntimeRunBase)> {
        let canonical = self.agents.get(agent_id)?;
        let mut history: Vec<Message> = canonical
            .messages()
            .iter()
            .filter(|message| message.room_id == room_id)
            .cloned()
            .collect();
        // Interim context guard (controller ruling, M2 pre-flight audit
        // finding 5): a `schedule:` room's history drops silent check-in
        // pairs (the same rule `crate::sessions::hidden_message_ids` gives
        // session views) and keeps only the newest whole turns. M3's context
        // selection replaces this for every room.
        if crate::sessions::schedule_id_of_room(room_id).is_some() {
            history = recent_turns(history);
        }
        // Every room (final fix wave A2): providers reject a tool result
        // whose call is missing, so a history that starts mid-turn (after a
        // prune, with an old assistant message kept alone because an
        // undelivered Telegram record still names it, or in a legacy
        // transcript) loses its messages before the first user message.
        // `run_base` below reads this trimmed copy's own length, so run
        // deltas and commits still see exactly what this run appends; the
        // canonical transcript above is only read, never written.
        let first_turn = crate::sessions::turn_starts(&history)
            .next()
            .unwrap_or(history.len());
        history.drain(..first_turn);
        let mut runtime = AgentRuntime::from_snapshot(
            canonical.run_snapshot(history),
            Arc::clone(&self.model_adapter),
        );
        self.wire_runtime(&mut runtime);
        let base = runtime.run_base();
        Some((runtime, self.tool_execution_context(), base))
    }

    /// Merges a finished run into its agent's canonical record and finalizes
    /// its ledger record. Returns `false`, merging nothing, when the agent was
    /// deleted while the run executed: that commit is discarded.
    pub(crate) fn commit_run(
        &mut self,
        change_set: &mut RunChangeSet,
        outcome: &RunOutcome,
    ) -> bool {
        let now_ms = now_millis();
        if !self.agents.contains_key(&change_set.agent_id) {
            if let Some(record) = self.runs.get_mut(&change_set.run_id) {
                record.finish(
                    RunStatus::Failed,
                    Some(RunError::new(
                        AGENT_DELETED,
                        "The agent was deleted before this run could be saved",
                    )),
                    now_ms,
                );
            }
            return false;
        }
        let runtime = self
            .agents
            .get_mut(&change_set.agent_id)
            .expect("agent existence was checked above");
        change_set.undo = Some(runtime.apply_run_delta(&change_set.delta));
        // Spec §3.2: activity follows the commit, and the owner's own turn is read.
        // Controller ruling (M2 pre-flight audit): a calendar write's own
        // confirmation follow-up is `api`-sourced like an owner call, but it
        // is not the owner's own turn.
        let owner_authored = self.runs.get(&change_set.run_id).is_some_and(|record| {
            matches!(record.source, RunSource::Api | RunSource::Web)
                && !crate::sessions::is_calendar_write_followup(record.source_ref.as_deref())
        }) || change_set
            .delta
            .messages
            .iter()
            .find(|message| message.role == MessageRole::User)
            .is_some_and(crate::sessions::is_owner_web_turn);
        change_set.session_undo = self.sessions.record_commit(
            &change_set.agent_id,
            &crate::sessions::session_id_for_room(&change_set.session_id),
            &change_set.delta.messages,
            owner_authored,
        );
        if let Some(record) = self.runs.get_mut(&change_set.run_id) {
            record.usage = change_set.token_delta.clone();
            record.tools_started = change_set.tools_started();
            // Per-model-call usage the observer kept (spec §4.1 `steps`).
            record.steps = self.live.runs().steps(&change_set.run_id);
            record.reply_message_id = outcome.reply_message_id.clone();
            record.finish(outcome.status, outcome.error(), now_ms);
        }
        self.runs.prune(now_ms);
        true
    }

    /// Removes exactly one committed run's messages, events, usage, and steps
    /// and records why its commit did not stand.
    ///
    /// This is the one exception to "nothing streamed is retracted" (spec
    /// §4.5): a rejected or undurable commit removes the messages that hold
    /// the text its run streamed. Clients saw that text only as `step.delta`
    /// events, never as `message.created`, and the run's `run.failed` event
    /// (`commit_rejected` or `commit_failed`) tells them to drop it.
    pub(crate) fn rollback_run(&mut self, change_set: &RunChangeSet, error: RunError) {
        if let (Some(runtime), Some(undo)) = (
            self.agents.get_mut(&change_set.agent_id),
            change_set.undo.clone(),
        ) {
            runtime.revert_run_delta(&change_set.delta, undo);
        }
        if let Some(record) = self.runs.get_mut(&change_set.run_id) {
            record.reply_message_id = None;
            record.finish(RunStatus::Failed, Some(error), now_millis());
        }
        if let Some(undo) = change_set.session_undo.clone() {
            self.sessions.revert_commit(undo);
        }
    }

    /// Reports `Running` while any run of the agent is in flight (spec §4.4 item 5).
    pub(super) fn with_derived_status(
        &self,
        mut snapshot: AgentRuntimeSnapshot,
    ) -> AgentRuntimeSnapshot {
        snapshot.state = self.with_derived_state(snapshot.state);
        snapshot
    }

    /// `with_derived_status` for an agent's state alone.
    pub(super) fn with_derived_state(&self, mut state: AgentState) -> AgentState {
        if self.in_flight_runs(&state.id) > 0 {
            state.status = AgentStatus::Running;
        }
        state
    }
}

/// `history` with silent check-in pairs excluded and only the newest
/// [`SCHEDULE_ROOM_CONTEXT_TURNS`] whole turns kept, cut at a turn start
/// ([`crate::sessions::turn_starts`]) so a tool-call turn is never split from
/// its results. With fewer turns nothing is cut here; `build_run_runtime`
/// drops any messages before the first user message in every room.
fn recent_turns(history: Vec<Message>) -> Vec<Message> {
    let hidden = crate::sessions::hidden_message_ids(history.iter());
    let mut visible: Vec<Message> = history
        .into_iter()
        .filter(|message| !hidden.contains(&message.id))
        .collect();
    let cutoff = crate::sessions::turn_starts(&visible)
        .rev()
        .nth(SCHEDULE_ROOM_CONTEXT_TURNS - 1);
    if let Some(cutoff) = cutoff {
        visible.drain(..cutoff);
    }
    visible
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashSet};
    use std::sync::Arc;

    use anima_core::{
        AgentConfig, AgentSettings, AgentStatus, Content, DataValue, Message, MessageRole,
        ModelAdapter, ModelGenerateRequest, ModelGenerateResponse, ModelStopReason,
        RuntimeRunDelta, TokenUsage,
    };
    use async_trait::async_trait;

    use crate::runs::{
        RunChangeSet, RunError, RunOutcome, RunRecord, RunSource, RunStart, RunStatus,
    };
    use crate::state::DaemonState;

    struct FixedUsageModel;

    #[async_trait]
    impl ModelAdapter for FixedUsageModel {
        fn provider(&self) -> &str {
            "fixed-usage"
        }

        async fn generate(
            &self,
            _config: &AgentConfig,
            request: &ModelGenerateRequest,
        ) -> Result<ModelGenerateResponse, String> {
            let input = request
                .messages
                .last()
                .map(|message| message.content.text.clone())
                .unwrap_or_default();
            Ok(ModelGenerateResponse {
                content: Content {
                    text: format!("reply to {input}"),
                    ..Content::default()
                },
                tool_calls: None,
                usage: TokenUsage {
                    prompt_tokens: 3,
                    completion_tokens: 4,
                    total_tokens: 7,
                    ..TokenUsage::default()
                },
                stop_reason: ModelStopReason::End,
            })
        }
    }

    /// Keeps the messages of every request, then answers like `FixedUsageModel`.
    #[derive(Default)]
    struct RecordingModel {
        requests: std::sync::Mutex<Vec<Vec<Message>>>,
    }

    #[async_trait]
    impl ModelAdapter for RecordingModel {
        fn provider(&self) -> &str {
            "recording"
        }

        async fn generate(
            &self,
            config: &AgentConfig,
            request: &ModelGenerateRequest,
        ) -> Result<ModelGenerateResponse, String> {
            self.requests.lock().unwrap().push(request.messages.clone());
            FixedUsageModel.generate(config, request).await
        }
    }

    fn state_with_agent() -> (DaemonState, String) {
        state_with_model(Arc::new(FixedUsageModel))
    }

    fn state_with_model(model: Arc<dyn ModelAdapter>) -> (DaemonState, String) {
        let mut state = DaemonState::with_model_adapter(model);
        let agent_id = state
            .create_agent(AgentConfig {
                name: "committer".into(),
                model: "fixed".into(),
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
                settings: Some(AgentSettings::default()),
            })
            .expect("agent should be created")
            .state
            .id;
        (state, agent_id)
    }

    fn start_run(state: &mut DaemonState, agent_id: &str, room_id: &str) -> String {
        let record = RunRecord::running(
            RunStart {
                agent_id: agent_id.into(),
                session_id: room_id.into(),
                source: RunSource::Api,
                source_ref: None,
                idempotency_key: None,
                text: "hello".into(),
                model: "fixed".into(),
                provider: None,
                parent_run_id: None,
            },
            anima_core::primitives::now_millis(),
        );
        let run_id = record.id.clone();
        state.runs.insert(record);
        run_id
    }

    async fn execute(
        state: &DaemonState,
        agent_id: &str,
        room_id: &str,
        run_id: &str,
        text: &str,
    ) -> (RunChangeSet, RunOutcome) {
        let (mut runtime, _tools, base) = state
            .build_run_runtime(agent_id, room_id)
            .expect("agent exists");
        let history = runtime.messages().to_vec();
        let result = runtime
            .run_in_room_with_context(
                room_id.into(),
                history,
                Content {
                    text: text.into(),
                    ..Content::default()
                },
            )
            .await;
        let change_set = RunChangeSet::new(
            run_id.into(),
            agent_id.into(),
            room_id.into(),
            runtime.run_delta_since(&base),
        );
        let outcome = RunOutcome::new(&change_set, result);
        (change_set, outcome)
    }

    #[tokio::test]
    async fn an_isolated_run_sees_only_its_room_and_commits_exactly_its_turn() {
        let (mut state, agent_id) = state_with_agent();
        let run_id = start_run(&mut state, &agent_id, "room-a");
        let (mut change_set, outcome) =
            execute(&state, &agent_id, "room-a", &run_id, "first").await;

        assert!(state.commit_run(&mut change_set, &outcome));

        let (other_room, _, _) = state.build_run_runtime(&agent_id, "room-b").unwrap();
        assert!(
            other_room.messages().is_empty(),
            "a room starts without other rooms' history"
        );
        let (same_room, _, _) = state.build_run_runtime(&agent_id, "room-a").unwrap();
        assert_eq!(same_room.messages().len(), 2);
        assert!(
            same_room.events().is_empty(),
            "run copies never carry the event log"
        );
        let agent = state.get_agent(&agent_id).unwrap();
        assert_eq!(agent.messages.len(), 2);
        assert_eq!(agent.state.token_usage.total_tokens, 7);
        assert_eq!(agent.state.status, AgentStatus::Completed);
        let reply = outcome
            .reply_message_id
            .as_deref()
            .expect("a successful run has a reply");
        assert!(agent.messages.iter().any(|message| {
            message.id == reply
                && message.role == MessageRole::Assistant
                && message.room_id == "room-a"
        }));
        let record = state.runs.get(&run_id).unwrap();
        assert_eq!(record.status, RunStatus::Completed);
        assert_eq!(record.usage.total_tokens, 7);
        assert!(record.finished_at_ms.is_some());
    }

    #[tokio::test]
    async fn two_rooms_commit_from_one_base_and_rolling_back_one_keeps_the_other() {
        let (mut state, agent_id) = state_with_agent();
        let run_a = start_run(&mut state, &agent_id, "room-a");
        let run_b = start_run(&mut state, &agent_id, "room-b");
        let (mut change_a, outcome_a) =
            execute(&state, &agent_id, "room-a", &run_a, "from a").await;
        let (mut change_b, outcome_b) =
            execute(&state, &agent_id, "room-b", &run_b, "from b").await;

        assert_eq!(
            state.get_agent(&agent_id).unwrap().state.status,
            AgentStatus::Running
        );
        assert!(state.commit_run(&mut change_a, &outcome_a));
        assert_eq!(
            state.get_agent(&agent_id).unwrap().state.status,
            AgentStatus::Running,
            "room b is still in flight"
        );
        assert!(state.commit_run(&mut change_b, &outcome_b));
        assert_eq!(state.get_agent(&agent_id).unwrap().messages.len(), 4);

        state.rollback_run(&change_a, RunError::new("commit_failed", "disk full"));

        let agent = state.get_agent(&agent_id).unwrap();
        assert_eq!(agent.messages.len(), 2);
        assert!(agent
            .messages
            .iter()
            .all(|message| message.room_id == "room-b"));
        assert_eq!(agent.state.token_usage.total_tokens, 7);
        assert_eq!(
            agent
                .last_task
                .as_ref()
                .and_then(|task| task.data.as_ref())
                .map(|content| content.text.as_str()),
            Some("reply to from b")
        );
        let rolled_back = state.runs.get(&run_a).unwrap();
        assert_eq!(rolled_back.status, RunStatus::Failed);
        assert_eq!(rolled_back.error.as_ref().unwrap().code, "commit_failed");
        assert_eq!(state.runs.get(&run_b).unwrap().status, RunStatus::Completed);
    }

    #[tokio::test]
    async fn a_commit_for_an_agent_removed_during_the_run_is_discarded() {
        let (mut state, agent_id) = state_with_agent();
        let run_id = start_run(&mut state, &agent_id, "room-a");
        let (mut change_set, outcome) = execute(&state, &agent_id, "room-a", &run_id, "late").await;

        state.remove_agent(&agent_id);

        assert!(!state.commit_run(&mut change_set, &outcome));
        assert!(state.get_agent(&agent_id).is_none());
        let record = state.runs.get(&run_id).unwrap();
        assert_eq!(record.status, RunStatus::Failed);
        assert_eq!(record.error.as_ref().unwrap().code, "agent_deleted");
    }

    #[test]
    fn agent_status_is_running_while_a_run_is_in_flight_and_otherwise_its_last_result() {
        let (mut state, agent_id) = state_with_agent();
        assert_eq!(
            state.get_agent(&agent_id).unwrap().state.status,
            AgentStatus::Idle
        );

        let run_id = start_run(&mut state, &agent_id, "room-a");
        assert_eq!(state.in_flight_runs(&agent_id), 1);
        assert_eq!(
            state.get_agent(&agent_id).unwrap().state.status,
            AgentStatus::Running
        );
        assert_eq!(state.list_agents()[0].state.status, AgentStatus::Running);

        state.runs.get_mut(&run_id).unwrap().finish(
            RunStatus::Completed,
            None,
            anima_core::primitives::now_millis(),
        );
        assert_eq!(state.in_flight_runs(&agent_id), 0);
        assert_eq!(
            state.get_agent(&agent_id).unwrap().state.status,
            AgentStatus::Idle
        );
    }

    #[test]
    fn agent_states_match_the_listed_agents_without_cloning_transcripts() {
        let (mut state, busy) = state_with_agent();
        let mut config = state.get_agent(&busy).unwrap().state.config;
        config.name = "second".into();
        let idle = state.create_agent(config).unwrap().state.id;
        start_run(&mut state, &busy, "room-a");

        let states = state.agent_states();

        assert_eq!(
            states,
            state
                .list_agents()
                .into_iter()
                .map(|snapshot| snapshot.state)
                .collect::<Vec<_>>(),
            "the same agents, order, and derived status as list_agents"
        );
        let status = |id: &str| {
            states
                .iter()
                .find(|agent| agent.id == id)
                .map(|agent| agent.status)
        };
        assert_eq!(status(&busy), Some(AgentStatus::Running));
        assert_eq!(status(&idle), Some(AgentStatus::Idle));
    }

    #[test]
    fn a_saved_snapshot_marks_agents_with_in_flight_runs_as_running() {
        let (mut state, agent_id) = state_with_agent();
        start_run(&mut state, &agent_id, "room-a");

        let snapshot = state.control_plane_snapshot();
        assert_eq!(snapshot.agents[0].state.status, AgentStatus::Running);

        let mut restored = DaemonState::new();
        restored.restore_control_plane_snapshot(snapshot).unwrap();
        assert_eq!(
            restored.get_agent(&agent_id).unwrap().state.status,
            AgentStatus::Failed,
            "restart handling of an interrupted agent is unchanged"
        );
        assert_eq!(restored.in_flight_runs(&agent_id), 0);
    }

    fn history_message(
        agent_id: &str,
        room_id: &str,
        id: &str,
        role: MessageRole,
        text: &str,
        checkin: bool,
    ) -> Message {
        Message {
            id: id.to_string(),
            agent_id: agent_id.to_string(),
            room_id: room_id.to_string(),
            content: Content {
                text: text.to_string(),
                attachments: None,
                metadata: checkin.then(|| {
                    BTreeMap::from([("kind".to_string(), DataValue::String("checkin".into()))])
                }),
            },
            role,
            created_at_ms: 1,
        }
    }

    /// Appends messages straight to the agent's canonical transcript, bypassing
    /// the run machinery: `build_run_runtime` only reads `self.agents`, so its
    /// interim `schedule:` room guard needs a long history to trim, not a real run.
    fn seed_history(state: &mut DaemonState, agent_id: &str, messages: Vec<Message>) {
        state
            .agents
            .get_mut(agent_id)
            .expect("agent exists")
            .apply_run_delta(&RuntimeRunDelta {
                messages,
                events: Vec::new(),
                event_total: 0,
                token_usage: TokenUsage::default(),
                step_count: 0,
                last_task: None,
                status: AgentStatus::Idle,
            });
    }

    /// Ruling test 1/2 (M2 pre-flight audit, finding 5): a `schedule:` room
    /// with 30 prior ticks, half silent, gives the model at most 10 visible
    /// turns and no `CHECKIN_OK` pairs.
    #[test]
    fn schedule_room_context_hides_silent_checkins_and_keeps_the_newest_ten_turns() {
        let (mut state, agent_id) = state_with_agent();
        let room = crate::sessions::schedule_room_id("schedule-1");
        let mut messages = Vec::new();
        for tick in 0..30u32 {
            let silent = tick % 2 == 0;
            let reply_text = if silent {
                "CHECKIN_OK".to_string()
            } else {
                format!("Reply {tick}")
            };
            messages.push(history_message(
                &agent_id,
                &room,
                &format!("user-{tick}"),
                MessageRole::User,
                "Check status",
                true,
            ));
            messages.push(history_message(
                &agent_id,
                &room,
                &format!("assistant-{tick}"),
                MessageRole::Assistant,
                &reply_text,
                false,
            ));
        }
        seed_history(&mut state, &agent_id, messages);

        let (runtime, _tools, _base) = state.build_run_runtime(&agent_id, &room).unwrap();

        let expected: HashSet<String> = (11..30)
            .step_by(2)
            .flat_map(|tick| [format!("user-{tick}"), format!("assistant-{tick}")])
            .collect();
        let actual: HashSet<String> = runtime.messages().iter().map(|m| m.id.clone()).collect();
        assert_eq!(
            actual, expected,
            "the newest 10 spoken turns remain; every silent check-in pair is dropped"
        );
        assert_eq!(
            runtime
                .messages()
                .iter()
                .filter(|message| message.role == MessageRole::User)
                .count(),
            10,
            "at most 10 visible turns reach the model"
        );
        assert!(
            runtime
                .messages()
                .iter()
                .all(|message| message.content.text.trim() != "CHECKIN_OK"),
            "no CHECKIN_OK pair leaks into the model's context"
        );
    }

    /// Ruling test 2/2: a tool-call turn is kept whole even when it sits at
    /// the newest-10-turns cutoff boundary.
    #[test]
    fn schedule_room_context_keeps_a_tool_call_turn_whole() {
        let (mut state, agent_id) = state_with_agent();
        let room = crate::sessions::schedule_room_id("schedule-2");
        let mut messages = vec![
            history_message(&agent_id, &room, "user-0", MessageRole::User, "old", false),
            history_message(
                &agent_id,
                &room,
                "assistant-0",
                MessageRole::Assistant,
                "old reply",
                false,
            ),
            history_message(
                &agent_id,
                &room,
                "user-1",
                MessageRole::User,
                "run the tool",
                false,
            ),
            history_message(
                &agent_id,
                &room,
                "assistant-1-call",
                MessageRole::Assistant,
                "using a tool",
                false,
            ),
            history_message(
                &agent_id,
                &room,
                "tool-1-result",
                MessageRole::Tool,
                "tool output",
                false,
            ),
            history_message(
                &agent_id,
                &room,
                "assistant-1-final",
                MessageRole::Assistant,
                "done",
                false,
            ),
        ];
        for tick in 2..11u32 {
            messages.push(history_message(
                &agent_id,
                &room,
                &format!("user-{tick}"),
                MessageRole::User,
                "tick",
                false,
            ));
            messages.push(history_message(
                &agent_id,
                &room,
                &format!("assistant-{tick}"),
                MessageRole::Assistant,
                "reply",
                false,
            ));
        }
        seed_history(&mut state, &agent_id, messages);

        let (runtime, _tools, _base) = state.build_run_runtime(&agent_id, &room).unwrap();

        let ids: Vec<&str> = runtime.messages().iter().map(|m| m.id.as_str()).collect();
        assert!(
            !ids.contains(&"user-0") && !ids.contains(&"assistant-0"),
            "the oldest turn is trimmed away"
        );
        for id in [
            "user-1",
            "assistant-1-call",
            "tool-1-result",
            "assistant-1-final",
        ] {
            assert!(
                ids.contains(&id),
                "the tool-call turn at the cutoff stays whole: missing {id}"
            );
        }
        assert_eq!(
            runtime
                .messages()
                .iter()
                .filter(|message| message.role == MessageRole::User)
                .count(),
            10,
            "10 turns remain: the tool-call turn plus 9 simple ticks"
        );
    }

    /// Final fix wave A2: a room's history that starts mid-turn (a tool
    /// result whose call is gone, or an old assistant message kept alone)
    /// reaches the model from its first user message in every room, while the
    /// canonical transcript keeps every message and gains exactly the run.
    #[tokio::test]
    async fn a_run_history_that_starts_mid_turn_reaches_the_model_from_its_first_user_message() {
        let schedule_room = crate::sessions::schedule_room_id("schedule-3");
        for (room, leading_role) in [
            ("chat:a", MessageRole::Tool),
            ("chat:a", MessageRole::Assistant),
            (schedule_room.as_str(), MessageRole::Tool),
        ] {
            let context = format!("{room} starting with a {leading_role:?} message");
            let model = Arc::new(RecordingModel::default());
            let (mut state, agent_id) = state_with_model(model.clone());
            seed_history(
                &mut state,
                &agent_id,
                vec![
                    history_message(&agent_id, room, "leading", leading_role, "old", false),
                    history_message(
                        &agent_id,
                        room,
                        "leading-reply",
                        MessageRole::Assistant,
                        "old reply",
                        false,
                    ),
                    history_message(&agent_id, room, "user-1", MessageRole::User, "hi", false),
                    history_message(
                        &agent_id,
                        room,
                        "assistant-1",
                        MessageRole::Assistant,
                        "hello",
                        false,
                    ),
                ],
            );
            let run_id = start_run(&mut state, &agent_id, room);

            let (mut change_set, outcome) = execute(&state, &agent_id, room, &run_id, "next").await;

            let requests = model.requests.lock().unwrap().clone();
            let sent: Vec<(&str, MessageRole)> = requests[0]
                .iter()
                .map(|message| (message.content.text.as_str(), message.role))
                .collect();
            assert_eq!(
                sent,
                [
                    ("hi", MessageRole::User),
                    ("hello", MessageRole::Assistant),
                    ("next", MessageRole::User),
                ],
                "{context}: the model sees whole turns only"
            );
            assert!(state.commit_run(&mut change_set, &outcome));
            let transcript: Vec<String> = state
                .get_agent(&agent_id)
                .unwrap()
                .messages
                .into_iter()
                .map(|message| message.id)
                .collect();
            assert_eq!(
                transcript[..4],
                ["leading", "leading-reply", "user-1", "assistant-1"],
                "{context}: the canonical transcript is never trimmed"
            );
            assert_eq!(
                transcript.len(),
                6,
                "{context}: the commit adds exactly the run's turn"
            );
        }
    }
}
