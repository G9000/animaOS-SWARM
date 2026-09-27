//! Graceful shutdown of accepted runs (M3 final fix wave S2-A; spec §4.8).
//!
//! Once shutdown begins, a session's queue starts nothing more: its queued
//! runs stay queued, saved as nothing else, so the next start interrupts them
//! as never started (`restart_before_start`); an accepted run still waiting
//! for its room, slot, or permit gives up the wait and stays queued too; and a
//! new message is refused with 503. The accepted runs already going get a
//! bounded wait to commit, as `JobService::shutdown` finishes its admitted
//! runs. A run still going when the wait ends is logged and left to the
//! restart, which interrupts it (`restart_during_run`).

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use tokio::sync::watch;
use tracing::warn;

use super::AgentRunCoordinator;
use crate::routes::ApiError;

/// How long shutdown waits for the accepted runs already going to commit.
pub(crate) const ACCEPTED_RUN_SHUTDOWN_WAIT_MS: u64 = 30_000;

/// `accept_run`'s answer (503) once shutdown has begun.
pub(crate) const DAEMON_SHUTTING_DOWN: &str =
    "The daemon is shutting down; send the message again once it restarts";

#[derive(Debug, Default)]
struct Lifecycle {
    closing: bool,
    /// Accepted runs a session queue started whose start has not ended.
    going: HashSet<String>,
}

/// Whether shutdown has begun, and the accepted runs going. The lifecycle is
/// shared by a coordinator's clones; its lock is a leaf, never held across
/// an `.await`.
#[derive(Clone)]
pub(super) struct AcceptedRuns {
    lifecycle: Arc<watch::Sender<Lifecycle>>,
    wait: Duration,
}

impl Default for AcceptedRuns {
    fn default() -> Self {
        Self {
            lifecycle: Arc::new(watch::channel(Lifecycle::default()).0),
            wait: Duration::from_millis(ACCEPTED_RUN_SHUTDOWN_WAIT_MS),
        }
    }
}

/// An accepted run its session queue started, going until dropped.
pub(super) struct GoingRun {
    run_id: String,
    lifecycle: Arc<watch::Sender<Lifecycle>>,
}

impl Drop for GoingRun {
    fn drop(&mut self) {
        self.lifecycle.send_modify(|lifecycle| {
            lifecycle.going.remove(&self.run_id);
        });
    }
}

impl AcceptedRuns {
    pub(super) fn is_closing(&self) -> bool {
        self.lifecycle.borrow().closing
    }

    /// Counts `run_id` as going, or `None` once shutdown has begun. The check
    /// and the count are one step under the lifecycle's lock, so shutdown
    /// either waits for the run or the run never starts.
    pub(super) fn begin(&self, run_id: &str) -> Option<GoingRun> {
        let mut began = false;
        self.lifecycle.send_if_modified(|lifecycle| {
            if lifecycle.closing {
                return false;
            }
            lifecycle.going.insert(run_id.to_string());
            began = true;
            true
        });
        began.then(|| GoingRun {
            run_id: run_id.to_string(),
            lifecycle: Arc::clone(&self.lifecycle),
        })
    }

    /// Resolves once shutdown has begun.
    pub(super) async fn closed(&self) {
        let mut lifecycle = self.lifecycle.subscribe();
        // `self` keeps the sender alive, so this ends only once closing.
        let _ = lifecycle.wait_for(|lifecycle| lifecycle.closing).await;
    }

    /// Begins shutdown, then waits up to the bound for the going runs to
    /// end. Returns the ones still going.
    async fn close_and_wait(&self) -> Vec<String> {
        let mut lifecycle = self.lifecycle.subscribe();
        self.lifecycle
            .send_modify(|lifecycle| lifecycle.closing = true);
        let ended = tokio::time::timeout(self.wait, async {
            let _ = lifecycle
                .wait_for(|lifecycle| lifecycle.going.is_empty())
                .await;
        })
        .await;
        if ended.is_ok() {
            return Vec::new();
        }
        let mut left = self
            .lifecycle
            .borrow()
            .going
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        left.sort();
        left
    }
}

/// `accept_run`'s refusal once shutdown has begun, and how an accepted run's
/// admission wait ends then.
pub(super) fn shutting_down() -> ApiError {
    ApiError::service_unavailable(DAEMON_SHUTTING_DOWN)
}

/// Whether a start ended with `shutting_down()`: its run never started, so
/// its session queue leaves it queued.
pub(crate) fn is_shutting_down(error: &ApiError) -> bool {
    error.status() == StatusCode::SERVICE_UNAVAILABLE && error.message() == DAEMON_SHUTTING_DOWN
}

impl AgentRunCoordinator {
    /// Graceful shutdown's first step (`app::serve_with_state`): see the
    /// module documentation.
    pub(crate) async fn shutdown(&self) {
        let left = self.accepted_runs.close_and_wait().await;
        if !left.is_empty() {
            warn!(
                run_ids = ?left,
                wait_ms = self.accepted_runs.wait.as_millis() as u64,
                "accepted runs were still going when shutdown stopped waiting; the restart interrupts them"
            );
        }
    }

    /// Whether shutdown has begun.
    pub(crate) fn is_shutting_down(&self) -> bool {
        self.accepted_runs.is_closing()
    }

    /// A shorter shutdown wait, so tests need not wait thirty seconds.
    #[cfg(test)]
    pub(crate) fn with_accepted_shutdown_wait(mut self, wait: Duration) -> Self {
        self.accepted_runs.wait = wait;
        self
    }
}
