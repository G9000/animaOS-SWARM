//! The gate in `ToolExecutionContext::execute_tool` (spec §7.3): judges each
//! call, and for one that needs the owner saves a request and waits for its
//! decision, the run's stop, or the timeout, whichever settles it first. The
//! waiting call holds no lock; every settlement takes the control-plane
//! transaction, so exactly one of them wins.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, MutexGuard};
use std::time::Duration;

use tokio::sync::oneshot;

use super::{ApprovalRequest, APPROVAL_TIMEOUT_MS, DENIED_BY_OWNER, TELEGRAM_APPROVAL_TIMEOUT_MS};
use crate::runs::RunSource;

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
    fn a_denial_reads_denied_by_owner_with_the_note_when_there_is_one() {
        assert_eq!(denial_text(None), "Denied by owner");
        assert_eq!(denial_text(Some("not now")), "Denied by owner: not now");
    }
}
