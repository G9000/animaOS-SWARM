//! The history worker: the loop that flushes the outbox while a router owns
//! it, and the owner token that keeps it running.

use std::sync::{Arc, Mutex as StdMutex};

use anima_core::primitives::now_millis;
use tokio::sync::{watch, Mutex};
use tokio::task::JoinHandle;
use tracing::{error, warn};

use super::lock;
use crate::app::SharedDaemonState;

struct WorkerHandle {
    stop: watch::Sender<bool>,
    join: JoinHandle<()>,
}

/// Keeps a started history loop running. The router that owns the worker
/// holds one in its app state; once every clone is dropped the loop stops
/// without writing anything more. `HistoryWorker::shutdown` is the way to
/// stop it with a final flush.
#[derive(Clone)]
pub(crate) struct HistoryWorkerOwner {
    /// Never sent on: the loop waits for this channel to close.
    alive: watch::Sender<()>,
}

impl HistoryWorkerOwner {
    pub(crate) fn new() -> Self {
        Self {
            alive: watch::channel(()).0,
        }
    }
}

/// Runs the outbox flush loop; Task 13 adds hot-tail pruning to it. The loop
/// ends on `shutdown`, after one final flush, or once every
/// `HistoryWorkerOwner` it was started with is dropped. Worker handles never
/// keep it running.
#[derive(Clone)]
pub(crate) struct HistoryWorker {
    state: SharedDaemonState,
    /// The control-plane transaction, under which flushes read the control
    /// plane; hot-tail pruning (Task 13) takes it too.
    transactions: Arc<Mutex<()>>,
    running: Arc<StdMutex<Option<WorkerHandle>>>,
}

impl HistoryWorker {
    pub(crate) fn new(state: SharedDaemonState, transactions: Arc<Mutex<()>>) -> Self {
        Self {
            state,
            transactions,
            running: Arc::new(StdMutex::new(None)),
        }
    }

    /// Starts the flush loop, which runs while `owner` or a clone of it
    /// lives. Needs a Tokio runtime; a second call is a no-op.
    pub(crate) fn start(&self, owner: &HistoryWorkerOwner) {
        let mut running = lock(&self.running);
        if running.is_some() {
            return;
        }
        let (stop, mut stopping) = watch::channel(false);
        // The loop holds a stop sender itself, so dropping every handle leaves
        // the stop channel open: only `shutdown` stops it that way.
        let keep_open = stop.clone();
        let mut owned = owner.alive.subscribe();
        let state = Arc::clone(&self.state);
        let transactions = Arc::clone(&self.transactions);
        let join = tokio::spawn(async move {
            let _keep_open = keep_open;
            loop {
                let history = state.read().await.history.clone();
                tokio::select! {
                    biased;
                    _ = stopping.wait_for(|stop| *stop) => break,
                    // Returns once the last owner is dropped.
                    _ = owned.changed() => break,
                    () = history.wait_for_work() => {}
                }
                let _ = history
                    .flush_once(&state, &transactions, now_millis())
                    .await;
            }
        });
        *running = Some(WorkerHandle { stop, join });
    }

    /// Whether the loop was started and has ended.
    #[cfg(test)]
    pub(crate) fn has_stopped(&self) -> bool {
        lock(&self.running)
            .as_ref()
            .is_some_and(|handle| handle.join.is_finished())
    }

    /// Stops the loop and makes one final flush attempt.
    pub(crate) async fn shutdown(&self) {
        let handle = lock(&self.running).take();
        if let Some(handle) = handle {
            let _ = handle.stop.send(true);
            if let Err(error) = handle.join.await {
                error!(
                    error = %error,
                    "history worker loop ended abnormally; records stay in the control plane"
                );
            }
        }
        let history = self.state.read().await.history.clone();
        if let Err(error) = history
            .flush_once(&self.state, &self.transactions, now_millis())
            .await
        {
            warn!(error = %error, "final history flush failed; records stay in the control plane");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::conformance::{history_message, FlakyHistoryStore};
    use crate::history::{HistoryService, HistoryStore, MessagePageQuery, HISTORY_FLUSH_INTERVAL};
    use crate::state::DaemonState;
    use anima_core::MessageRole;
    use std::time::Duration;
    use tokio::sync::RwLock;

    fn page(agent_id: &str, session_id: &str) -> MessagePageQuery {
        MessagePageQuery {
            agent_id: agent_id.into(),
            session_id: session_id.into(),
            before: None,
            limit: 100,
            include_hidden: true,
        }
    }

    #[tokio::test]
    async fn shutdown_logs_a_loop_that_panicked_and_still_flushes_once_more() {
        #[derive(Clone, Default)]
        struct Captured(Arc<StdMutex<Vec<u8>>>);

        impl std::io::Write for Captured {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                lock(&self.0).extend_from_slice(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let store = Arc::new(FlakyHistoryStore::new());
        let history = HistoryService::new(store.clone());
        let mut daemon = DaemonState::new();
        daemon.set_history(Arc::clone(&history));
        let state = Arc::new(RwLock::new(daemon));
        let worker = HistoryWorker::new(Arc::clone(&state), Arc::new(Mutex::new(())));
        let owner = HistoryWorkerOwner::new();
        store.panic_on_next_write();
        worker.start(&owner);
        history.enqueue_committed(
            "agent-1",
            "chat:one",
            &[history_message(
                "msg-1-1",
                "agent-1",
                "chat:one",
                MessageRole::User,
                "hello",
                1,
            )
            .message],
        );
        let deadline = tokio::time::Instant::now() + HISTORY_FLUSH_INTERVAL;
        while !worker.has_stopped() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the store's panic ends the loop"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let captured = Captured::default();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer({
                let captured = captured.clone();
                move || captured.clone()
            })
            .finish();
        {
            let _default = tracing::subscriber::set_default(subscriber);
            worker.shutdown().await;
        }
        let logged = String::from_utf8(lock(&captured.0).clone()).unwrap();

        assert!(
            logged.contains("ERROR") && logged.contains("history worker loop ended abnormally"),
            "{logged}"
        );
        assert_eq!(
            store
                .page_messages(&page("agent-1", "chat:one"))
                .await
                .unwrap()
                .len(),
            1,
            "shutdown still flushes once more"
        );
    }
}
