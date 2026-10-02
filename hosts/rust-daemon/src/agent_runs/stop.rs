//! Stopping runs (spec §4.6) and refusing messages to an agent that is being
//! deleted (spec §4.2's 409).

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use anima_core::primitives::now_millis;

use super::AgentRunCoordinator;
use crate::live::run_status_event;
use crate::routes::ApiError;
use crate::runs::RunRecord;

pub(crate) const AGENT_BEING_DELETED: &str = "This companion is being deleted";

/// Agents with a deletion in progress, counted.
pub(super) type DeletingAgents = Arc<StdMutex<HashMap<String, usize>>>;

/// Marks an agent as being deleted until dropped.
pub(crate) struct AgentDeletionGuard {
    agent_id: String,
    deleting: DeletingAgents,
}

impl Drop for AgentDeletionGuard {
    fn drop(&mut self) {
        let mut deleting = self
            .deleting
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(count) = deleting.get_mut(&self.agent_id) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                deleting.remove(&self.agent_id);
            }
        }
    }
}

impl AgentRunCoordinator {
    pub(crate) fn begin_agent_deletion(&self, agent_id: &str) -> AgentDeletionGuard {
        *self
            .deleting_agents
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(agent_id.to_string())
            .or_insert(0) += 1;
        AgentDeletionGuard {
            agent_id: agent_id.to_string(),
            deleting: Arc::clone(&self.deleting_agents),
        }
    }

    pub(crate) fn is_being_deleted(&self, agent_id: &str) -> bool {
        self.deleting_agents
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains_key(agent_id)
    }

    /// Stops a run (spec §4.6): the stop is saved, then the run's control and
    /// those of the runs it started are cancelled. A queued run is cancelled
    /// outright and announced. Stopping a finished run changes nothing.
    pub(crate) async fn stop_run(
        &self,
        agent_id: &str,
        run_id: &str,
    ) -> Result<RunRecord, ApiError> {
        // The stop runs to its end (saved, announced, woken, and signalled, or
        // taken back) even if whoever asked stops waiting: otherwise the
        // approvals it settled could stay settled in memory, unsaved, with
        // their waiting calls never woken.
        let coordinator = self.clone();
        let (agent_id, run_id) = (agent_id.to_string(), run_id.to_string());
        let operation =
            tokio::spawn(async move { coordinator.stop_run_to_the_end(&agent_id, &run_id).await });
        match operation.await {
            Ok(stopped) => stopped,
            Err(error) => Err(ApiError::service_unavailable(format!(
                "run stop failed: {error}"
            ))),
        }
    }

    async fn stop_run_to_the_end(
        &self,
        agent_id: &str,
        run_id: &str,
    ) -> Result<RunRecord, ApiError> {
        let transaction = self.control_plane_transaction().await;
        let planned = {
            let mut guard = self.state.write().await;
            if !guard.agents.contains_key(agent_id) {
                return Err(ApiError::not_found());
            }
            match guard.request_run_stop(agent_id, run_id, now_millis()) {
                Some(plan) => {
                    let persist =
                        (!plan.undo.is_empty()).then(|| guard.control_plane_persist_request());
                    Some((plan, persist))
                }
                None => None,
            }
        };
        let Some((plan, persist)) = planned else {
            drop(transaction);
            // Not in the ledger: a finished run only the history store keeps.
            let history = self.state.read().await.history.clone();
            return match history.store().get_run(run_id).await {
                Ok(Some(record)) if record.agent_id == agent_id => Ok(record),
                Ok(_) => Err(ApiError::not_found()),
                Err(error) => Err(ApiError::service_unavailable(error.message())),
            };
        };
        if let Some(persist) = persist {
            // Durable before any signal (spec §4.6). Nothing acts on the
            // unsaved stop meanwhile: a session queue checks a run under this
            // transaction before starting or dropping it.
            if let Err(error) = persist.save().await {
                self.state.write().await.revert_run_stop(plan.undo);
                return Err(ApiError::service_unavailable(error.to_string()));
            }
        }
        {
            let guard = self.state.read().await;
            // Settled with the stop (spec §4.6): the streams hear it, then
            // each waiting call, before any run is signalled.
            for approval in &plan.approvals {
                guard.publish_approval(approval);
            }
            for approval in &plan.approvals {
                self.approval_waiters.wake(approval);
            }
            for id in &plan.signal {
                if let Some(control) = guard.live.runs().control(id) {
                    control.cancel.cancel();
                }
            }
            for record in &plan.cancelled {
                // A cancelled queued run never starts: its control goes too.
                guard.live.runs().remove(&record.id);
                let parent = guard.live_parent_agent(&record.agent_id, &record.session_id);
                guard
                    .live
                    .publish(run_status_event(record), parent.as_deref());
            }
        }
        drop(transaction);
        Ok(plan.run)
    }
}
