//! Per-run isolation and exact change-set commit/rollback (spec §4.4 items 1–5).

use std::sync::Arc;

use anima_core::primitives::now_millis;
use anima_core::{AgentRuntime, AgentRuntimeSnapshot, AgentStatus, RuntimeRunBase};

use super::DaemonState;
use crate::runs::{RunChangeSet, RunError, RunOutcome, RunStatus, AGENT_DELETED};
use crate::tools::ToolExecutionContext;

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
    /// canonical state and counters with only that room's history, the
    /// standard providers, evaluators, and database, and no Running→Failed
    /// restore conversion. The canonical runtime is not touched.
    pub(crate) fn build_run_runtime(
        &self,
        agent_id: &str,
        room_id: &str,
    ) -> Option<(AgentRuntime, ToolExecutionContext, RuntimeRunBase)> {
        let canonical = self.agents.get(agent_id)?;
        let history = canonical
            .messages()
            .iter()
            .filter(|message| message.room_id == room_id)
            .cloned()
            .collect();
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
        if let Some(record) = self.runs.get_mut(&change_set.run_id) {
            record.usage = change_set.token_delta.clone();
            record.tools_started = change_set.tools_started();
            record.finish(outcome.status, outcome.error(), now_ms);
        }
        self.runs.prune(now_ms);
        true
    }

    /// Removes exactly one committed run's messages, events, usage, and steps
    /// and records why its commit did not stand.
    pub(crate) fn rollback_run(&mut self, change_set: &RunChangeSet, error: RunError) {
        if let (Some(runtime), Some(undo)) = (
            self.agents.get_mut(&change_set.agent_id),
            change_set.undo.clone(),
        ) {
            runtime.revert_run_delta(&change_set.delta, undo);
        }
        if let Some(record) = self.runs.get_mut(&change_set.run_id) {
            record.finish(RunStatus::Failed, Some(error), now_millis());
        }
    }

    /// Reports `Running` while any run of the agent is in flight (spec §4.4 item 5).
    pub(super) fn with_derived_status(
        &self,
        mut snapshot: AgentRuntimeSnapshot,
    ) -> AgentRuntimeSnapshot {
        if self.in_flight_runs(&snapshot.state.id) > 0 {
            snapshot.state.status = AgentStatus::Running;
        }
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use anima_core::{
        AgentConfig, AgentSettings, AgentStatus, Content, MessageRole, ModelAdapter,
        ModelGenerateRequest, ModelGenerateResponse, ModelStopReason, TokenUsage,
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

    fn state_with_agent() -> (DaemonState, String) {
        let mut state = DaemonState::with_model_adapter(Arc::new(FixedUsageModel));
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
}
