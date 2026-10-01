//! The gate in `ToolExecutionContext::execute_tool` (spec §7.3): judges each
//! call, and for one that needs the owner saves a request and waits for its
//! decision, the run's stop, or the timeout, whichever settles it first. The
//! waiting call holds no lock; every settlement takes the control-plane
//! transaction, so exactly one of them wins.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, MutexGuard};
use std::time::Duration;

use anima_core::{
    tool_not_configured_error, AgentState, CancelSignal, Content, TaskResult, ToolCall,
    CANCELLED_TOOL_RESULT,
};
use tokio::sync::oneshot;

use super::{
    risk_class, ApprovalRequest, ApprovalStatus, RiskClass, Verdict, APPROVAL_LOST,
    APPROVAL_TIMEOUT_MS, DENIED_BY_OWNER, DENIED_BY_POLICY, HELPER_NEEDS_APPROVAL,
    TELEGRAM_APPROVAL_TIMEOUT_MS,
};
use crate::agent_runs::{is_helper_config, AgentRunCoordinator};
use crate::runs::{RunLink, RunSource};
use crate::state::{ApprovalAsk, Settlement};

/// How long a call waits for the owner (spec §7.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ApprovalTimeouts {
    pub(crate) default: Duration,
    /// For Telegram-started runs: the connector handles one message at a time.
    pub(crate) telegram: Duration,
}

impl Default for ApprovalTimeouts {
    fn default() -> Self {
        Self {
            default: Duration::from_millis(APPROVAL_TIMEOUT_MS),
            telegram: Duration::from_millis(TELEGRAM_APPROVAL_TIMEOUT_MS),
        }
    }
}

impl ApprovalTimeouts {
    pub(crate) fn for_source(self, source: RunSource) -> Duration {
        if source == RunSource::Telegram {
            self.telegram
        } else {
            self.default
        }
    }
}

type Senders = HashMap<String, oneshot::Sender<ApprovalRequest>>;

/// The calls waiting for the owner, by approval id. Whoever settles an
/// approval sends the settled record to its one waiter. A leaf lock, never
/// held across `.await` or while taking the state lock.
#[derive(Clone, Default)]
pub(crate) struct ApprovalWaiters {
    senders: Arc<StdMutex<Senders>>,
}

impl ApprovalWaiters {
    fn senders(&self) -> MutexGuard<'_, Senders> {
        self.senders
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The wake-up of approval `id`; registered before the request is saved.
    pub(crate) fn register(&self, id: &str) -> oneshot::Receiver<ApprovalRequest> {
        let (sender, receiver) = oneshot::channel();
        self.senders().insert(id.to_string(), sender);
        receiver
    }

    pub(crate) fn forget(&self, id: &str) {
        self.senders().remove(id);
    }

    /// Sends the settled record to its waiter, once; a later call finds none.
    pub(crate) fn wake(&self, approval: &ApprovalRequest) {
        let sender = self.senders().remove(&approval.id);
        if let Some(sender) = sender {
            let _ = sender.send(approval.clone());
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.senders().len()
    }
}

/// A saved and announced request, and the wake-up its settlement sends.
pub(crate) struct PendingApproval {
    pub(crate) id: String,
    pub(crate) woken: oneshot::Receiver<ApprovalRequest>,
}

/// A denial's tool result: `Denied by owner: <note>`, or `Denied by owner`.
pub(crate) fn denial_text(note: Option<&str>) -> String {
    match note {
        Some(note) => format!("{DENIED_BY_OWNER}: {note}"),
        None => DENIED_BY_OWNER.to_string(),
    }
}

/// Whether an approved call lost its permission while it waited: the agent
/// is gone, or its own configuration listed the tool when it asked and no
/// longer does. A tool its configuration never listed was added for the run
/// (the peer and team tools); the run's live checks judge that one.
fn lost_during_wait(configured_then: Option<bool>, configured_now: Option<bool>) -> bool {
    match configured_now {
        None => true,
        Some(now) => configured_then == Some(true) && !now,
    }
}

/// What `execute_tool` does with a call once the gate has judged it.
pub(crate) enum GateOutcome {
    /// Allowed without asking: dispatch.
    Proceed,
    /// The owner allowed it after a wait: check again, then dispatch.
    Approved,
    /// Answer the model with this instead of running the tool.
    Refuse(TaskResult<Content>),
}

/// One coordinator run's gate (spec §7.3).
#[derive(Clone)]
pub(crate) struct ApprovalGate {
    coordinator: AgentRunCoordinator,
    run: RunLink,
    source: RunSource,
    cancel: CancelSignal,
}

impl ApprovalGate {
    pub(crate) fn new(
        coordinator: AgentRunCoordinator,
        run: RunLink,
        source: RunSource,
        cancel: CancelSignal,
    ) -> Self {
        Self {
            coordinator,
            run,
            source,
            cancel,
        }
    }

