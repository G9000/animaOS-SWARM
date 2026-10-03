//! Automation fire history (spec §9.1): one record per occurrence, saved
//! with its outcome and kept in the control plane until the history store
//! holds it (spec §13.1).

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use tracing::warn;

use super::ScheduleOutcomeStatus;

/// Fire records the control plane keeps while the history store fails; past
/// it the oldest leave with a warning.
pub(crate) const MAX_UNMIRRORED_FIRES: usize = 1_000;

/// One occurrence of an automation (spec §9.1). `id` is the occurrence's run
/// idempotency key, so writing it twice is harmless.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScheduleFireRecord {
    pub(crate) id: String,
    pub(crate) schedule_id: String,
    pub(crate) agent_id: String,
    pub(crate) fired_at_ms: u64,
    pub(crate) finished_at_ms: u64,
    pub(crate) outcome: ScheduleOutcomeStatus,
    #[serde(default)]
    pub(crate) run_id: Option<String>,
    #[serde(default)]
    pub(crate) session_id: Option<String>,
    #[serde(default)]
    pub(crate) error_code: Option<String>,
    #[serde(default)]
    pub(crate) manual: bool,
}

/// Fire records the history store does not hold yet, oldest first.
#[derive(Clone, Debug, Default)]
pub(crate) struct FireLog {
    pending: Vec<ScheduleFireRecord>,
}

/// What [`FireLog::record_undoable`] changed, so [`FireLog::undo`] puts
/// the log back exactly: the same records in the same order.
#[derive(Clone, Debug)]
pub(crate) enum FireUndo {
    /// `previous` had the fire's id and kept its place.
    Replaced { previous: ScheduleFireRecord },
    /// The fire was appended; `dropped` was the oldest, pushed out by the cap.
    Added {
        id: String,
        dropped: Option<ScheduleFireRecord>,
    },
}

impl FireLog {
    /// Adds `fire`, replacing one with its id (returned); past the cap the
    /// oldest leaves.
    #[cfg(test)]
    pub(crate) fn record(&mut self, fire: ScheduleFireRecord) -> Option<ScheduleFireRecord> {
        match self.record_undoable(fire) {
            FireUndo::Replaced { previous } => Some(previous),
            FireUndo::Added { .. } => None,
        }
    }

    /// [`FireLog::record`], returning what [`FireLog::undo`] needs.
    pub(crate) fn record_undoable(&mut self, fire: ScheduleFireRecord) -> FireUndo {
        if let Some(existing) = self.pending.iter_mut().find(|known| known.id == fire.id) {
            return FireUndo::Replaced {
                previous: std::mem::replace(existing, fire),
            };
        }
        let id = fire.id.clone();
        self.pending.push(fire);
        let mut dropped = None;
        if self.pending.len() > MAX_UNMIRRORED_FIRES {
            let oldest = self.pending.remove(0);
            warn!(
                fire_id = %oldest.id,
                "dropped the oldest automation fire record: the history store has not taken any for a while"
            );
            dropped = Some(oldest);
        }
        FireUndo::Added { id, dropped }
    }

    /// Undoes a [`FireLog::record_undoable`]: a replaced record returns to
    /// its place, an added one leaves, and one the cap pushed out returns as
    /// the oldest.
    pub(crate) fn undo(&mut self, undo: FireUndo) {
        match undo {
            FireUndo::Replaced { previous } => {
                match self
                    .pending
                    .iter_mut()
                    .find(|known| known.id == previous.id)
                {
                    Some(existing) => *existing = previous,
                    None => self.pending.push(previous),
                }
            }
            FireUndo::Added { id, dropped } => {
                self.remove(&id);
                if let Some(dropped) = dropped {
                    self.pending.insert(0, dropped);
                }
            }
        }
    }

    pub(crate) fn remove(&mut self, id: &str) -> Option<ScheduleFireRecord> {
        let index = self.pending.iter().position(|fire| fire.id == id)?;
        Some(self.pending.remove(index))
    }

