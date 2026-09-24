//! Durable run ledger (spec §4.1): one record per coordinator run, kept in the
//! control-plane snapshot, with restart recovery (spec §4.8) and retention.

use std::collections::{HashMap, HashSet};

use anima_core::TokenUsage;
use serde::{Deserialize, Serialize};

/// Terminal runs stay in the control plane for 24 hours (spec §4.1).
pub(crate) const TERMINAL_RUN_RETENTION_MS: u64 = 24 * 60 * 60 * 1000;
/// ...and at most this many per agent, newest first (spec §4.1).
pub(crate) const MAX_TERMINAL_RUNS_PER_AGENT: usize = 50;
/// Stored run input text is capped at 32 KiB (spec §4.1, §16).
pub(crate) const MAX_RUN_INPUT_TEXT_BYTES: usize = 32 * 1024;
/// Distinct tool names kept per run (spec §4.1).
pub(crate) const MAX_RUN_TOOLS_STARTED: usize = 50;

pub(crate) const RESTART_BEFORE_START: &str = "restart_before_start";
pub(crate) const RESTART_DURING_RUN: &str = "restart_during_run";
pub(crate) const RUN_FAILED: &str = "run_failed";
pub(crate) const RUN_ABORTED: &str = "run_aborted";
pub(crate) const COMMIT_REJECTED: &str = "commit_rejected";
pub(crate) const COMMIT_FAILED: &str = "commit_failed";
pub(crate) const AGENT_DELETED: &str = "agent_deleted";

/// Ledger states (spec §4.1). M1 produces `Running`, `Completed`, `Failed`,
/// and `Interrupted`; the others arrive with async runs and approvals.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RunStatus {
    Queued,
    Running,
    AwaitingApproval,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

impl RunStatus {
    pub(crate) const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }

    /// Running or awaiting approval: the agent counts as working (spec §4.4 item 5).
    pub(crate) const fn is_in_flight(self) -> bool {
        matches!(self, Self::Running | Self::AwaitingApproval)
    }
}

/// What started a run (spec §4.1). `Web` arrives with the async runs route.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RunSource {
    Web,
    Api,
    Telegram,
    Schedule,
    Job,
    Delegation,
    Peer,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunInput {
    pub(crate) text: String,
    #[serde(default)]
    pub(crate) attachment_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) skill: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunError {
    pub(crate) code: String,
    pub(crate) message: String,
}

impl RunError {
    pub(crate) fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }
}

/// A persisted stop request (spec §4.6); set from M3 on.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunStopRequest {
    pub(crate) requested_at_ms: u64,
}

/// Usage of one model call (spec §4.1 `steps`); recorded from M3's observer.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunStepUsage {
    pub(crate) step_id: String,
    #[serde(default)]
    pub(crate) usage: TokenUsage,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunRecord {
    pub(crate) id: String,
    pub(crate) agent_id: String,
    pub(crate) session_id: String,
    pub(crate) source: RunSource,
    #[serde(default)]
    pub(crate) source_ref: Option<String>,
    pub(crate) status: RunStatus,
    #[serde(default)]
    pub(crate) idempotency_key: Option<String>,
    #[serde(default)]
    pub(crate) input: RunInput,
    pub(crate) created_at_ms: u64,
    #[serde(default)]
    pub(crate) started_at_ms: Option<u64>,
    #[serde(default)]
    pub(crate) finished_at_ms: Option<u64>,
    #[serde(default)]
    pub(crate) error: Option<RunError>,
    #[serde(default)]
    pub(crate) stop: Option<RunStopRequest>,
    #[serde(default)]
    pub(crate) tools_started: Vec<String>,
    #[serde(default)]
    pub(crate) steps: Vec<RunStepUsage>,
    #[serde(default)]
    pub(crate) usage: TokenUsage,
    #[serde(default)]
    pub(crate) model: String,
    #[serde(default)]
    pub(crate) provider: Option<String>,
    #[serde(default)]
    pub(crate) parent_run_id: Option<String>,
    /// Set once the history store holds this terminal record (spec §4.1).
    #[serde(default)]
    pub(crate) mirrored: bool,
}

