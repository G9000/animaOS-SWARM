//! Stopping runs (spec §4.6) and refusing messages to an agent that is being
//! deleted (spec §4.2's 409).

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use anima_core::primitives::now_millis;
use anima_core::{Content, DataValue};
use tracing::warn;

use super::queue::{metadata_text, ACCEPTED_AT_METADATA_KEY, CLIENT_REQUEST_ID_METADATA_KEY};
use super::AgentRunCoordinator;
use crate::live::run_status_event;
use crate::routes::ApiError;
use crate::runs::{
    RunError, RunRecord, RunSource, RunStart, RunStatus, STOPPED_BEFORE_START,
    STOPPED_BEFORE_START_MESSAGE,
};

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

/// When a steer was accepted, from its metadata.
fn accepted_at_ms(content: &Content) -> Option<u64> {
    match content.metadata.as_ref()?.get(ACCEPTED_AT_METADATA_KEY)? {
        DataValue::Number(at) if at.is_finite() && *at >= 0.0 => Some(*at as u64),
        _ => None,
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

    /// Makes the steers a stopped run never read `interrupted` runs of its
    /// session, which the owner can send again, with one save. An explicit
    /// stop means stop everything, so they are not requeued (controller
    /// ruling, M3 pre-flight audit M8: a deliberate deviation from spec
    /// §4.7, which requeues a finished run's leftover steers).
    pub(super) async fn interrupt_steers(
        &self,
        agent_id: &str,
        session_id: &str,
        steers: Vec<Content>,
    ) {
        let transaction = self.control_plane_transaction().await;
        let (records, parent, hub, persist) = {
            let mut guard = self.state.write().await;
            let Some(runtime) = guard.agents.get(agent_id) else {
                return;
            };
            let model = runtime.config().model.clone();
            let provider = runtime.config().provider.clone();
            let now_ms = now_millis();
            let records = steers
                .into_iter()
                .map(|content| {
                    let accepted_at_ms = accepted_at_ms(&content).unwrap_or(now_ms);
                    let idempotency_key =
                        metadata_text(&content, CLIENT_REQUEST_ID_METADATA_KEY).map(str::to_string);
                    let mut record = RunRecord::queued(
                        RunStart {
                            agent_id: agent_id.to_string(),
                            session_id: session_id.to_string(),
                            source: RunSource::Web,
                            source_ref: None,
                            idempotency_key,
                            text: content.text,
                            model: model.clone(),
                            provider: provider.clone(),
                            parent_run_id: None,
                        },
                        accepted_at_ms,
                    );
                    record.finish(
                        RunStatus::Interrupted,
                        Some(RunError::new(
                            STOPPED_BEFORE_START,
                            STOPPED_BEFORE_START_MESSAGE,
                        )),
                        now_ms,
                    );
                    record
                })
                .collect::<Vec<_>>();
            for record in &records {
                guard.runs.insert(record.clone());
            }
            let parent = guard.live_parent_agent(agent_id, session_id);
            (
                records,
                parent,
                guard.live.clone(),
                guard.control_plane_persist_request(),
            )
        };
        if let Err(error) = persist.save().await {
            // Kept in memory; the next save persists them.
            warn!(agent_id = %agent_id, session_id = %session_id, error = %error, "could not save the steers a stopped run never read");
        }
        drop(transaction);
        for record in &records {
            hub.publish(run_status_event(record), parent.as_deref());
        }
    }
}