    pub(crate) fn for_schedule(&self, schedule_id: &str) -> Vec<ScheduleFireRecord> {
        self.pending
            .iter()
            .filter(|fire| fire.schedule_id == schedule_id)
            .cloned()
            .collect()
    }

    /// Drops the fires of agents not in `live`; returns how many.
    pub(crate) fn retain_agents(&mut self, live: &HashSet<String>) -> usize {
        let before = self.pending.len();
        self.pending.retain(|fire| live.contains(&fire.agent_id));
        before - self.pending.len()
    }

    pub(crate) fn unmirrored(&self, limit: usize) -> Vec<ScheduleFireRecord> {
        self.pending.iter().take(limit).cloned().collect()
    }

    /// Removes each written fire still exactly as written; returns how many.
    pub(crate) fn mark_mirrored(&mut self, written: &[ScheduleFireRecord]) -> usize {
        let before = self.pending.len();
        self.pending.retain(|fire| !written.contains(fire));
        before - self.pending.len()
    }

    pub(crate) fn snapshot(&self) -> Vec<ScheduleFireRecord> {
        self.pending.clone()
    }

    /// A restored log keeps the newest `MAX_UNMIRRORED_FIRES`.
    pub(crate) fn restored(mut fires: Vec<ScheduleFireRecord>) -> Self {
        if fires.len() > MAX_UNMIRRORED_FIRES {
            fires.drain(..fires.len() - MAX_UNMIRRORED_FIRES);
        }
        Self { pending: fires }
    }