/// What the coordinator knows when a run starts.
#[derive(Clone, Debug)]
pub(crate) struct RunStart {
    pub(crate) agent_id: String,
    pub(crate) session_id: String,
    pub(crate) source: RunSource,
    pub(crate) source_ref: Option<String>,
    pub(crate) idempotency_key: Option<String>,
    pub(crate) text: String,
    pub(crate) model: String,
    pub(crate) provider: Option<String>,
    pub(crate) parent_run_id: Option<String>,
}

impl RunRecord {
    /// A run that starts executing now.
    pub(crate) fn running(start: RunStart, now_ms: u64) -> Self {
        Self {
            id: format!("run_{}", uuid::Uuid::new_v4()),
            agent_id: start.agent_id,
            session_id: start.session_id,
            source: start.source,
            source_ref: start.source_ref,
            status: RunStatus::Running,
            idempotency_key: start.idempotency_key,
            input: RunInput {
                text: truncate_to_bytes(&start.text, MAX_RUN_INPUT_TEXT_BYTES),
                attachment_ids: Vec::new(),
                skill: None,
            },
            created_at_ms: now_ms,
            started_at_ms: Some(now_ms),
            finished_at_ms: None,
            error: None,
            stop: None,
            tools_started: Vec::new(),
            steps: Vec::new(),
            usage: TokenUsage::default(),
            model: start.model,
            provider: start.provider,
            parent_run_id: start.parent_run_id,
            mirrored: false,
        }
    }

    /// Records a terminal status; a later call (for example a rolled-back
    /// commit) replaces an earlier one, so the history store's copy, if any,
    /// is written again.
    pub(crate) fn finish(&mut self, status: RunStatus, error: Option<RunError>, now_ms: u64) {
        self.status = status;
        self.error = error;
        self.finished_at_ms = Some(now_ms.max(self.created_at_ms));
        self.mirrored = false;
    }

    /// Notes a tool the run is starting: first-use order, no duplicates, and at
    /// most `MAX_RUN_TOOLS_STARTED` names (spec §4.1, §4.8).
    pub(crate) fn note_tool_started(&mut self, name: &str) {
        if self.tools_started.len() < MAX_RUN_TOOLS_STARTED
            && !self.tools_started.iter().any(|known| known == name)
        {
            self.tools_started.push(name.to_string());
        }
    }

    fn recover_after_restart(&mut self, now_ms: u64) {
        let (code, message) = match self.status {
            RunStatus::Queued => (
                RESTART_BEFORE_START,
                "The daemon restarted before this run started; it is safe to send it again.",
            ),
            RunStatus::Running | RunStatus::AwaitingApproval => (
                RESTART_DURING_RUN,
                "The daemon restarted while this run was in progress; tools it started may have had effects.",
            ),
            _ => return,
        };
        self.finish(
            RunStatus::Interrupted,
            Some(RunError::new(code, message)),
            now_ms,
        );
    }
}

