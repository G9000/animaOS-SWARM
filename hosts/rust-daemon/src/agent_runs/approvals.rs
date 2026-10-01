//! The coordinator's side of approvals (spec §7.3): asking, settling, and
//! the owner's decision, each under the control-plane transaction, saved
//! before the waiting call is woken and before anything is announced.

use std::time::Duration;

use anima_core::primitives::now_millis;
use anima_core::{AgentState, Content, TaskResult, ToolCall};
use tracing::warn;

use super::AgentRunCoordinator;
use crate::approvals::{
    ApprovalRequest, PendingApproval, Verdict, APPROVAL_ALREADY_RESOLVED, APPROVAL_NOT_SAVED,
    APPROVAL_REVISION_STALE,
};
use crate::routes::ApiError;
use crate::runs::RunSource;
use crate::state::{ApprovalAsk, OwnerDecision, SettleRefusal, Settlement};

/// A decided approval: the same owner decision again is answered with it
/// (spec §7.3, idempotent); anything else conflicts.
#[allow(dead_code)] // M4 Task 8's decision route calls `decide_approval`.
fn replay_or_conflict(
    record: ApprovalRequest,
    decision: &OwnerDecision,
) -> Result<ApprovalRequest, ApiError> {
    if record.was_decided_as(decision.kind) {
        Ok(record)
    } else {
        Err(ApiError::conflict(APPROVAL_ALREADY_RESOLVED))
    }
}

impl AgentRunCoordinator {
    pub(crate) async fn approval_verdict(
        &self,
        agent: &AgentState,
        session_id: &str,
        call: &ToolCall,
    ) -> Verdict {
        self.state
            .read()
            .await
            .approval_verdict(agent, session_id, call)
    }

    /// Whether `agent_id` still exists and may use `tool` as it is configured
    /// now. An approval can take minutes, and the owner may change or remove
    /// the agent meanwhile.
    pub(crate) async fn agent_still_allows(&self, agent_id: &str, tool: &str) -> bool {
        self.state
            .read()
            .await
            .agents
            .get(agent_id)
            .is_some_and(|runtime| runtime.config().allows_tool(tool))
    }

    /// How long a run from `source` waits for the owner (spec §7.3).
    pub(crate) fn approval_timeout(&self, source: RunSource) -> Duration {
        self.approval_timeouts.for_source(source)
    }

    /// Drops approval `id`'s wake-up if nobody sent it; a waiting call does
    /// this once it is done waiting.
    pub(crate) fn forget_approval_waiter(&self, id: &str) {
        self.approval_waiters.forget(id);
    }

    /// Saves a request for `ask`, then announces it: `run.awaiting_approval`
    /// when the run moved there, then `approval.requested` (spec §7.3). A
    /// request that cannot be saved is taken back and the tool does not run.
    pub(crate) async fn open_approval(
        &self,
        ask: ApprovalAsk,
    ) -> Result<PendingApproval, TaskResult<Content>> {
        let transaction = self.control_plane_transaction().await;
        let (opened, persist) = {
            let mut guard = self.state.write().await;
            let opened = guard
                .open_approval(&ask, now_millis())
                .map_err(|refusal| refusal.result())?;
            (opened, guard.control_plane_persist_request())
        };
        // Registered before the request is durable or announced, so a
        // decision always finds its waiter.
        let woken = self.approval_waiters.register(&opened.approval.id);
        if let Err(error) = persist.save().await {
            warn!(approval_id = %opened.approval.id, error = %error, "could not save an approval request; the tool does not run");
            self.approval_waiters.forget(&opened.approval.id);
            self.state.write().await.revert_open_approval(&opened);
            drop(transaction);
            return Err(TaskResult::error(APPROVAL_NOT_SAVED, 0));
        }
        {
            let guard = self.state.read().await;
            if let Some(run) = &opened.run {
                guard.publish_run_status(run);
            }
            guard.publish_approval(&opened.approval);
        }
        drop(transaction);
        Ok(PendingApproval {
            id: opened.approval.id,
            woken,
        })
    }

