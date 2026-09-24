//! Coordinator runs: the durable run ledger (spec §4.1, §4.8).

mod ledger;

#[allow(unused_imports)] // Later M1 tasks consume the remaining names.
pub(crate) use ledger::{
    RunError, RunInput, RunLedger, RunRecord, RunSource, RunStart, RunStatus, RunStepUsage,
    RunStopRequest, AGENT_DELETED, COMMIT_FAILED, COMMIT_REJECTED, MAX_RUN_INPUT_TEXT_BYTES,
    MAX_RUN_TOOLS_STARTED, MAX_TERMINAL_RUNS_PER_AGENT, RESTART_BEFORE_START, RESTART_DURING_RUN,
    RUN_ABORTED, RUN_FAILED, TERMINAL_RUN_RETENTION_MS,
};

/// The run that started another run (spec §3.2 parent fields, §4.1 `parentRunId`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RunLink {
    pub(crate) run_id: String,
    pub(crate) session_id: String,
    pub(crate) agent_id: String,
}

use anima_core::{
    Content, DataValue, MessageRole, RuntimeRunDelta, RuntimeRunUndo, TaskResult, TaskStatus,
    TokenUsage,
};

/// Exactly what one run added to its agent, so a commit can merge it and a
/// rollback can remove it without touching other rooms' turns (spec §4.4).
#[derive(Clone, Debug)]
pub(crate) struct RunChangeSet {
    pub(crate) run_id: String,
    pub(crate) agent_id: String,
    pub(crate) session_id: String,
    pub(crate) message_ids: Vec<String>,
    pub(crate) event_ids: Vec<String>,
    pub(crate) token_delta: TokenUsage,
    pub(crate) step_delta: u64,
    pub(crate) delta: RuntimeRunDelta,
    /// Set by `DaemonState::commit_run`; read by `rollback_run`.
    pub(crate) undo: Option<RuntimeRunUndo>,
    /// Set by `DaemonState::commit_run`; read by `rollback_run`.
    pub(crate) session_undo: Option<crate::sessions::SessionCommitUndo>,
}

impl RunChangeSet {
    pub(crate) fn new(
        run_id: String,
        agent_id: String,
        session_id: String,
        delta: RuntimeRunDelta,
    ) -> Self {
        Self {
            message_ids: delta
                .messages
                .iter()
                .map(|message| message.id.clone())
                .collect(),
            event_ids: delta.events.iter().map(|event| event.id.clone()).collect(),
            token_delta: delta.token_usage.clone(),
            step_delta: delta.step_count,
            run_id,
            agent_id,
            session_id,
            delta,
            undo: None,
            session_undo: None,
        }
    }

    /// The run's final assistant reply in its own room, when the run succeeded.
    pub(crate) fn reply_message_id(&self, result: &TaskResult<Content>) -> Option<String> {
        if result.status != TaskStatus::Success {
            return None;
        }
        self.delta
            .messages
            .last()
            .filter(|message| {
                message.role == MessageRole::Assistant && message.room_id == self.session_id
            })
            .map(|message| message.id.clone())
    }

    /// Distinct tools the run asked for, in first-use order (spec §4.1).
    pub(crate) fn tools_started(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for message in self
            .delta
            .messages
            .iter()
            .filter(|message| message.role == MessageRole::Assistant)
        {
            let Some(DataValue::Array(calls)) = message
                .content
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("toolCalls"))
            else {
                continue;
            };
            for call in calls {
                let DataValue::Object(call) = call else {
                    continue;
                };
                let Some(DataValue::String(name)) = call.get("name") else {
                    continue;
                };
                if names.len() < MAX_RUN_TOOLS_STARTED && !names.iter().any(|known| known == name) {
                    names.push(name.clone());
                }
            }
        }
        names
    }
}

/// What a source's commit hook receives (spec §4.4 item 3). Hooks use
/// `reply_message_id` instead of scanning the transcript.
#[derive(Clone, Debug)]
pub(crate) struct RunOutcome {
    pub(crate) run_id: String,
    pub(crate) session_id: String,
    pub(crate) reply_message_id: Option<String>,
    pub(crate) result: TaskResult<Content>,
    pub(crate) status: RunStatus,
}

impl RunOutcome {
    pub(crate) fn new(change_set: &RunChangeSet, result: TaskResult<Content>) -> Self {
        let status = if result.status == TaskStatus::Success {
            RunStatus::Completed
        } else {
            RunStatus::Failed
        };
        Self {
            run_id: change_set.run_id.clone(),
            session_id: change_set.session_id.clone(),
            reply_message_id: change_set.reply_message_id(&result),
            result,
            status,
        }
    }