    pub(crate) async fn check(&self, agent: &AgentState, call: &ToolCall) -> GateOutcome {
        // Read-class tools never ask (spec §7.2), so they never wait for the
        // state lock either: a read tool runs while the lock is held (M3).
        if risk_class(&call.name) == RiskClass::Read {
            return GateOutcome::Proceed;
        }
        match self
            .coordinator
            .approval_verdict(agent, &self.run.session_id, call)
            .await
        {
            Verdict::Allow => GateOutcome::Proceed,
            Verdict::Deny => GateOutcome::Refuse(TaskResult::error(DENIED_BY_POLICY, 0)),
            // Helpers never wait (spec §7.3).
            Verdict::Ask if is_helper_config(&agent.config) => {
                GateOutcome::Refuse(TaskResult::error(HELPER_NEEDS_APPROVAL, 0))
            }
            Verdict::Ask => self.ask(agent, call).await,
        }
    }

    async fn ask(&self, agent: &AgentState, call: &ToolCall) -> GateOutcome {
        let timeout = self.coordinator.approval_timeout(self.source);
        // Taken before the request is made, so the wait ends when the
        // record's `expiresAtMs` says it does.
        let deadline = tokio::time::Instant::now() + timeout;
        let ask = ApprovalAsk {
            run: self.run.clone(),
            call: call.clone(),
            timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
        };
        // What the agent's own configuration said when it asked, for the
        // check after an approval.
        let configured = self
            .coordinator
            .configured_tool(&agent.id, &call.name)
            .await;
        let pending = match self.coordinator.open_approval(ask).await {
            Ok(pending) => pending,
            Err(refused) => return GateOutcome::Refuse(refused),
        };
        let outcome = match self.wait(pending, deadline).await {
            Some(approval) => self.outcome(&approval),
            None => GateOutcome::Refuse(TaskResult::error(APPROVAL_LOST, 0)),
        };
        // The owner may have changed the agent during the wait: what runs is
        // what the agent allows now, not what it allowed when it asked.
        if matches!(outcome, GateOutcome::Approved)
            && lost_during_wait(
                configured,
                self.coordinator
                    .configured_tool(&agent.id, &call.name)
                    .await,
            )
        {
            return GateOutcome::Refuse(TaskResult::error(
                tool_not_configured_error(&call.name),
                0,
            ));
        }
        outcome
    }

    /// The approval as it was settled, by whoever settled it first. Holds no
    /// lock while it waits.
    async fn wait(
        &self,
        pending: PendingApproval,
        deadline: tokio::time::Instant,
    ) -> Option<ApprovalRequest> {
        let PendingApproval { id, mut woken } = pending;
        let mut wake_open = true;
        let settled = loop {
            tokio::select! {
                biased;
                settled = &mut woken, if wake_open => match settled {
                    Ok(approval) => break Some(approval),
                    // Dropped unsent: no wake-up is coming, but that is not a
                    // timeout. The stop or the deadline still settles it.
                    Err(_) => wake_open = false,
                },
                () = self.cancel.cancelled() => break None,
                () = tokio::time::sleep_until(deadline) => break None,
            }
        };
        if settled.is_some() {
            return settled;
        }
        let settlement = if self.cancel.is_cancelled() {
            Settlement::Stopped
        } else {
            Settlement::TimedOut
        };
        let settled = match self.coordinator.settle_approval(&id, settlement).await {
            Some(approval) => Some(approval),
            // Settled first by someone whose record already left the control
            // plane; that settlement sent it here before it let go of the
            // transaction this settle waited for.
            None => woken.try_recv().ok(),
        };
        // Whoever won has woken (and so removed) this waiter, or nobody will.
        self.coordinator.forget_approval_waiter(&id);
        settled
    }