    /// Settles `id` as timed out or stopped unless something settled it
    /// first, and returns the record as it stands; `None` once it has left
    /// the control plane. An unsaved timeout or stop is kept: it only keeps
    /// the call from running, and a restart before the next save expires
    /// the request anyway (spec §4.8).
    pub(crate) async fn settle_approval(
        &self,
        id: &str,
        settlement: Settlement,
    ) -> Option<ApprovalRequest> {
        let transaction = self.control_plane_transaction().await;
        let (settled, persist) = {
            let mut guard = self.state.write().await;
            match guard.settle_approval(id, settlement, now_millis()) {
                Ok(settled) => (settled, guard.control_plane_persist_request()),
                Err(SettleRefusal::Resolved(record)) => return Some(record),
                Err(_) => return None,
            }
        };
        if let Err(error) = persist.save().await {
            warn!(approval_id = %id, error = %error, "could not save a timed-out or stopped approval; the next save keeps it");
        }
        // Announced before any waiter goes on, so a stream hears the
        // resolution before the tool's own events.
        self.state.read().await.publish_settled(&settled);
        self.approval_waiters.wake(&settled.approval);
        drop(transaction);
        Some(settled.approval)
    }

    /// The owner's decision (spec §7.3): saved before its waiter is woken; a
    /// failed save changes nothing (503). The same decision again answers
    /// with the record, from the history store once it moved there.
    #[allow(dead_code)] // M4 Task 8's decision route calls it.
    pub(crate) async fn decide_approval(
        &self,
        id: &str,
        decision: OwnerDecision,
    ) -> Result<ApprovalRequest, ApiError> {
        // The decision runs to its end (saved, announced, and woken, or taken
        // back) even if whoever asked stops waiting, e.g. an HTTP client that
        // disconnects mid-save; otherwise it could stay applied in memory,
        // unsaved, unannounced, and unwoken.
        let coordinator = self.clone();
        let id = id.to_string();
        let operation =
            tokio::spawn(
                async move { coordinator.decide_approval_to_the_end(&id, decision).await },
            );
        match operation.await {
            Ok(decided) => decided,
            Err(error) => Err(ApiError::service_unavailable(format!(
                "approval decision failed: {error}"
            ))),
        }
    }

    #[allow(dead_code)] // M4 Task 8's decision route calls `decide_approval`.
    async fn decide_approval_to_the_end(
        &self,
        id: &str,
        decision: OwnerDecision,
    ) -> Result<ApprovalRequest, ApiError> {
        let transaction = self.control_plane_transaction().await;
        let outcome = {
            let mut guard = self.state.write().await;
            match guard.settle_approval(id, Settlement::Owner(decision.clone()), now_millis()) {
                Ok(settled) => Ok((settled, guard.control_plane_persist_request())),
                Err(refusal) => Err(refusal),
            }
        };
        let (settled, persist) = match outcome {
            Ok(settled) => settled,
            Err(SettleRefusal::Resolved(record)) => return replay_or_conflict(record, &decision),
            Err(SettleRefusal::Stale) => return Err(ApiError::conflict(APPROVAL_REVISION_STALE)),
            Err(SettleRefusal::Invalid(message)) => {
                return Err(ApiError::bad_request_static(message))
            }
            Err(SettleRefusal::Conflict(message)) => return Err(ApiError::conflict(message)),
            Err(SettleRefusal::NotFound) => {
                drop(transaction);
                // Moved to the history store, or never known. Read outside
                // the state lock (M2 lock rule).
                let history = self.state.read().await.history.clone();
                return match history.store().get_approval(id).await {
                    Ok(Some(record)) => replay_or_conflict(record, &decision),
                    Ok(None) => Err(ApiError::not_found()),
                    Err(error) => Err(ApiError::service_unavailable(error.message())),
                };
            }
        };
        if let Err(error) = persist.save().await {
            warn!(approval_id = %id, error = %error, "could not save the owner's decision; it is taken back");
            self.state
                .write()
                .await
                .revert_settled_approval(settled.undo);
            return Err(ApiError::service_unavailable(error.to_string()));
        }
        // Durable: the streams hear it first, then the waiting call goes on,
        // so `approval.resolved` precedes the tool's own events.
        self.state.read().await.publish_settled(&settled);
        self.approval_waiters.wake(&settled.approval);
        drop(transaction);
        Ok(settled.approval)
    }
}