    pub(crate) fn validate(fires: &[ScheduleFireRecord]) -> Result<(), String> {
        let mut ids = HashSet::new();
        for fire in fires {
            if fire.id.trim().is_empty() || !ids.insert(fire.id.as_str()) {
                return Err(format!(
                    "duplicate or empty schedule fire id in snapshot: {}",
                    fire.id
                ));
            }
            if fire.schedule_id.trim().is_empty()
                || fire.agent_id.trim().is_empty()
                || fire.fired_at_ms == 0
                || fire.finished_at_ms < fire.fired_at_ms
            {
                return Err(format!("schedule fire '{}' is invalid", fire.id));
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.pending.len()
    }
}

/// A finished fire of automation `s1`, for tests elsewhere in the crate.
#[cfg(test)]
pub(crate) fn tests_support_fire(agent_id: &str) -> ScheduleFireRecord {
    ScheduleFireRecord {
        id: "schedule:s1:10".into(),
        schedule_id: "s1".into(),
        agent_id: agent_id.into(),
        fired_at_ms: 10,
        finished_at_ms: 15,
        outcome: ScheduleOutcomeStatus::Spoke,
        run_id: Some("run_1".into()),
        session_id: Some("schedule:s1".into()),
        error_code: None,
        manual: false,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn fire(id: &str, agent_id: &str, fired_at_ms: u64) -> ScheduleFireRecord {
        ScheduleFireRecord {
            id: id.into(),
            schedule_id: "s1".into(),
            agent_id: agent_id.into(),
            fired_at_ms,
            finished_at_ms: fired_at_ms + 5,
            outcome: ScheduleOutcomeStatus::Spoke,
            run_id: Some("run_1".into()),
            session_id: Some("schedule:s1".into()),
            error_code: None,
            manual: false,
        }
    }

    #[test]
    fn fire_records_serialize_in_camel_case() {
        assert_eq!(MAX_UNMIRRORED_FIRES, 1_000);
        assert_eq!(
            serde_json::to_value(fire("schedule:s1:10", "agent-1", 10)).unwrap(),
            serde_json::json!({
                "id": "schedule:s1:10",
                "scheduleId": "s1",
                "agentId": "agent-1",
                "firedAtMs": 10,
                "finishedAtMs": 15,
                "outcome": "spoke",
                "runId": "run_1",
                "sessionId": "schedule:s1",
                "errorCode": null,
                "manual": false
            })
        );
    }

    #[test]
    fn the_log_records_replaces_removes_and_caps() {
        let mut log = FireLog::default();
        assert_eq!(log.record(fire("a", "agent-1", 10)), None);
        let mut changed = fire("a", "agent-1", 10);
        changed.outcome = ScheduleOutcomeStatus::Failed;
        assert_eq!(log.record(changed.clone()), Some(fire("a", "agent-1", 10)));
        assert_eq!(log.len(), 1);
        assert_eq!(log.for_schedule("s1"), vec![changed]);
        assert!(log.for_schedule("other").is_empty());
        assert!(log.remove("a").is_some());
        assert_eq!(log.len(), 0);

        for n in 0..=MAX_UNMIRRORED_FIRES as u64 {
            log.record(fire(&format!("f{n}"), "agent-1", n + 1));
        }
        assert_eq!(log.len(), MAX_UNMIRRORED_FIRES);
        assert!(
            log.snapshot().iter().all(|kept| kept.id != "f0"),
            "the oldest leaves first"
        );
    }

    #[test]
    fn undo_puts_the_log_back_exactly() {
        let mut log = FireLog::default();
        for n in 0..MAX_UNMIRRORED_FIRES as u64 {
            log.record(fire(&format!("f{n}"), "agent-1", n + 1));
        }
        let full = log.snapshot();

        let mut changed = fire("f5", "agent-1", 6);
        changed.outcome = ScheduleOutcomeStatus::Failed;
        let replaced = log.record_undoable(changed);
        assert_eq!(log.len(), MAX_UNMIRRORED_FIRES);
        log.undo(replaced);
        assert_eq!(log.snapshot(), full, "the replaced record keeps its place");

        let added = log.record_undoable(fire("new", "agent-1", 5_000));
        assert!(log.snapshot().iter().all(|kept| kept.id != "f0"));
        log.undo(added);
        assert_eq!(
            log.snapshot(),
            full,
            "the record the cap pushed out returns"
        );
    }

    #[test]
    fn mirroring_removes_only_unchanged_fires_and_orphans_go() {
        let mut log = FireLog::default();
        log.record(fire("a", "agent-1", 10));
        log.record(fire("b", "agent-1", 20));
        log.record(fire("c", "agent-gone", 30));
        let written = log.unmirrored(2);
        assert_eq!(
            written
                .iter()
                .map(|fire| fire.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"],
            "oldest first"
        );
        let mut rewritten = fire("b", "agent-1", 20);
        rewritten.outcome = ScheduleOutcomeStatus::Stopped;
        log.record(rewritten);
        assert_eq!(
            log.mark_mirrored(&written),
            1,
            "b changed since it was read"
        );
        assert_eq!(log.len(), 2);

        let live = HashSet::from(["agent-1".to_string()]);
        assert_eq!(log.retain_agents(&live), 1);
        assert_eq!(
            log.snapshot()
                .iter()
                .map(|fire| fire.id.as_str())
                .collect::<Vec<_>>(),
            ["b"]
        );
    }

    #[test]
    fn restored_logs_are_validated_and_capped() {
        assert_eq!(FireLog::validate(&[fire("a", "agent-1", 10)]), Ok(()));
        let mut finished_early = fire("b", "agent-1", 10);
        finished_early.finished_at_ms = 9;
        let mut unnamed = fire("c", "agent-1", 10);
        unnamed.schedule_id = String::new();
        for bad in [
            vec![fire("a", "agent-1", 10), fire("a", "agent-1", 11)],
            vec![fire(" ", "agent-1", 10)],
            vec![fire("z", "agent-1", 0)],
            vec![finished_early],
            vec![unnamed],
        ] {
            assert!(FireLog::validate(&bad).is_err(), "{bad:?}");
        }
        let many = (0..MAX_UNMIRRORED_FIRES as u64 + 5)
            .map(|n| fire(&format!("f{n}"), "agent-1", n + 1))
            .collect::<Vec<_>>();
        assert_eq!(FireLog::restored(many).len(), MAX_UNMIRRORED_FIRES);
    }
}
