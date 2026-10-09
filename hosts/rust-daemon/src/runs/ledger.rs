//! Durable run ledger (spec §4.1): one record per coordinator run, kept in the
//! control-plane snapshot, with restart recovery (spec §4.8) and retention.

use std::collections::{BTreeMap, HashMap, HashSet};

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
/// Per-model-call usage kept per run (spec §4.1 `steps`).
pub(crate) const MAX_RUN_STEPS: usize = 50;
/// Attachments per message (spec §4.1, §16).
pub(crate) const MAX_RUN_ATTACHMENTS: usize = 10;
/// A reused `Idempotency-Key` answers with its original run for 24 hours
/// (spec §4.2), within the ledger's retention: only while the ledger still
/// holds the run, so at most 24 hours and, once mirrored, among the agent's
/// newest `MAX_TERMINAL_RUNS_PER_AGENT` finished runs (audit M1).
pub(crate) const IDEMPOTENCY_WINDOW_MS: u64 = 24 * 60 * 60 * 1000;

pub(crate) const RESTART_BEFORE_START: &str = "restart_before_start";
pub(crate) const RESTART_BEFORE_START_MESSAGE: &str =
    "The daemon restarted before this run started; it is safe to send it again.";
pub(crate) const RESTART_DURING_RUN: &str = "restart_during_run";
pub(crate) const RUN_FAILED: &str = "run_failed";
pub(crate) const RUN_ABORTED: &str = "run_aborted";
pub(crate) const COMMIT_REJECTED: &str = "commit_rejected";
pub(crate) const COMMIT_FAILED: &str = "commit_failed";
pub(crate) const AGENT_DELETED: &str = "agent_deleted";
pub(crate) const RUN_STOPPED: &str = "stopped";
pub(crate) const STOPPED_BY_OWNER: &str = "Stopped by owner";
/// A steer its run never read because the owner stopped the run (controller
/// ruling, M3 pre-flight audit M8): kept as an `interrupted` run to send again.
pub(crate) const STOPPED_BEFORE_START: &str = "stopped_before_start";
pub(crate) const STOPPED_BEFORE_START_MESSAGE: &str =
    "The run was stopped before this message reached it; it is safe to send it again.";
/// A steer its run left behind when eight messages were already waiting
/// (controller ruling, M3 pre-flight audit M9): kept as an `interrupted` run to
/// send again instead of passing the queue cap.
pub(crate) const QUEUE_FULL_BEFORE_START: &str = "queue_full_before_start";
pub(crate) const QUEUE_FULL_BEFORE_START_MESSAGE: &str =
    "Eight messages were already waiting when the run this message joined ended; it is safe to send it again.";
/// A steer held by a run that failed (its result refused or unsaved, or its
/// task crashed) before its transcript was saved (Task 9 fix round 1): kept
/// as an `interrupted` run to send again.
pub(crate) const FAILED_BEFORE_START: &str = "failed_before_start";
pub(crate) const FAILED_BEFORE_START_MESSAGE: &str =
    "The run this message joined failed before reading it; send it again.";
/// A deleted agent's queued message (spec §4.4 item 6).
pub(crate) const AGENT_DELETED_BEFORE_START_MESSAGE: &str =
    "The companion was deleted before this message ran";

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

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::AwaitingApproval => "awaiting_approval",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }
}

/// What started a run (spec §4.1).
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

impl RunSource {
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

/// A persisted stop request (spec §4.6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunStopRequest {
    pub(crate) requested_at_ms: u64,
}

/// Usage of one model call (spec §4.1 `steps`), recorded by the live run observer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunStepUsage {
    pub(crate) step_id: String,
    #[serde(default)]
    pub(crate) usage: TokenUsage,
    /// When the step's usage was reported; 0 for a step saved before timing.
    #[serde(default)]
    pub(crate) at_ms: u64,
    /// How long the step took; 0 when unknown.
    #[serde(default)]
    pub(crate) duration_ms: u64,
}

/// A steer saved with the run it joined (spec §4.7; controller ruling, M3
/// pre-flight audit I3): the owner's message with its key and acceptance time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunSteer {
    pub(crate) idempotency_key: String,
    pub(crate) text: String,
    pub(crate) accepted_at_ms: u64,
}