    fn outcome(&self, approval: &ApprovalRequest) -> GateOutcome {
        match approval.status {
            ApprovalStatus::Allowed if !self.cancel.is_cancelled() => GateOutcome::Approved,
            ApprovalStatus::Denied => GateOutcome::Refuse(TaskResult::error(
                denial_text(
                    approval
                        .resolution
                        .as_ref()
                        .and_then(|resolution| resolution.note.as_deref()),
                ),
                0,
            )),
            // Allowed, but the run is being stopped: nothing new starts.
            ApprovalStatus::Allowed | ApprovalStatus::Stopped => {
                GateOutcome::Refuse(TaskResult::error(CANCELLED_TOOL_RESULT, 0))
            }
            ApprovalStatus::Pending | ApprovalStatus::Expired => {
                GateOutcome::Refuse(TaskResult::error(APPROVAL_LOST, 0))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use anima_core::ToolCall;

    use super::*;
    use crate::approvals::PendingApprovalStart;

    fn approval(id: &str) -> ApprovalRequest {
        let call = ToolCall {
            id: "call-1".into(),
            name: "memory_add".into(),
            args: BTreeMap::new(),
        };
        let mut approval = ApprovalRequest::pending(
            PendingApprovalStart {
                agent_id: "agent",
                session_id: "chat:a",
                run_id: "run-1",
                call: &call,
                timeout_ms: 1_000,
            },
            1,
        );
        approval.id = id.into();
        approval
    }

    #[test]
    fn telegram_runs_wait_fifteen_minutes_and_every_other_source_thirty() {
        let timeouts = ApprovalTimeouts::default();
        assert_eq!(
            timeouts.for_source(RunSource::Telegram),
            Duration::from_secs(15 * 60)
        );
        assert_eq!(
            timeouts.for_source(RunSource::Web),
            Duration::from_secs(30 * 60)
        );
        assert_eq!(
            timeouts.for_source(RunSource::Schedule),
            Duration::from_secs(30 * 60)
        );
    }

    #[test]
    fn a_wake_reaches_its_waiter_once_and_an_unknown_id_is_a_no_op() {
        let waiters = ApprovalWaiters::default();
        let mut woken = waiters.register("apr_1");
        assert_eq!(waiters.len(), 1);

        waiters.wake(&approval("apr_other"));
        assert!(woken.try_recv().is_err(), "another id wakes nothing");
        assert_eq!(waiters.len(), 1);

        waiters.wake(&approval("apr_1"));
        assert_eq!(woken.try_recv().unwrap().id, "apr_1");
        assert_eq!(waiters.len(), 0);

        waiters.wake(&approval("apr_1"));
        assert_eq!(waiters.len(), 0, "a second wake finds no waiter");

        let mut forgotten = waiters.register("apr_2");
        waiters.forget("apr_2");
        waiters.wake(&approval("apr_2"));
        assert!(forgotten.try_recv().is_err());
    }

    #[test]
    fn an_approval_is_lost_with_the_agent_or_with_a_configured_tool_only() {
        assert!(lost_during_wait(Some(true), None), "the agent is gone");
        assert!(lost_during_wait(Some(false), None), "the agent is gone");
        assert!(
            lost_during_wait(Some(true), Some(false)),
            "the tool was removed"
        );
        assert!(!lost_during_wait(Some(true), Some(true)));
        assert!(
            !lost_during_wait(Some(false), Some(false)),
            "a tool added for the run"
        );
        assert!(!lost_during_wait(Some(false), Some(true)));
    }

    #[test]
    fn a_denial_reads_denied_by_owner_with_the_note_when_there_is_one() {
        assert_eq!(denial_text(None), "Denied by owner");
        assert_eq!(denial_text(Some("not now")), "Denied by owner: not now");
    }
}