    /// The ledger error for a failed run.
    pub(crate) fn error(&self) -> Option<RunError> {
        (self.status == RunStatus::Failed).then(|| {
            RunError::new(
                RUN_FAILED,
                self.result
                    .error
                    .clone()
                    .unwrap_or_else(|| "run failed".to_string()),
            )
        })
    }
}

/// Concurrent runs of one agent across different rooms unless
/// `ANIMAOS_RS_MAX_RUNS_PER_AGENT` says otherwise (spec §4.3, §16).
pub(crate) const DEFAULT_MAX_RUNS_PER_AGENT: usize = 3;
/// Generated helpers run one task at a time (spec §4.3).
pub(crate) const HELPER_MAX_RUNS: usize = 1;

#[cfg(test)]
mod tests {
    use super::*;
    use anima_core::{AgentStatus, Message};
    use std::collections::BTreeMap;

    fn message(
        id: &str,
        role: MessageRole,
        metadata: Option<BTreeMap<String, DataValue>>,
    ) -> Message {
        Message {
            id: id.into(),
            agent_id: "agent-1".into(),
            room_id: "room-a".into(),
            content: Content {
                text: id.into(),
                metadata,
                ..Content::default()
            },
            role,
            created_at_ms: 1,
        }
    }

    fn tool_call(name: &str) -> DataValue {
        DataValue::Object(BTreeMap::from([(
            "name".to_string(),
            DataValue::String(name.into()),
        )]))
    }

    #[test]
    fn outcome_reports_the_final_reply_only_when_the_run_succeeded() {
        let calls = BTreeMap::from([(
            "toolCalls".to_string(),
            DataValue::Array(vec![
                tool_call("bash"),
                tool_call("bash"),
                tool_call("read_file"),
            ]),
        )]);
        let change_set = RunChangeSet::new(
            "run_1".into(),
            "agent-1".into(),
            "room-a".into(),
            RuntimeRunDelta {
                messages: vec![
                    message("user", MessageRole::User, None),
                    message("calls", MessageRole::Assistant, Some(calls)),
                    message("tool", MessageRole::Tool, None),
                    message("reply", MessageRole::Assistant, None),
                ],
                events: vec![],
                event_total: 0,
                token_usage: TokenUsage {
                    prompt_tokens: 3,
                    completion_tokens: 4,
                    total_tokens: 7,
                    ..TokenUsage::default()
                },
                step_count: 1,
                last_task: None,
                status: AgentStatus::Completed,
            },
        );

        assert_eq!(change_set.message_ids, ["user", "calls", "tool", "reply"]);
        assert_eq!(change_set.token_delta.total_tokens, 7);
        assert_eq!(change_set.step_delta, 1);
        assert_eq!(change_set.tools_started(), ["bash", "read_file"]);

        let success = RunOutcome::new(&change_set, TaskResult::success(Content::default(), 1));
        assert_eq!(success.run_id, "run_1");
        assert_eq!(success.session_id, "room-a");
        assert_eq!(success.reply_message_id.as_deref(), Some("reply"));
        assert_eq!(success.status, RunStatus::Completed);
        assert_eq!(success.error(), None);

        let failure = RunOutcome::new(&change_set, TaskResult::error("model unavailable", 1));
        assert_eq!(failure.reply_message_id, None);
        assert_eq!(failure.status, RunStatus::Failed);
        assert_eq!(
            failure.error(),
            Some(RunError::new(RUN_FAILED, "model unavailable"))
        );
    }

    #[test]
    fn tools_started_is_capped_at_the_limit_in_first_use_order() {
        let names: Vec<String> = (0..51).map(|index| format!("tool-{index}")).collect();
        let calls = BTreeMap::from([(
            "toolCalls".to_string(),
            DataValue::Array(names.iter().map(|name| tool_call(name)).collect()),
        )]);
        let change_set = RunChangeSet::new(
            "run_2".into(),
            "agent-1".into(),
            "room-a".into(),
            RuntimeRunDelta {
                messages: vec![message("calls", MessageRole::Assistant, Some(calls))],
                events: vec![],
                event_total: 0,
                token_usage: TokenUsage::default(),
                step_count: 1,
                last_task: None,
                status: AgentStatus::Completed,
            },
        );

        let started = change_set.tools_started();
        assert_eq!(started.len(), MAX_RUN_TOOLS_STARTED);
        assert_eq!(started, names[..MAX_RUN_TOOLS_STARTED].to_vec());
    }
}