/// A steer a run's committed transcript took in: its key, which answers a
/// retry within the idempotency window of its acceptance (spec §4.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SteeredKey {
    pub(crate) idempotency_key: String,
    pub(crate) accepted_at_ms: u64,
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
    /// The committed final reply of a completed run (spec §4.4 item 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reply_message_id: Option<String>,
    /// Steers accepted into this run that are in no committed transcript or
    /// run of their own yet, oldest first: each leaves in the save that puts
    /// it in one, and a restart offers the rest again (audit I3).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) pending_steers: Vec<RunSteer>,
    /// Keys of the steers this run's committed transcript took in, so a
    /// retried one is answered with this run (spec §4.2).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) steered_keys: Vec<SteeredKey>,
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
            reply_message_id: None,
            pending_steers: Vec::new(),
            steered_keys: Vec::new(),
            mirrored: false,
        }
    }

    /// A run accepted now that starts later (spec §4.2).
    pub(crate) fn queued(start: RunStart, now_ms: u64) -> Self {
        let mut record = Self::running(start, now_ms);
        record.status = RunStatus::Queued;
        record.started_at_ms = None;
        record
    }

    /// A queued run starts executing now, with the model it runs on.
    pub(crate) fn start(&mut self, model: String, provider: Option<String>, now_ms: u64) {
        self.status = RunStatus::Running;
        self.started_at_ms = Some(now_ms.max(self.created_at_ms));
        self.model = model;
        self.provider = provider;
        self.mirrored = false;
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

    /// The steers `taken` were committed with this run's transcript: they
    /// leave its pending steers, and their keys stay to answer a retry.
    pub(crate) fn commit_steers(&mut self, taken: &[RunSteer]) {
        if taken.is_empty() {
            return;
        }
        self.pending_steers.retain(|pending| {
            !taken
                .iter()
                .any(|steer| steer.idempotency_key == pending.idempotency_key)
        });
        for steer in taken {
            if !self
                .steered_keys
                .iter()
                .any(|steered| steered.idempotency_key == steer.idempotency_key)
            {
                self.steered_keys.push(SteeredKey {
                    idempotency_key: steer.idempotency_key.clone(),
                    accepted_at_ms: steer.accepted_at_ms,
                });
            }
        }
    }

    /// Undoes `commit_steers` for a transcript that did not stand: the
    /// steers are pending again, so nothing the owner sent is lost.
    pub(crate) fn revert_steers(&mut self, taken: &[RunSteer]) {
        if taken.is_empty() {
            return;
        }
        self.steered_keys.retain(|steered| {
            !taken
                .iter()
                .any(|steer| steer.idempotency_key == steered.idempotency_key)
        });
        for steer in taken {
            if !self
                .pending_steers
                .iter()
                .any(|pending| pending.idempotency_key == steer.idempotency_key)
            {
                self.pending_steers.push(steer.clone());
            }
        }
        self.pending_steers
            .sort_by_key(|pending| pending.accepted_at_ms);
    }

    /// Finished, and holding no steer a restart still has to hand on.
    fn is_prunable(&self) -> bool {
        self.status.is_terminal() && self.pending_steers.is_empty()
    }

    /// A run of this record's session for each steer it still holds, never
    /// started: the owner can send it again (audit I3).
    fn steers_to_send_again(&mut self) -> Vec<Self> {
        if self.pending_steers.is_empty() {
            return Vec::new();
        }
        // The history store's copy, if any, is written again without them.
        self.mirrored = false;
        std::mem::take(&mut self.pending_steers)
            .into_iter()
            .map(|steer| {
                Self::queued(
                    RunStart {
                        agent_id: self.agent_id.clone(),
                        session_id: self.session_id.clone(),
                        source: RunSource::Web,
                        source_ref: None,
                        idempotency_key: Some(steer.idempotency_key),
                        text: steer.text,
                        model: self.model.clone(),
                        provider: self.provider.clone(),
                        parent_run_id: None,
                    },
                    steer.accepted_at_ms,
                )
            })
            .collect()
    }

    fn recover_after_restart(&mut self, now_ms: u64) {
        let (code, message) = match self.status {
            RunStatus::Queued => (RESTART_BEFORE_START, RESTART_BEFORE_START_MESSAGE),
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

    /// Messages of this agent waiting for their turn (spec §4.2's queue): its
    /// `Queued` runs plus the steers its other runs hold that no transcript
    /// or run of their own has taken in yet (M3 carry-over; a steer waits as
    /// surely as a queued run does).
    pub(crate) fn queued_count(&self, agent_id: &str) -> usize {
        self.records
            .values()
            .filter(|record| record.agent_id == agent_id)
            .map(|record| {
                usize::from(record.status == RunStatus::Queued) + record.pending_steers.len()
            })
            .sum()
    }

    /// Runs the ledger holds, by status name (spec §11.3). Every status is
    /// listed, with 0 when none.
    pub(crate) fn status_counts(&self) -> BTreeMap<&'static str, usize> {
        let mut counts: BTreeMap<&'static str, usize> = [
            RunStatus::Queued,
            RunStatus::Running,
            RunStatus::AwaitingApproval,
            RunStatus::Completed,
            RunStatus::Failed,
            RunStatus::Cancelled,
            RunStatus::Interrupted,
        ]
        .into_iter()
        .map(|status| (status.as_str(), 0))
        .collect();
        for record in self.records.values() {
            *counts.entry(record.status.as_str()).or_default() += 1;
        }
        counts
    }

    /// The newest run of this agent created with `key` at or after `since_ms`.
    pub(crate) fn find_by_idempotency_key(
        &self,
        agent_id: &str,
        key: &str,
        since_ms: u64,
    ) -> Option<&RunRecord> {
        self.records
            .values()
            .filter(|record| {
                record.agent_id == agent_id
                    && record.created_at_ms >= since_ms
                    && record.idempotency_key.as_deref() == Some(key)
            })
            .max_by(|left, right| {
                left.created_at_ms
                    .cmp(&right.created_at_ms)
                    .then_with(|| left.id.cmp(&right.id))
            })
    }

    /// This session's runs, newest first.
    pub(crate) fn for_session(&self, agent_id: &str, session_id: &str) -> Vec<&RunRecord> {
        let mut records = self
            .records
            .values()
            .filter(|record| record.agent_id == agent_id && record.session_id == session_id)
            .collect::<Vec<_>>();
        records.sort_by(|left, right| {
            right
                .created_at_ms
                .cmp(&left.created_at_ms)
                .then_with(|| right.id.cmp(&left.id))
        });
        records
    }

    /// Runs of this session that are queued, running, or awaiting approval.
    pub(crate) fn active_count_for_session(&self, agent_id: &str, session_id: &str) -> usize {
        self.records
            .values()
            .filter(|record| {
                record.agent_id == agent_id
                    && record.session_id == session_id
                    && !record.status.is_terminal()
            })
            .count()
    }

    /// Every run that is queued, running, or awaiting approval.
    pub(crate) fn active_records(&self) -> Vec<&RunRecord> {
        self.records
            .values()
            .filter(|record| !record.status.is_terminal())
            .collect()
    }

    /// `(agentId, sessionId)` of every run that is queued, running, or awaiting approval.
    pub(crate) fn active_sessions(&self) -> HashSet<(String, String)> {
        self.records
            .values()
            .filter(|record| !record.status.is_terminal())
            .map(|record| (record.agent_id.clone(), record.session_id.clone()))
            .collect()
    }

    /// Removes a deleted session's terminal runs and returns them, so a failed
    /// save can put them back.
    pub(crate) fn remove_terminal_for_session(
        &mut self,
        agent_id: &str,
        session_id: &str,
    ) -> Vec<RunRecord> {
        let ids = self
            .records
            .values()
            .filter(|record| {
                record.agent_id == agent_id
                    && record.session_id == session_id
                    && record.status.is_terminal()
            })
            .map(|record| record.id.clone())
            .collect::<Vec<_>>();
        ids.into_iter()
            .filter_map(|id| self.records.remove(&id))
            .collect()
    }

    /// Removes a deleted agent's terminal runs and returns them, so a failed
    /// save can put them back. Without this, a deleted agent's terminal runs
    /// would stay in the ledger until a restart: `unmirrored_terminal` skips
    /// agents that no longer exist, so they would never be mirrored and never
    /// pruned (Controller ruling 2, M2 pre-flight audit).
    pub(crate) fn remove_terminal_for_agent(&mut self, agent_id: &str) -> Vec<RunRecord> {
        let ids = self
            .records
            .values()
            .filter(|record| record.agent_id == agent_id && record.status.is_terminal())
            .map(|record| record.id.clone())
            .collect::<Vec<_>>();
        ids.into_iter()
            .filter_map(|id| self.records.remove(&id))
            .collect()
    }

    /// Cancels a deleted agent's queued runs (spec §4.4 item 6) and returns
    /// each as it was and as it is now.
    pub(crate) fn cancel_queued_for_agent(
        &mut self,
        agent_id: &str,
        now_ms: u64,
    ) -> Vec<(RunRecord, RunRecord)> {
        self.records
            .values_mut()
            .filter(|record| record.agent_id == agent_id && record.status == RunStatus::Queued)
            .map(|record| {
                let queued = record.clone();
                record.finish(
                    RunStatus::Cancelled,
                    Some(RunError::new(
                        AGENT_DELETED,
                        AGENT_DELETED_BEFORE_START_MESSAGE,
                    )),
                    now_ms,
                );
                (queued, record.clone())
            })
            .collect()
    }

    /// The run of this agent a steer sent with `key` at or after `since_ms`
    /// joined, and the steer while that run still holds it (spec §4.2,
    /// §4.7). Only the ledger's records are read, never a transcript (audit
    /// M28).
    pub(crate) fn find_steer(
        &self,
        agent_id: &str,
        key: &str,
        since_ms: u64,
    ) -> Option<(&RunRecord, Option<&RunSteer>)> {
        self.records
            .values()
            .filter(|record| record.agent_id == agent_id)
            .find_map(|record| {
                if let Some(steer) = record
                    .pending_steers
                    .iter()
                    .find(|steer| steer.idempotency_key == key && steer.accepted_at_ms >= since_ms)
                {
                    return Some((record, Some(steer)));
                }
                record
                    .steered_keys
                    .iter()
                    .any(|steered| {
                        steered.idempotency_key == key && steered.accepted_at_ms >= since_ms
                    })
                    .then_some((record, None))
            })
    }

    /// Makes each steer a failed run still holds an `interrupted` run of its
    /// own to send again (`failed_before_start`) and returns them as
    /// inserted; a run in any other state keeps its steers.
    pub(crate) fn offer_steers_of_failed_run(
        &mut self,
        run_id: &str,
        now_ms: u64,
    ) -> Vec<RunRecord> {
        let Some(record) = self
            .records
            .get_mut(run_id)
            .filter(|record| record.status == RunStatus::Failed)
        else {
            return Vec::new();
        };
        let offered = record
            .steers_to_send_again()
            .into_iter()
            .map(|mut steer| {
                steer.finish(
                    RunStatus::Interrupted,
                    Some(RunError::new(
                        FAILED_BEFORE_START,
                        FAILED_BEFORE_START_MESSAGE,
                    )),
                    now_ms,
                );
                steer
            })
            .collect::<Vec<_>>();
        for record in &offered {
            self.insert(record.clone());
        }
        offered
    }

    pub(crate) fn has_in_flight_idempotency_key(&self, agent_id: &str, key: &str) -> bool {
        self.records.values().any(|record| {
            record.agent_id == agent_id
                && record.status.is_in_flight()
                && record.idempotency_key.as_deref() == Some(key)
        })
    }

    /// This agent's runs, oldest first, for tests; the runs routes read a
    /// session's runs with `for_session`.
    #[cfg(test)]
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
            for record in self.records.values().filter(|record| record.is_prunable()) {
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
    /// and in-flight runs become interrupted (spec §4.8), each steer a run
    /// still held becomes an interrupted run of its own (audit I3), and
    /// retention applies.
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
            for mut steer in record.steers_to_send_again() {
                steer.recover_after_restart(now_ms);
                ledger.insert(steer);
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
            assert_eq!(status.as_str(), name);
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
            assert_eq!(source.as_str(), name);
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
        assert!(
            value.get("replyMessageId").is_none(),
            "an unset reply is not written, so saved records keep their shape"
        );
        assert!(value.get("pendingSteers").is_none());
        assert!(value.get("steeredKeys").is_none());
        let mut steered = record.clone();
        steered.pending_steers = vec![RunSteer {
            idempotency_key: "key-2".into(),
            text: "and this".into(),
            accepted_at_ms: 43,
        }];
        steered.steered_keys = vec![SteeredKey {
            idempotency_key: "key-3".into(),
            accepted_at_ms: 44,
        }];
        let written = serde_json::to_value(&steered).unwrap();
        assert_eq!(
            written["pendingSteers"],
            json!([{"idempotencyKey": "key-2", "text": "and this", "acceptedAtMs": 43}])
        );
        assert_eq!(
            written["steeredKeys"],
            json!([{"idempotencyKey": "key-3", "acceptedAtMs": 44}])
        );
        assert_eq!(
            serde_json::from_value::<RunRecord>(written).unwrap(),
            steered
        );
        let mut replied = record.clone();
        replied.reply_message_id = Some("msg-1".into());
        let written = serde_json::to_value(&replied).unwrap();
        assert_eq!(written["replyMessageId"], "msg-1");
        assert_eq!(
            serde_json::from_value::<RunRecord>(written).unwrap(),
            replied
        );

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
        assert_eq!(minimal.reply_message_id, None);
        assert!(minimal.pending_steers.is_empty());
        assert!(minimal.steered_keys.is_empty());
    }

    #[test]
    fn an_old_step_without_timing_loads_with_zeros() {
        let step: RunStepUsage = serde_json::from_value(json!({
            "stepId": "run_1:1",
            "usage": { "prompt_tokens": 3, "completion_tokens": 2, "total_tokens": 5 }
        }))
        .unwrap();
        assert_eq!(step.at_ms, 0);
        assert_eq!(step.duration_ms, 0);
        assert_eq!(step.usage.total_tokens, 5);
        let written = serde_json::to_value(&step).unwrap();
        assert_eq!(written["atMs"], 0);
        assert_eq!(written["durationMs"], 0);
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

    /// Controller ruling (M3 pre-flight audit I3): each steer a run still
    /// held when the daemon stopped becomes one `interrupted` run of the
    /// run's session to send again, at its acceptance time; the run keeps
    /// none of them.
    #[test]
    fn restart_recovery_offers_the_steers_a_run_still_held_once() {
        let now = 5 * TERMINAL_RUN_RETENTION_MS;
        let steer = |key: &str, at: u64| RunSteer {
            idempotency_key: key.into(),
            text: format!("text of {key}"),
            accepted_at_ms: at,
        };
        let mut running = record("agent-a", now - 10);
        running.session_id = "chat:s".into();
        running.pending_steers = vec![steer("key-2", now - 9), steer("key-3", now - 8)];
        // A run whose result could not be saved keeps the steers it took in.
        let mut failed = mirrored("agent-a", now - 7);
        failed.status = RunStatus::Failed;
        failed.pending_steers = vec![steer("key-4", now - 6)];
        let live = HashSet::from(["agent-a".to_string()]);

        let ledger = RunLedger::restored(vec![running.clone(), failed.clone()], &live, now);

        for (key, at, session) in [
            ("key-2", now - 9, "chat:s"),
            ("key-3", now - 8, "chat:s"),
            ("key-4", now - 6, "direct:test"),
        ] {
            let offered = ledger
                .find_by_idempotency_key("agent-a", key, 0)
                .unwrap_or_else(|| panic!("{key} is offered again"));
            assert_eq!(offered.status, RunStatus::Interrupted);
            assert_eq!(offered.error.as_ref().unwrap().code, RESTART_BEFORE_START);
            assert_eq!(
                offered.error.as_ref().unwrap().message,
                RESTART_BEFORE_START_MESSAGE
            );
            assert_eq!(offered.source, RunSource::Web);
            assert_eq!(offered.session_id, session);
            assert_eq!(offered.input.text, format!("text of {key}"));
            assert_eq!(offered.created_at_ms, at);
            assert_eq!(offered.started_at_ms, None);
            assert!(!offered.mirrored);
        }
        assert!(ledger.get(&running.id).unwrap().pending_steers.is_empty());
        let failed = ledger.get(&failed.id).unwrap();
        assert!(failed.pending_steers.is_empty());
        assert!(!failed.mirrored, "the history store gets the change");
        assert_eq!(ledger.for_agent("agent-a").len(), 5);
    }

    /// Fix round 1 (Task 9 review 7a): a steer's key answers for the same
    /// 24 hours as a run's, counted from when the steer was accepted.
    #[test]
    fn steer_keys_are_found_per_agent_within_the_idempotency_window() {
        let now = 10 * IDEMPOTENCY_WINDOW_MS;
        let since = now - IDEMPOTENCY_WINDOW_MS;
        let mut ledger = RunLedger::default();
        let mut run = record("agent-a", since - 100);
        run.pending_steers = vec![
            RunSteer {
                idempotency_key: "old-pending".into(),
                text: "old".into(),
                accepted_at_ms: since - 1,
            },
            RunSteer {
                idempotency_key: "pending".into(),
                text: "new".into(),
                accepted_at_ms: since,
            },
        ];
        run.steered_keys = vec![
            SteeredKey {
                idempotency_key: "old-steered".into(),
                accepted_at_ms: since - 1,
            },
            SteeredKey {
                idempotency_key: "steered".into(),
                accepted_at_ms: now,
            },
        ];
        ledger.insert(run.clone());

        let (found, pending) = ledger.find_steer("agent-a", "pending", since).unwrap();
        assert_eq!(found.id, run.id);
        assert_eq!(pending.unwrap().text, "new");
        let (found, pending) = ledger.find_steer("agent-a", "steered", since).unwrap();
        assert_eq!(found.id, run.id);
        assert!(pending.is_none());
        for expired in ["old-pending", "old-steered"] {
            assert!(
                ledger.find_steer("agent-a", expired, since).is_none(),
                "{expired}"
            );
            assert!(
                ledger.find_steer("agent-a", expired, 0).is_some(),
                "{expired}"
            );
        }
        assert!(ledger.find_steer("agent-b", "pending", 0).is_none());
    }

    #[test]
    fn a_finished_run_still_holding_steers_is_not_pruned() {
        let now = 10 * TERMINAL_RUN_RETENTION_MS;
        let mut ledger = RunLedger::default();
        let mut holding = mirrored("agent-a", now - 2 * TERMINAL_RUN_RETENTION_MS);
        holding.pending_steers = vec![RunSteer {
            idempotency_key: "key-2".into(),
            text: "and this".into(),
            accepted_at_ms: 1,
        }];
        ledger.insert(holding.clone());

        ledger.prune(now);

        assert!(ledger.get(&holding.id).is_some());
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

    #[test]
    fn active_runs_are_counted_per_session() {
        let mut ledger = RunLedger::default();
        let running = record("agent-a", 1);
        let mut queued = record("agent-a", 2);
        queued.status = RunStatus::Queued;
        let done = finished("agent-a", 3);
        let mut elsewhere = record("agent-a", 4);
        elsewhere.session_id = "chat:other".into();
        for run in [running, queued, done, elsewhere] {
            ledger.insert(run);
        }

        assert_eq!(ledger.active_count_for_session("agent-a", "direct:test"), 2);
        assert_eq!(ledger.active_count_for_session("agent-a", "chat:other"), 1);
        assert_eq!(ledger.active_count_for_session("agent-b", "direct:test"), 0);
    }

    #[test]
    fn a_deleted_session_takes_only_its_terminal_runs() {
        let mut ledger = RunLedger::default();
        let running = record("agent-a", 1);
        let running_id = running.id.clone();
        let done = finished("agent-a", 2);
        let done_id = done.id.clone();
        let mut elsewhere = finished("agent-a", 3);
        elsewhere.session_id = "chat:other".into();
        for run in [running, done, elsewhere] {
            ledger.insert(run);
        }

        let removed = ledger.remove_terminal_for_session("agent-a", "direct:test");

        assert_eq!(
            removed
                .iter()
                .map(|run| run.id.as_str())
                .collect::<Vec<_>>(),
            [done_id.as_str()]
        );
        assert!(ledger.get(&running_id).is_some());
        assert_eq!(ledger.for_agent("agent-a").len(), 2);
    }

    #[test]
    fn a_deleted_agent_takes_only_its_terminal_runs() {
        let mut ledger = RunLedger::default();
        let running = record("agent-a", 1);
        let running_id = running.id.clone();
        let mut done = finished("agent-a", 2);
        done.session_id = "chat:one".into();
        let done_id = done.id.clone();
        let mut done_elsewhere = finished("agent-a", 3);
        done_elsewhere.session_id = "chat:two".into();
        let done_elsewhere_id = done_elsewhere.id.clone();
        let other_agent = finished("agent-b", 4);
        let other_agent_id = other_agent.id.clone();
        for run in [running, done, done_elsewhere, other_agent] {
            ledger.insert(run);
        }

        let mut removed = ledger
            .remove_terminal_for_agent("agent-a")
            .iter()
            .map(|run| run.id.clone())
            .collect::<Vec<_>>();
        removed.sort();
        let mut expected = [done_id, done_elsewhere_id];
        expected.sort();

        assert_eq!(removed, expected);
        assert!(
            ledger.get(&running_id).is_some(),
            "an in-flight run is never removed"
        );
        assert!(
            ledger.get(&other_agent_id).is_some(),
            "other agents are untouched"
        );
        assert!(ledger
            .for_agent("agent-a")
            .iter()
            .all(|run| run.id == running_id));
    }

    #[test]
    fn queued_runs_wait_until_started_and_count_toward_the_queue() {
        let mut ledger = RunLedger::default();
        let queued = RunRecord::queued(start("agent-1"), 10);
        assert_eq!(queued.status, RunStatus::Queued);
        assert_eq!(queued.started_at_ms, None);
        ledger.insert(queued.clone());
        ledger.insert(record("agent-1", 11));
        ledger.insert(RunRecord::queued(start("agent-2"), 12));
        assert_eq!(ledger.queued_count("agent-1"), 1);
        assert_eq!(
            ledger.in_flight_count("agent-1"),
            1,
            "a queued run is not in flight"
        );

        let run = ledger.get_mut(&queued.id).unwrap();
        run.start("gpt-5.5".into(), Some("openai".into()), 20);
        assert_eq!(run.status, RunStatus::Running);
        assert_eq!(run.started_at_ms, Some(20));
        assert_eq!(run.model, "gpt-5.5");
        assert_eq!(ledger.queued_count("agent-1"), 0);
    }

    fn steer(key: &str) -> RunSteer {
        RunSteer {
            idempotency_key: key.into(),
            text: key.into(),
            accepted_at_ms: 1,
        }
    }

    #[test]
    fn pending_steers_count_toward_the_waiting_total() {
        let mut ledger = RunLedger::default();
        let mut running = record("agent-1", 10);
        running.pending_steers = vec![steer("a"), steer("b")];
        ledger.insert(running);
        let mut other = record("agent-2", 11);
        other.pending_steers = vec![steer("c")];
        ledger.insert(other);
        ledger.insert(RunRecord::queued(start("agent-1"), 12));
        assert_eq!(ledger.queued_count("agent-1"), 3);
        assert_eq!(ledger.queued_count("agent-2"), 1);
        assert_eq!(ledger.queued_count("agent-3"), 0);
    }

    #[test]
    fn a_queued_run_is_counted_once() {
        let mut ledger = RunLedger::default();
        ledger.insert(RunRecord::queued(start("agent-1"), 10));
        ledger.insert(RunRecord::queued(start("agent-1"), 11));
        assert_eq!(ledger.queued_count("agent-1"), 2);
    }

    #[test]
    fn status_counts_lists_every_status() {
        let mut ledger = RunLedger::default();
        ledger.insert(RunRecord::queued(start("agent-1"), 10));
        ledger.insert(record("agent-1", 11));
        ledger.insert(record("agent-2", 12));
        ledger.insert(finished("agent-1", 13));
        let counts = ledger.status_counts();
        assert_eq!(
            counts.keys().copied().collect::<Vec<_>>(),
            [
                "awaiting_approval",
                "cancelled",
                "completed",
                "failed",
                "interrupted",
                "queued",
                "running"
            ]
        );
        assert_eq!(counts["queued"], 1);
        assert_eq!(counts["running"], 2);
        assert_eq!(counts["completed"], 1);
        assert_eq!(counts["failed"], 0);
    }

    #[test]
    fn idempotency_keys_are_found_per_agent_within_the_window() {
        let mut ledger = RunLedger::default();
        let mut keyed = start("agent-1");
        keyed.idempotency_key = Some("key-1".into());
        let old = RunRecord::queued(keyed.clone(), 100);
        let newer = RunRecord::queued(keyed, 200);
        ledger.insert(old.clone());
        ledger.insert(newer.clone());

        assert_eq!(
            ledger
                .find_by_idempotency_key("agent-1", "key-1", 0)
                .map(|record| record.id.as_str()),
            Some(newer.id.as_str())
        );
        assert_eq!(
            ledger.find_by_idempotency_key("agent-1", "key-1", 201),
            None,
            "outside the window"
        );
        assert_eq!(ledger.find_by_idempotency_key("agent-2", "key-1", 0), None);
        assert_eq!(ledger.find_by_idempotency_key("agent-1", "key-2", 0), None);
        let ids: Vec<&str> = ledger
            .for_session("agent-1", "direct:test")
            .iter()
            .map(|record| record.id.as_str())
            .collect();
        assert_eq!(ids, [newer.id.as_str(), old.id.as_str()], "newest first");
    }

    /// A deleted agent's queued runs are cancelled, each returned as it was
    /// and as it is now; its running runs and other agents' runs are not
    /// touched (spec §4.4 item 6; M3 Task 7 review Minor 3).
    #[test]
    fn a_deleted_agents_queued_runs_are_cancelled_and_returned_before_and_after() {
        let mut ledger = RunLedger::default();
        let queued = RunRecord::queued(start("agent-a"), 10);
        let running = record("agent-a", 11);
        let other = RunRecord::queued(start("agent-b"), 12);
        for record in [&queued, &running, &other] {
            ledger.insert(record.clone());
        }

        let cancelled = ledger.cancel_queued_for_agent("agent-a", 20);

        assert_eq!(cancelled.len(), 1);
        let (before, after) = &cancelled[0];
        assert_eq!(before, &queued);
        assert_eq!(after.id, queued.id);
        assert_eq!(after.status, RunStatus::Cancelled);
        assert_eq!(after.finished_at_ms, Some(20));
        assert_eq!(after.started_at_ms, None);
        assert_eq!(
            after.error,
            Some(RunError::new(
                AGENT_DELETED,
                "The companion was deleted before this message ran"
            ))
        );
        assert_eq!(ledger.get(&queued.id), Some(after));
        assert_eq!(ledger.get(&running.id), Some(&running));
        assert_eq!(ledger.get(&other.id), Some(&other));
        assert!(ledger.cancel_queued_for_agent("agent-a", 30).is_empty());
    }

    /// Carry-forward (M2 T17 Minor 9): a message still waiting to start keeps
    /// its session active, so the web never declares the send unconfirmed.
    #[test]
    fn a_queued_run_alone_keeps_its_session_active() {
        let mut ledger = RunLedger::default();
        let queued = RunRecord::queued(start("agent-1"), 10);
        ledger.insert(queued);

        assert_eq!(
            ledger.active_sessions(),
            HashSet::from([("agent-1".to_string(), "direct:test".to_string())])
        );
        assert_eq!(ledger.active_count_for_session("agent-1", "direct:test"), 1);
        assert_eq!(ledger.active_records().len(), 1);
    }

    #[test]
    fn the_accepted_run_limits_match_the_spec() {
        assert_eq!(MAX_RUN_ATTACHMENTS, 10);
        assert_eq!(IDEMPOTENCY_WINDOW_MS, 24 * 60 * 60 * 1000);
        assert_eq!(RUN_STOPPED, "stopped");
        assert_eq!(STOPPED_BY_OWNER, "Stopped by owner");
    }
}
