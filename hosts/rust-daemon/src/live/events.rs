//! The events of an agent's stream (spec §6) and their JSON.

use anima_core::primitives::now_millis;
use serde_json::{json, Value};

use super::registry::LiveRunView;
use super::MAX_PREVIEW_BYTES;
use crate::routes::RunResponse;
use crate::runs::{RunRecord, RunStatus};

#[allow(dead_code)] // M3 Tasks 6–12 publish these; until then tests build some of them.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum LiveEventBody {
    SessionCreated,
    SessionUpdated,
    SessionDeleted,
    RunQueued(RunRecord),
    RunStarted(RunRecord),
    RunAwaitingApproval(RunRecord),
    RunProgress {
        phase: &'static str,
    },
    RunSteered {
        message_id: String,
        text: String,
    },
    RunCompleted(RunRecord),
    RunFailed(RunRecord),
    RunCancelled(RunRecord),
    RunInterrupted(RunRecord),
    StepDelta {
        step_id: String,
        /// UTF-16 offset of `text` within its step, so a client joining
        /// mid-step (after a snapshot) drops what it already has.
        offset: u64,
        text: String,
    },
    MessageCreated {
        message_id: String,
        role: &'static str,
        step_id: Option<String>,
    },
    ToolStarted {
        step_id: String,
        tool_call_id: String,
        name: String,
        arguments_preview: String,
        arguments_truncated: bool,
    },
    ToolFinished {
        step_id: String,
        tool_call_id: String,
        name: String,
        status: &'static str,
        duration_ms: u64,
        result_preview: String,
        truncated: bool,
        recovered: bool,
    },
}

impl LiveEventBody {
    pub(crate) const fn type_name(&self) -> &'static str {
        match self {
            Self::SessionCreated => "session.created",
            Self::SessionUpdated => "session.updated",
            Self::SessionDeleted => "session.deleted",
            Self::RunQueued(_) => "run.queued",
            Self::RunStarted(_) => "run.started",
            Self::RunAwaitingApproval(_) => "run.awaiting_approval",
            Self::RunProgress { .. } => "run.progress",
            Self::RunSteered { .. } => "run.steered",
            Self::RunCompleted(_) => "run.completed",
            Self::RunFailed(_) => "run.failed",
            Self::RunCancelled(_) => "run.cancelled",
            Self::RunInterrupted(_) => "run.interrupted",
            Self::StepDelta { .. } => "step.delta",
            Self::MessageCreated { .. } => "message.created",
            Self::ToolStarted { .. } => "tool.started",
            Self::ToolFinished { .. } => "tool.finished",
        }
    }
}

/// One event, before a stream numbers it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LiveEvent {
    pub(crate) agent_id: String,
    pub(crate) session_id: Option<String>,
    pub(crate) run_id: Option<String>,
    pub(crate) at_ms: u64,
    pub(crate) body: LiveEventBody,
}

#[allow(dead_code)] // M3 Task 6's coordinator builds events; until then only tests do.
impl LiveEvent {
    pub(crate) fn new(agent_id: &str, body: LiveEventBody) -> Self {
        Self {
            agent_id: agent_id.to_string(),
            session_id: None,
            run_id: None,
            at_ms: now_millis(),
            body,
        }
    }

    pub(crate) fn session(mut self, session_id: &str) -> Self {
        self.session_id = Some(session_id.to_string());
        self
    }

    pub(crate) fn run(mut self, run_id: &str) -> Self {
        self.run_id = Some(run_id.to_string());
        self
    }

    /// An event about `record`'s run, in its agent and session.
    pub(crate) fn for_run(record: &RunRecord, body: LiveEventBody) -> Self {
        Self::new(&record.agent_id, body)
            .session(&record.session_id)
            .run(&record.id)
    }

