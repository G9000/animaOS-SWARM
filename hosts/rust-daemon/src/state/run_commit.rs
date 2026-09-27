//! Per-run isolation and exact change-set commit/rollback (spec §4.4 items 1–5).

use std::sync::Arc;

use anima_core::primitives::now_millis;
use anima_core::{
    select_context, AgentRuntime, AgentRuntimeSnapshot, AgentState, AgentStatus, Content,
    ContextSummary, DataValue, Message, MessageRole, Provider, RuntimeRunBase, TokenEstimator,
    REVISED_METADATA_KEY,
};

use super::DaemonState;
use crate::runs::{RunChangeSet, RunError, RunOutcome, RunSource, RunStatus, AGENT_DELETED};
use crate::sessions::context::{
    newest_left_out, uncovered_pruned_through, ContextBudget, SessionSummaryProvider,
};
use crate::tools::ToolExecutionContext;

/// What a run starts from (spec §4.4 item 1, §5).
pub(crate) struct RunBuild {
    pub(crate) runtime: AgentRuntime,
    pub(crate) tools: ToolExecutionContext,
    pub(crate) base: RuntimeRunBase,
    pub(crate) context: RunContextReport,
}

/// How a run's history was selected (spec §5).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RunContextReport {
    pub(crate) budget_tokens: u64,
    /// The selected history, the summary, and the current message, estimated
    /// without calibration: the denominator of the next calibration. It
    /// leaves out the system prompt and the tool schemas; the factor absorbs
    /// that fixed overhead (audit M10), so it often sits at the 2.0 clamp in
    /// a short session and settles as the history grows.
    pub(crate) raw_estimate_tokens: u64,
    /// The newest message left out that no summary covers (spec §5.3): the
    /// newest dropped turn's last message, or the newest pruned message when
    /// the summary does not reach it (audit I5) and it is newer.
    pub(crate) trimmed_through: Option<String>,
    /// Every message the selection left out, oldest first (compaction's
    /// input, Task 12); pruned messages are not in the control plane.
    pub(crate) dropped: Vec<Message>,
}

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

    /// The room's messages the model may see, oldest first: everything but
    /// silent check-in pairs (spec §5.2) and drafts an evaluator sent back
    /// (`revised`; controller ruling, M3 pre-flight audit M17), which stay in
    /// the transcript because nothing streamed is retracted. The system
    /// message right after such a draft goes too (fix round 1): it is the
    /// evaluator's request to revise an answer the model no longer sees.
    pub(crate) fn model_visible_history(&self, agent_id: &str, room_id: &str) -> Vec<Message> {
        let Some(canonical) = self.agents.get(agent_id) else {
            return Vec::new();
        };
        let room: Vec<&Message> = canonical
            .messages()
            .iter()
            .filter(|message| message.room_id == room_id)
            .collect();
        let hidden = crate::sessions::hidden_message_ids(room.iter().copied());
        room.iter()
            .enumerate()
            .filter(|&(index, message)| {
                let revision_request = message.role == MessageRole::System
                    && index
                        .checked_sub(1)
                        .is_some_and(|previous| is_revised_draft(room[previous]));
                !hidden.contains(&message.id) && !is_revised_draft(message) && !revision_request
            })
            .map(|(_, message)| (*message).clone())
            .collect()
    }

    /// An isolated runtime for one run of `agent_id` in `room_id` (spec §4.4
    /// item 1) whose history is the room's model-visible messages selected
    /// as whole turns within the agent's budget (spec §5.2): silent check-in
    /// pairs and revised drafts are hidden, the session summary covers what
    /// it summarizes and joins the context as data, and the current `input`
    /// and the reply are reserved first. Every room's copy starts at a user
    /// message, so it never opens mid-turn (final fix wave A2): providers
    /// reject a tool result whose call is missing. The report names what was
    /// left out, including pruned turns no summary covers (audit I5). The
    /// canonical transcript is only read, never written; the run base counts
    /// the selected copy, so run deltas and commits see exactly what the run
    /// appends.
    pub(crate) fn build_run_runtime(
        &self,
        agent_id: &str,
        room_id: &str,
        input: &Content,
    ) -> Option<RunBuild> {
        let canonical = self.agents.get(agent_id)?;
        let history = self.model_visible_history(agent_id, room_id);
        let session = self
            .sessions
            .get(agent_id, &crate::sessions::session_id_for_room(room_id));
        let summary = session
            .and_then(|session| session.summary.as_ref())
            .map(|summary| ContextSummary {
                text: summary.text.clone(),
                through_message_id: summary.through_message_id.clone(),
            });
        let estimator = TokenEstimator::new(
            session
                .and_then(|session| session.context_calibration_permille)
                .map_or(1.0, |permille| f64::from(permille) / 1000.0),
        );
        let budget = ContextBudget::for_config(canonical.config());
        let selection = select_context(
            &history,
            summary.as_ref(),
            budget.history_tokens(estimator.text_tokens(&input.text)),
            &estimator,
        );
        let raw = TokenEstimator::default();
        let raw_estimate_tokens = selection
            .messages
            .iter()
            .map(TokenEstimator::raw_message_tokens)
            .sum::<u64>()
            + raw.text_tokens(&input.text)
            + summary
                .as_ref()
                .map_or(0, |summary| raw.text_tokens(&summary.text));
        let pruned = session.and_then(|session| uncovered_pruned_through(session, &history));
        let context = RunContextReport {
            budget_tokens: budget.budget_tokens,
            raw_estimate_tokens,
            trimmed_through: newest_left_out(selection.dropped.last(), pruned),
            dropped: selection.dropped,
        };
        let mut runtime = AgentRuntime::from_snapshot(
            canonical.run_snapshot(selection.messages),
            Arc::clone(&self.model_adapter),
        );
        self.wire_runtime(&mut runtime);
        if let Some(summary) = summary {
            let mut providers = crate::components::default_providers(Arc::clone(&self.memory));
            providers
                .push(Arc::new(SessionSummaryProvider { text: summary.text }) as Arc<dyn Provider>);
            runtime.set_providers(providers);
        }
        let base = runtime.run_base();
        Some(RunBuild {
            runtime,
            tools: self.tool_execution_context(),
            base,
            context,
        })
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
            // The steers the run took in are in its transcript now (spec §4.7).
            record.commit_steers(&crate::agent_runs::steers_taken_in(
                &change_set.delta.messages,
            ));
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
    /// the text its run streamed, and so does an agent deleted mid-run, whose
    /// commit `commit_run` discards. Clients saw that text only as
    /// `step.delta` events, never as `message.created`, and the run's
    /// `run.failed` event (`commit_rejected`, `commit_failed`, or
    /// `agent_deleted`) tells them to drop it.
    ///
    /// Returns the `interrupted` runs the failed run's steers became in this
    /// same change (Task 9 fix round 1): the caller saves them with it and
    /// announces them once saved.
    pub(crate) fn rollback_run(
        &mut self,
        change_set: &RunChangeSet,
        error: RunError,
    ) -> Vec<crate::runs::RunRecord> {
        if let (Some(runtime), Some(undo)) = (
            self.agents.get_mut(&change_set.agent_id),
            change_set.undo.clone(),
        ) {
            runtime.revert_run_delta(&change_set.delta, undo);
        }
        if let Some(record) = self.runs.get_mut(&change_set.run_id) {
            record.reply_message_id = None;
            // The steers its transcript took in did not stand with it.
            record.revert_steers(&crate::agent_runs::steers_taken_in(
                &change_set.delta.messages,
            ));
            record.finish(RunStatus::Failed, Some(error), now_millis());
        }
        if let Some(undo) = change_set.session_undo.clone() {
            self.sessions.revert_commit(undo);
        }
        self.runs
            .offer_steers_of_failed_run(&change_set.run_id, now_millis())
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

/// A draft an evaluator sent back (spec §4.5: kept, marked `revised`).
fn is_revised_draft(message: &Message) -> bool {
    message
        .content
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get(REVISED_METADATA_KEY))
        == Some(&DataValue::Bool(true))
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
        let input = Content {
            text: text.into(),
            ..Content::default()
        };
        let build = state
            .build_run_runtime(agent_id, room_id, &input)
            .expect("agent exists");
        let (mut runtime, base) = (build.runtime, build.base);
        let history = runtime.messages().to_vec();
        let result = runtime
            .run_in_room_with_context(room_id.into(), history, input)
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

        let other_room = state
            .build_run_runtime(&agent_id, "room-b", &Content::default())
            .unwrap()
            .runtime;
        assert!(
            other_room.messages().is_empty(),
            "a room starts without other rooms' history"
        );
        let same_room = state
            .build_run_runtime(&agent_id, "room-a", &Content::default())
            .unwrap()
            .runtime;
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
    /// the run machinery: `build_run_runtime` only reads `self.agents`.
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

    /// The agent's budget with a 10-token reply reserve.
    fn set_budget(state: &mut DaemonState, agent_id: &str, budget: f64) {
        let mut config = state.agents[agent_id].config().clone();
        let settings = config.settings.get_or_insert_with(AgentSettings::default);
        settings.max_tokens = Some(10);
        settings
            .additional
            .insert("contextBudgetTokens".into(), DataValue::Number(budget));
        state.restore_agent_config(agent_id, config);
    }

    /// Replaces the M2 interim schedule-room guard (ruling test 1/2): silent
    /// check-in pairs never reach the model, and the newest spoken turns that
    /// fit the budget do. Each spoken turn is 21 tokens ("Check status" 11 +
    /// "Reply NN" 10); "next" is 9 and the reserve 10, so 124 leaves 105: five
    /// turns.
    #[test]
    fn a_run_context_hides_silent_checkins_and_keeps_the_newest_turns_that_fit() {
        let (mut state, agent_id) = state_with_agent();
        set_budget(&mut state, &agent_id, 124.0);
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

        let build = state
            .build_run_runtime(
                &agent_id,
                &room,
                &Content {
                    text: "next".into(),
                    ..Content::default()
                },
            )
            .unwrap();

        let expected: HashSet<String> = (21..30)
            .step_by(2)
            .flat_map(|tick| [format!("user-{tick}"), format!("assistant-{tick}")])
            .collect();
        let actual: HashSet<String> = build
            .runtime
            .messages()
            .iter()
            .map(|message| message.id.clone())
            .collect();
        assert_eq!(actual, expected, "the newest five spoken turns fit");
        assert!(build
            .runtime
            .messages()
            .iter()
            .all(|message| message.content.text.trim() != "CHECKIN_OK"));
        assert_eq!(
            build.context.trimmed_through.as_deref(),
            Some("assistant-19")
        );
        assert_eq!(build.context.dropped.len(), 20, "ten spoken turns left out");
        assert_eq!(build.context.budget_tokens, 124);
        assert_eq!(
            build.context.raw_estimate_tokens,
            5 * 21 + 9,
            "the kept turns and the current message, without calibration"
        );
    }

    /// Ruling test 2/2: a tool-call turn is kept or dropped whole. The
    /// newest turn is 19 tokens, the tool turn 42, the oldest 20.
    #[test]
    fn a_tool_call_turn_is_kept_or_dropped_whole() {
        let (mut state, agent_id) = state_with_agent();
        let room = crate::sessions::schedule_room_id("schedule-2");
        seed_history(
            &mut state,
            &agent_id,
            vec![
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
                history_message(&agent_id, &room, "user-2", MessageRole::User, "tick", false),
                history_message(
                    &agent_id,
                    &room,
                    "assistant-2",
                    MessageRole::Assistant,
                    "reply",
                    false,
                ),
            ],
        );
        let input = Content {
            text: "next".into(),
            ..Content::default()
        };
        let ids = |state: &DaemonState| -> Vec<String> {
            state
                .build_run_runtime(&agent_id, &room, &input)
                .unwrap()
                .runtime
                .messages()
                .iter()
                .map(|message| message.id.clone())
                .collect()
        };

        // 49 for history: the newest turn fits, the tool turn does not.
        set_budget(&mut state, &agent_id, 68.0);
        assert_eq!(ids(&state), ["user-2", "assistant-2"]);

        // 75 for history: the tool turn fits whole, the oldest turn does not.
        set_budget(&mut state, &agent_id, 94.0);
        assert_eq!(
            ids(&state),
            [
                "user-1",
                "assistant-1-call",
                "tool-1-result",
                "assistant-1-final",
                "user-2",
                "assistant-2"
            ]
        );
    }

    /// Task 11 fix round 1: a `maxTokens` at or above the budget no longer
    /// leaves the run without history. Without a provider the budget is the
    /// 32,000-token fallback, so the reserve is half of it and the history
    /// keeps the other 16,000 tokens.
    #[test]
    fn a_max_tokens_above_the_budget_still_leaves_room_for_history() {
        let (mut state, agent_id) = state_with_agent();
        let mut config = state.agents[&agent_id].config().clone();
        config
            .settings
            .get_or_insert_with(AgentSettings::default)
            .max_tokens = Some(64_000);
        state.restore_agent_config(&agent_id, config);
        seed_history(
            &mut state,
            &agent_id,
            vec![
                history_message(
                    &agent_id,
                    "chat:a",
                    "user-1",
                    MessageRole::User,
                    "hi",
                    false,
                ),
                history_message(
                    &agent_id,
                    "chat:a",
                    "assistant-1",
                    MessageRole::Assistant,
                    "hello",
                    false,
                ),
            ],
        );

        let build = state
            .build_run_runtime(&agent_id, "chat:a", &Content::default())
            .unwrap();

        assert_eq!(build.context.budget_tokens, 32_000);
        assert_eq!(build.runtime.messages().len(), 2, "the history is sent");
        assert_eq!(build.context.trimmed_through, None);
    }

    /// Controller ruling (M3 pre-flight audit M17): a draft an evaluator
    /// sent back (`revised: true`) stays in the transcript, since nothing
    /// streamed is retracted, but never reaches a later run's model; nor
    /// does the evaluator's request right after it (fix round 1), which
    /// points at an answer the model can no longer see.
    #[test]
    fn a_run_context_leaves_out_revised_drafts_and_their_revision_requests() {
        let (mut state, agent_id) = state_with_agent();
        let mut draft = history_message(
            &agent_id,
            "chat:a",
            "draft",
            MessageRole::Assistant,
            "first try",
            false,
        );
        draft.content.metadata = Some(BTreeMap::from([(
            anima_core::REVISED_METADATA_KEY.to_string(),
            DataValue::Bool(true),
        )]));
        seed_history(
            &mut state,
            &agent_id,
            vec![
                history_message(&agent_id, "chat:a", "user-1", MessageRole::User, "hi", false),
                draft,
                history_message(
                    &agent_id,
                    "chat:a",
                    "feedback",
                    MessageRole::System,
                    "Evaluator requested a revision: be brief\nRevise your previous answer and try again.",
                    false,
                ),
                history_message(
                    &agent_id,
                    "chat:a",
                    "final",
                    MessageRole::Assistant,
                    "hello",
                    false,
                ),
                history_message(
                    &agent_id,
                    "chat:a",
                    "note",
                    MessageRole::System,
                    "an ordinary system note",
                    false,
                ),
            ],
        );
        let ids = |messages: &[Message]| -> Vec<String> {
            messages.iter().map(|message| message.id.clone()).collect()
        };

        assert_eq!(
            ids(&state.model_visible_history(&agent_id, "chat:a")),
            ["user-1", "final", "note"],
            "a system message after a kept answer stays"
        );
        let build = state
            .build_run_runtime(&agent_id, "chat:a", &Content::default())
            .unwrap();
        assert_eq!(ids(build.runtime.messages()), ["user-1", "final", "note"]);
        assert_eq!(
            state.get_agent(&agent_id).unwrap().messages.len(),
            5,
            "the canonical transcript keeps the draft and the request"
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
