//! Stop requests (spec §4.6): what a stop changes, applied under the
//! control-plane transaction and saved before any run is signalled.

use std::collections::HashSet;

use super::DaemonState;
use crate::runs::{RunError, RunRecord, RunStatus, RunStopRequest, RUN_STOPPED, STOPPED_BY_OWNER};

/// Everything a stop changed, as it was, so a failed save can put it back.
#[derive(Debug, Default)]
pub(crate) struct RunStopUndo {
    pub(crate) runs: Vec<RunRecord>,
}

impl RunStopUndo {
    /// Nothing changed, so nothing needs saving (a repeated stop).
    pub(crate) fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }
}

/// A planned stop: saved first, then every control in `signal` is cancelled.
#[derive(Debug)]
pub(crate) struct RunStopPlan {
    /// The stopped run as it is now.
    pub(crate) run: RunRecord,
    /// The run and the queued and in-flight runs it started, transitively.
    pub(crate) signal: Vec<String>,
    /// Queued runs cancelled outright, as they are now.
    pub(crate) cancelled: Vec<RunRecord>,
    pub(crate) undo: RunStopUndo,
}

impl DaemonState {
    /// Plans stopping `run_id` of `agent_id` and the queued and in-flight runs
    /// it started, transitively (spec §4.6: helper runs are stopped too). A
    /// queued run is cancelled outright; a running one records the stop once.
    /// `None` when the ledger has no such run of this agent.
    pub(crate) fn request_run_stop(
        &mut self,
        agent_id: &str,
        run_id: &str,
        now_ms: u64,
    ) -> Option<RunStopPlan> {
        let record = self
            .runs
            .get(run_id)
            .filter(|record| record.agent_id == agent_id)?
            .clone();
        if record.status.is_terminal() {
            return Some(RunStopPlan {
                run: record,
                signal: Vec::new(),
                cancelled: Vec::new(),
                undo: RunStopUndo::default(),
            });
        }
        let mut targets = vec![record.id.clone()];
        let mut seen: HashSet<String> = targets.iter().cloned().collect();
        let mut index = 0;
        while index < targets.len() {
            let parent = targets[index].clone();
            for child in self.runs.active_records() {
                if child.parent_run_id.as_deref() == Some(parent.as_str())
                    && seen.insert(child.id.clone())
                {
                    targets.push(child.id.clone());
                }
            }
            index += 1;
        }
        let mut plan = RunStopPlan {
            run: record,
            signal: targets.clone(),
            cancelled: Vec::new(),
            undo: RunStopUndo::default(),
        };
        for id in &targets {
            let Some(target) = self.runs.get_mut(id) else {
                continue;
            };
            match target.status {
                RunStatus::Queued => {
                    plan.undo.runs.push(target.clone());
                    target.stop = Some(RunStopRequest {
                        requested_at_ms: now_ms,
                    });
                    target.finish(
                        RunStatus::Cancelled,
                        Some(RunError::new(RUN_STOPPED, STOPPED_BY_OWNER)),
                        now_ms,
                    );
                    plan.cancelled.push(target.clone());
                }
                RunStatus::Running | RunStatus::AwaitingApproval if target.stop.is_none() => {
                    plan.undo.runs.push(target.clone());
                    target.stop = Some(RunStopRequest {
                        requested_at_ms: now_ms,
                    });
                }
                _ => {}
            }
        }
        if let Some(current) = self.runs.get(run_id).cloned() {
            plan.run = self.with_live_tools(current);
        }
        Some(plan)
    }

    /// Puts back what a stop changed after its save failed. A queued run is
    /// restored whole; a running one only loses the stop request, keeping
    /// anything its run recorded meanwhile.
    pub(crate) fn revert_run_stop(&mut self, undo: RunStopUndo) {
        for previous in undo.runs {
            match self.runs.get_mut(&previous.id) {
                Some(current) if previous.status == RunStatus::Queued => *current = previous,
                Some(current) => current.stop = previous.stop,
                None => {}
            }
        }
    }
}