    /// `type`, `agentId`, `sessionId`, `runId`, `seq`, `at`, and the body's fields.
    pub(crate) fn to_json(&self, seq: u64) -> Value {
        let mut value = json!({
            "type": self.body.type_name(),
            "agentId": self.agent_id,
            "seq": seq,
            "at": self.at_ms,
        });
        if let Some(session_id) = &self.session_id {
            value["sessionId"] = json!(session_id);
        }
        if let Some(run_id) = &self.run_id {
            value["runId"] = json!(run_id);
        }
        match &self.body {
            LiveEventBody::SessionCreated
            | LiveEventBody::SessionUpdated
            | LiveEventBody::SessionDeleted => {}
            LiveEventBody::RunQueued(record)
            | LiveEventBody::RunStarted(record)
            | LiveEventBody::RunAwaitingApproval(record)
            | LiveEventBody::RunCompleted(record)
            | LiveEventBody::RunFailed(record)
            | LiveEventBody::RunCancelled(record)
            | LiveEventBody::RunInterrupted(record) => {
                value["run"] = run_json(record);
            }
            LiveEventBody::RunProgress { phase } => value["phase"] = json!(phase),
            LiveEventBody::RunSteered { message_id, text } => {
                value["messageId"] = json!(message_id);
                value["text"] = json!(text);
            }
            LiveEventBody::StepDelta {
                step_id,
                offset,
                text,
            } => {
                value["stepId"] = json!(step_id);
                value["offset"] = json!(offset);
                value["text"] = json!(text);
            }
            LiveEventBody::MessageCreated {
                message_id,
                role,
                step_id,
            } => {
                value["messageId"] = json!(message_id);
                value["role"] = json!(role);
                value["stepId"] = json!(step_id);
            }
            LiveEventBody::ToolStarted {
                step_id,
                tool_call_id,
                name,
                arguments_preview,
                arguments_truncated,
            } => {
                value["stepId"] = json!(step_id);
                value["toolCallId"] = json!(tool_call_id);
                value["name"] = json!(name);
                value["argumentsPreview"] = json!(arguments_preview);
                value["argumentsTruncated"] = json!(arguments_truncated);
            }
            LiveEventBody::ToolFinished {
                step_id,
                tool_call_id,
                name,
                status,
                duration_ms,
                result_preview,
                truncated,
                recovered,
            } => {
                value["stepId"] = json!(step_id);
                value["toolCallId"] = json!(tool_call_id);
                value["name"] = json!(name);
                value["status"] = json!(status);
                value["durationMs"] = json!(duration_ms);
                value["resultPreview"] = json!(result_preview);
                value["truncated"] = json!(truncated);
                value["recovered"] = json!(recovered);
            }
        }
        value
    }
}

fn run_json(record: &RunRecord) -> Value {
    serde_json::to_value(RunResponse::from(record)).unwrap_or(Value::Null)
}

/// The lifecycle event for `record`'s current status.
#[allow(dead_code)] // M3 Task 6's coordinator publishes it.
pub(crate) fn run_status_event(record: &RunRecord) -> LiveEvent {
    let body = match record.status {
        RunStatus::Queued => LiveEventBody::RunQueued(record.clone()),
        RunStatus::Running => LiveEventBody::RunStarted(record.clone()),
        RunStatus::AwaitingApproval => LiveEventBody::RunAwaitingApproval(record.clone()),
        RunStatus::Completed => LiveEventBody::RunCompleted(record.clone()),
        RunStatus::Failed => LiveEventBody::RunFailed(record.clone()),
        RunStatus::Cancelled => LiveEventBody::RunCancelled(record.clone()),
        RunStatus::Interrupted => LiveEventBody::RunInterrupted(record.clone()),
    };
    LiveEvent::for_run(record, body)
}

/// An active run as a new stream first sees it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SnapshotRun {
    pub(crate) record: RunRecord,
    pub(crate) live: Option<LiveRunView>,
}

/// The first event of every stream (spec §6): the active runs with their
/// current step, text so far, and tool cards, and the pending approvals
/// (none before M4).
pub(crate) fn snapshot_json(agent_id: &str, seq: u64, runs: &[SnapshotRun]) -> Value {
    let runs = runs
        .iter()
        .map(|run| {
            let live = run.live.clone().unwrap_or_default();
            json!({
                "run": run_json(&run.record),
                "stepId": live.step_id,
                "text": live.text,
                "textOffset": live.text_offset,
                "tools": live.tools,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "type": "stream.snapshot",
        "agentId": agent_id,
        "seq": seq,
        "at": now_millis(),
        "runs": runs,
        "approvals": [],
    })
}

/// Tells a lagging stream that `missed` events were dropped; the client
/// refetches what it shows (spec §6: no replay buffer).
pub(crate) fn resync_json(agent_id: &str, seq: u64, missed: u64) -> Value {
    json!({
        "type": "stream.resync",
        "agentId": agent_id,
        "seq": seq,
        "at": now_millis(),
        "missed": missed,
    })
}

/// `text` cut to `MAX_PREVIEW_BYTES` on a char boundary, and whether it was cut.
#[allow(dead_code)] // M3 Task 6's run observer previews tool arguments and results.
pub(crate) fn preview(text: &str) -> (String, bool) {
    if text.len() <= MAX_PREVIEW_BYTES {
        return (text.to_string(), false);
    }
    let mut end = MAX_PREVIEW_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true)
}