fn truncate_to_bytes(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RunLedger {
    records: HashMap<String, RunRecord>,
}

impl RunLedger {
    pub(crate) fn insert(&mut self, record: RunRecord) {
        self.records.insert(record.id.clone(), record);
    }

    pub(crate) fn get(&self, run_id: &str) -> Option<&RunRecord> {
        self.records.get(run_id)
    }

    pub(crate) fn get_mut(&mut self, run_id: &str) -> Option<&mut RunRecord> {
        self.records.get_mut(run_id)
    }

    pub(crate) fn remove(&mut self, run_id: &str) -> Option<RunRecord> {
        self.records.remove(run_id)
    }

    pub(crate) fn in_flight_count(&self, agent_id: &str) -> usize {
        self.records
            .values()
            .filter(|record| record.agent_id == agent_id && record.status.is_in_flight())
            .count()
    }

    pub(crate) fn has_in_flight_idempotency_key(&self, agent_id: &str, key: &str) -> bool {
        self.records.values().any(|record| {
            record.agent_id == agent_id
                && record.status.is_in_flight()
                && record.idempotency_key.as_deref() == Some(key)
        })
    }

    /// This agent's runs, oldest first. Read by M3's runs routes; tests use it now.
    #[allow(dead_code)]
    pub(crate) fn for_agent(&self, agent_id: &str) -> Vec<&RunRecord> {
        let mut records = self
            .records
            .values()
            .filter(|record| record.agent_id == agent_id)
            .collect::<Vec<_>>();
        records.sort_by(|left, right| {
            left.created_at_ms
                .cmp(&right.created_at_ms)
                .then_with(|| left.id.cmp(&right.id))
        });
        records
    }

    /// Keeps every non-terminal run plus, per agent, the terminal runs from the
    /// last 24 hours up to 50; a terminal run leaves only once the history
    /// store holds it (spec §4.1).
    pub(crate) fn prune(&mut self, now_ms: u64) {
        let cutoff = now_ms.saturating_sub(TERMINAL_RUN_RETENTION_MS);
        let expired: Vec<String> = {
            let mut terminal: HashMap<&str, Vec<(u64, &str, bool)>> = HashMap::new();
            for record in self
                .records
                .values()
                .filter(|record| record.status.is_terminal())
            {
                terminal.entry(record.agent_id.as_str()).or_default().push((
                    record.finished_at_ms.unwrap_or(record.created_at_ms),
                    record.id.as_str(),
                    record.mirrored,
                ));
            }
            let mut expired = Vec::new();
            for runs in terminal.values_mut() {
                runs.sort_unstable_by(|left, right| (right.0, right.1).cmp(&(left.0, left.1)));
                for (index, (finished_at_ms, run_id, mirrored)) in runs.iter().enumerate() {
                    if *mirrored
                        && (index >= MAX_TERMINAL_RUNS_PER_AGENT || *finished_at_ms < cutoff)
                    {
                        expired.push((*run_id).to_string());
                    }
                }
            }
            expired
        };
        for run_id in expired {
            self.records.remove(&run_id);
        }
    }

    /// Terminal runs of live agents the history store does not hold yet, the
    /// `limit` oldest finished first. Only the returned records are cloned.
    pub(crate) fn unmirrored_terminal(
        &self,
        live_agents: &HashSet<String>,
        limit: usize,
    ) -> Vec<RunRecord> {
        let oldest_first = |left: &&RunRecord, right: &&RunRecord| {
            left.finished_at_ms
                .cmp(&right.finished_at_ms)
                .then_with(|| left.id.cmp(&right.id))
        };
        let mut pending = self
            .records
            .values()
            .filter(|record| {
                record.status.is_terminal()
                    && !record.mirrored
                    && live_agents.contains(&record.agent_id)
            })
            .collect::<Vec<_>>();
        if pending.len() > limit {
            pending.select_nth_unstable_by(limit, oldest_first);
            pending.truncate(limit);
        }
        pending.sort_unstable_by(oldest_first);
        pending.into_iter().cloned().collect()
    }

    /// Marks written runs mirrored, but only where the ledger still holds
    /// exactly what was written; a record changed meanwhile is written again.
    pub(crate) fn mark_mirrored(&mut self, written: &[RunRecord]) -> usize {
        let mut marked = 0;
        for run in written {
            if let Some(current) = self.records.get_mut(&run.id) {
                if !current.mirrored && *current == *run {
                    current.mirrored = true;
                    marked += 1;
                }
            }
        }
        marked
    }

    /// Records to save, sorted, without runs of agents that no longer exist.
    pub(crate) fn snapshot_records(&self, live_agents: &HashSet<String>) -> Vec<RunRecord> {
        let mut records = self
            .records
            .values()
            .filter(|record| live_agents.contains(&record.agent_id))
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by(|left, right| {
            left.agent_id
                .cmp(&right.agent_id)
                .then_with(|| left.created_at_ms.cmp(&right.created_at_ms))
                .then_with(|| left.id.cmp(&right.id))
        });
        records
    }

    pub(crate) fn validate(records: &[RunRecord]) -> Result<(), String> {
        let mut ids = HashSet::new();
        for record in records {
            if record.id.trim().is_empty() || !ids.insert(record.id.as_str()) {
                return Err(format!(
                    "duplicate or empty run id in snapshot: {}",
                    record.id
                ));
            }
            if record.agent_id.trim().is_empty() || record.session_id.trim().is_empty() {
                return Err(format!(
                    "run '{}' has an empty agent or session id",
                    record.id
                ));
            }
        }
        Ok(())
    }

    /// The ledger after a restart: runs of missing agents are dropped, queued
    /// and in-flight runs become interrupted (spec §4.8), and retention applies.
    pub(crate) fn restored(
        records: Vec<RunRecord>,
        live_agents: &HashSet<String>,
        now_ms: u64,
    ) -> Self {
        let mut ledger = Self::default();
        for mut record in records {
            if !live_agents.contains(&record.agent_id) {
                continue;
            }
            record.recover_after_restart(now_ms);
            ledger.insert(record);
        }
        ledger.prune(now_ms);
        ledger
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn start(agent_id: &str) -> RunStart {
        RunStart {
            agent_id: agent_id.into(),
            session_id: "direct:test".into(),
            source: RunSource::Api,
            source_ref: None,
            idempotency_key: None,
            text: "hello".into(),
            model: "test-model".into(),
            provider: None,
            parent_run_id: None,
        }
    }

    fn record(agent_id: &str, at_ms: u64) -> RunRecord {
        RunRecord::running(start(agent_id), at_ms)
    }

    fn finished(agent_id: &str, at_ms: u64) -> RunRecord {
        let mut record = record(agent_id, at_ms);
        record.finish(RunStatus::Completed, None, at_ms);
        record
    }

    fn mirrored(agent_id: &str, at_ms: u64) -> RunRecord {
        let mut record = finished(agent_id, at_ms);
        record.mirrored = true;
        record
    }

    #[test]
    fn run_status_and_source_use_snake_case_names() {
        for (status, name) in [
            (RunStatus::Queued, "queued"),
            (RunStatus::Running, "running"),
            (RunStatus::AwaitingApproval, "awaiting_approval"),
            (RunStatus::Completed, "completed"),
            (RunStatus::Failed, "failed"),
            (RunStatus::Cancelled, "cancelled"),
            (RunStatus::Interrupted, "interrupted"),
        ] {
            assert_eq!(serde_json::to_value(status).unwrap(), json!(name));
            assert_eq!(
                status.is_terminal(),
                matches!(name, "completed" | "failed" | "cancelled" | "interrupted"),
                "{name}"
            );
            assert_eq!(
                status.is_in_flight(),
                matches!(name, "running" | "awaiting_approval"),
                "{name}"
            );
        }
        for (source, name) in [
            (RunSource::Web, "web"),
            (RunSource::Api, "api"),
            (RunSource::Telegram, "telegram"),
            (RunSource::Schedule, "schedule"),
            (RunSource::Job, "job"),
            (RunSource::Delegation, "delegation"),
            (RunSource::Peer, "peer"),
        ] {
            assert_eq!(serde_json::to_value(source).unwrap(), json!(name));
        }
    }

    #[test]
    fn running_records_use_v4_run_ids_camel_case_and_serde_defaults() {
        let mut keyed = start("agent-a");
        keyed.idempotency_key = Some("key-1".into());
        let record = RunRecord::running(keyed, 42);
        let value = serde_json::to_value(&record).unwrap();

        let id = value["id"].as_str().unwrap();
        assert_eq!(
            uuid::Uuid::parse_str(id.strip_prefix("run_").unwrap())
                .unwrap()
                .get_version_num(),
            4
        );
        assert_eq!(value["sessionId"], "direct:test");
        assert_eq!(value["status"], "running");
        assert_eq!(value["idempotencyKey"], "key-1");
        assert_eq!(value["createdAtMs"], 42);
        assert_eq!(value["startedAtMs"], 42);
        assert_eq!(value["toolsStarted"], json!([]));
        assert_eq!(value["mirrored"], false);

        let minimal: RunRecord = serde_json::from_value(json!({
            "id": "run_legacy",
            "agentId": "agent-a",
            "sessionId": "direct:a",
            "source": "api",
            "status": "completed",
            "createdAtMs": 1
        }))
        .unwrap();
        assert!(!minimal.mirrored);
        assert!(minimal.tools_started.is_empty());
        assert!(minimal.steps.is_empty());
        assert_eq!(minimal.usage, TokenUsage::default());
        assert_eq!(minimal.input, RunInput::default());
        assert_eq!(minimal.parent_run_id, None);
    }

    #[test]
    fn run_input_text_is_truncated_on_a_char_boundary() {
        let mut long = start("agent-a");
        long.text = "é".repeat(MAX_RUN_INPUT_TEXT_BYTES);

        let record = RunRecord::running(long, 1);

        assert_eq!(record.input.text.len(), MAX_RUN_INPUT_TEXT_BYTES);
        assert!(record.input.text.chars().all(|character| character == 'é'));
    }

    #[test]
    fn noted_tools_keep_first_use_order_without_duplicates_up_to_the_limit() {
        let mut record = record("agent-a", 1);
        record.note_tool_started("bash");
        record.note_tool_started("read_file");
        record.note_tool_started("bash");
        assert_eq!(record.tools_started, ["bash", "read_file"]);

        for index in 0..MAX_RUN_TOOLS_STARTED {
            record.note_tool_started(&format!("tool-{index}"));
        }
        assert_eq!(record.tools_started.len(), MAX_RUN_TOOLS_STARTED);
        assert_eq!(record.tools_started[..2], ["bash", "read_file"]);
        assert_eq!(
            record.tools_started.last().map(String::as_str),
            Some(format!("tool-{}", MAX_RUN_TOOLS_STARTED - 3).as_str())
        );
    }

    #[test]
    fn restart_recovery_interrupts_unfinished_runs_and_keeps_their_tools() {
        let now = 5 * TERMINAL_RUN_RETENTION_MS;
        let mut running = record("agent-a", now - 10);
        running.tools_started = vec!["bash".into()];
        let mut awaiting = record("agent-a", now - 9);
        awaiting.status = RunStatus::AwaitingApproval;
        let mut queued = record("agent-a", now - 8);
        queued.status = RunStatus::Queued;
        queued.started_at_ms = None;
        let done = finished("agent-a", now - 7);
        let orphan = record("agent-gone", now - 6);
        let live = HashSet::from(["agent-a".to_string()]);

        let ledger = RunLedger::restored(
            vec![
                running.clone(),
                awaiting.clone(),
                queued.clone(),
                done.clone(),
                orphan.clone(),
            ],
            &live,
            now,
        );

        let interrupted = ledger.get(&running.id).unwrap();
        assert_eq!(interrupted.status, RunStatus::Interrupted);
        assert_eq!(interrupted.error.as_ref().unwrap().code, RESTART_DURING_RUN);
        assert_eq!(interrupted.tools_started, vec!["bash".to_string()]);
        assert_eq!(interrupted.finished_at_ms, Some(now));
        assert_eq!(
            ledger
                .get(&awaiting.id)
                .unwrap()
                .error
                .as_ref()
                .unwrap()
                .code,
            RESTART_DURING_RUN
        );
        let never_started = ledger.get(&queued.id).unwrap();
        assert_eq!(never_started.status, RunStatus::Interrupted);
        assert_eq!(
            never_started.error.as_ref().unwrap().code,
            RESTART_BEFORE_START
        );
        assert_eq!(ledger.get(&done.id), Some(&done));
        assert!(
            ledger.get(&orphan.id).is_none(),
            "runs of missing agents are dropped"
        );
        assert_eq!(ledger.in_flight_count("agent-a"), 0);
    }

    #[test]
    fn retention_keeps_in_flight_runs_and_the_newest_mirrored_terminal_runs_of_the_last_day() {
        let now = 10 * TERMINAL_RUN_RETENTION_MS;
        let mut ledger = RunLedger::default();
        let old_running = record("agent-a", now - 2 * TERMINAL_RUN_RETENTION_MS);
        ledger.insert(old_running.clone());
        let stale = mirrored("agent-a", now - TERMINAL_RUN_RETENTION_MS - 1);
        ledger.insert(stale.clone());
        let mut recent = Vec::new();
        for offset in 0..(MAX_TERMINAL_RUNS_PER_AGENT as u64 + 5) {
            let run = mirrored("agent-a", now - offset);
            recent.push(run.id.clone());
            ledger.insert(run);
        }
        let other = mirrored("agent-b", now - 10);
        ledger.insert(other.clone());

        ledger.prune(now);

        assert!(
            ledger.get(&old_running.id).is_some(),
            "in-flight runs are never pruned"
        );
        assert!(
            ledger.get(&stale.id).is_none(),
            "terminal runs older than a day are pruned"
        );
        assert!(ledger.get(&other.id).is_some(), "limits apply per agent");
        let kept = recent.iter().filter(|id| ledger.get(id).is_some()).count();
        assert_eq!(kept, MAX_TERMINAL_RUNS_PER_AGENT);
        assert!(ledger.get(&recent[0]).is_some(), "the newest run is kept");
        assert!(
            ledger.get(recent.last().unwrap()).is_none(),
            "the oldest excess run is pruned"
        );
    }

    #[test]
    fn terminal_runs_leave_the_control_plane_only_once_mirrored() {
        let now = 10 * TERMINAL_RUN_RETENTION_MS;
        let mut ledger = RunLedger::default();
        let stale = finished("agent-a", now - 2 * TERMINAL_RUN_RETENTION_MS);
        ledger.insert(stale.clone());
        let mut excess = Vec::new();
        for offset in 0..(MAX_TERMINAL_RUNS_PER_AGENT as u64 + 3) {
            let run = finished("agent-a", now - offset);
            excess.push(run.id.clone());
            ledger.insert(run);
        }

        ledger.prune(now);
        assert!(
            ledger.get(&stale.id).is_some(),
            "an old run waits for the history store"
        );
        assert!(
            excess.iter().all(|id| ledger.get(id).is_some()),
            "so do runs beyond the count limit"
        );

        ledger.get_mut(&stale.id).unwrap().mirrored = true;
        ledger.prune(now);
        assert!(ledger.get(&stale.id).is_none());
    }

    #[test]
    fn unmirrored_terminal_runs_are_listed_oldest_first_and_only_unchanged_records_are_marked() {
        let mut ledger = RunLedger::default();
        let running = record("agent-a", 1);
        let newer = finished("agent-a", 30);
        let older = finished("agent-a", 20);
        let already = mirrored("agent-a", 10);
        let orphan = finished("agent-gone", 5);
        for run in [
            running.clone(),
            newer.clone(),
            older.clone(),
            already,
            orphan,
        ] {
            ledger.insert(run);
        }
        let live = HashSet::from(["agent-a".to_string()]);

        let pending = ledger.unmirrored_terminal(&live, 10);
        assert_eq!(
            pending
                .iter()
                .map(|run| run.id.as_str())
                .collect::<Vec<_>>(),
            [older.id.as_str(), newer.id.as_str()]
        );
        assert_eq!(
            ledger
                .unmirrored_terminal(&live, 1)
                .iter()
                .map(|run| run.id.as_str())
                .collect::<Vec<_>>(),
            [older.id.as_str()],
            "a limited batch holds the oldest runs"
        );
        assert!(ledger.unmirrored_terminal(&live, 0).is_empty());

        // `newer` changes after it was read (a rolled-back commit, say).
        ledger.get_mut(&newer.id).unwrap().finish(
            RunStatus::Failed,
            Some(RunError::new(COMMIT_FAILED, "disk full")),
            31,
        );
        assert_eq!(ledger.mark_mirrored(&pending), 1);
        assert!(ledger.get(&older.id).unwrap().mirrored);
        assert!(
            !ledger.get(&newer.id).unwrap().mirrored,
            "a changed record is written again"
        );
        assert_eq!(
            ledger
                .unmirrored_terminal(&live, 10)
                .iter()
                .map(|run| run.id.as_str())
                .collect::<Vec<_>>(),
            [newer.id.as_str()]
        );
        assert!(!ledger.get(&running.id).unwrap().mirrored);
    }

    #[test]
    fn finishing_a_mirrored_run_again_marks_it_for_rewriting() {
        let mut ledger = RunLedger::default();
        let run = mirrored("agent-a", 10);
        ledger.insert(run.clone());

        // A commit the store already holds is rolled back (spec §4.4 item 4).
        ledger.get_mut(&run.id).unwrap().finish(
            RunStatus::Failed,
            Some(RunError::new(COMMIT_FAILED, "disk full")),
            11,
        );

        let record = ledger.get(&run.id).unwrap();
        assert_eq!(record.status, RunStatus::Failed);
        assert!(
            !record.mirrored,
            "the store's copy is stale, so the run is written again"
        );
    }

    #[test]
    fn in_flight_queries_ignore_queued_and_terminal_runs() {
        let mut ledger = RunLedger::default();
        let mut running = record("agent-a", 1);
        running.idempotency_key = Some("key-1".into());
        let mut queued = record("agent-a", 2);
        queued.status = RunStatus::Queued;
        queued.idempotency_key = Some("key-2".into());
        let mut done = finished("agent-a", 3);
        done.idempotency_key = Some("key-3".into());
        for run in [running, queued, done] {
            ledger.insert(run);
        }

        assert_eq!(ledger.in_flight_count("agent-a"), 1);
        assert_eq!(ledger.in_flight_count("agent-b"), 0);
        assert!(ledger.has_in_flight_idempotency_key("agent-a", "key-1"));
        assert!(!ledger.has_in_flight_idempotency_key("agent-a", "key-2"));
        assert!(!ledger.has_in_flight_idempotency_key("agent-a", "key-3"));
        assert!(!ledger.has_in_flight_idempotency_key("agent-b", "key-1"));
        assert_eq!(ledger.for_agent("agent-a").len(), 3);
    }

    #[test]
    fn snapshot_records_are_sorted_and_skip_deleted_agents() {
        let mut ledger = RunLedger::default();
        let late = record("agent-a", 20);
        let early = record("agent-a", 10);
        let other = record("agent-b", 5);
        let orphan = record("agent-gone", 1);
        for run in [late.clone(), early.clone(), other.clone(), orphan] {
            ledger.insert(run);
        }
        let live = HashSet::from(["agent-a".to_string(), "agent-b".to_string()]);

        let saved = ledger.snapshot_records(&live);

        assert_eq!(
            saved.iter().map(|run| run.id.as_str()).collect::<Vec<_>>(),
            [early.id.as_str(), late.id.as_str(), other.id.as_str()]
        );
    }

    #[test]
    fn validation_rejects_blank_and_duplicate_ids() {
        let valid = record("agent-a", 1);
        assert!(RunLedger::validate(std::slice::from_ref(&valid)).is_ok());
        assert!(RunLedger::validate(&[valid.clone(), valid]).is_err());
        let mut blank = record("agent-a", 2);
        blank.id = " ".into();
        assert!(RunLedger::validate(&[blank]).is_err());
        let mut no_session = record("agent-a", 3);
        no_session.session_id = String::new();
        assert!(RunLedger::validate(&[no_session]).is_err());
    }
}
