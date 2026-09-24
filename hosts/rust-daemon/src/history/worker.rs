//! The history worker: the loop that flushes the outbox and prunes the hot
//! tail while a router owns it, and the owner token that keeps it running.

use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use anima_core::primitives::now_millis;
use tokio::sync::{watch, Mutex};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::{error, info, warn};

use super::lock;
use crate::app::SharedDaemonState;
use crate::sessions::pruning::{prune_in_transaction, PRUNE_INTERVAL_MS};

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

/// Runs the outbox flush loop and, every ten minutes, hot-tail pruning. The
/// loop ends on `shutdown`, after one final flush, or once every
/// `HistoryWorkerOwner` it was started with is dropped. Worker handles never
/// keep it running.
#[derive(Clone)]
pub(crate) struct HistoryWorker {
    state: SharedDaemonState,
    /// The control-plane transaction, under which flushes read the control
    /// plane and every prune runs.
    transactions: Arc<Mutex<()>>,
    /// How often the loop prunes the hot tail.
    prune_interval: Duration,
    running: Arc<StdMutex<Option<WorkerHandle>>>,
}

impl HistoryWorker {
    pub(crate) fn new(state: SharedDaemonState, transactions: Arc<Mutex<()>>) -> Self {
        Self {
            state,
            transactions,
            prune_interval: Duration::from_millis(PRUNE_INTERVAL_MS),
            running: Arc::new(StdMutex::new(None)),
        }
    }

    /// A shorter pruning interval, so a test need not wait ten minutes.
    #[cfg(test)]
    pub(crate) fn with_prune_interval(mut self, prune_interval: Duration) -> Self {
        self.prune_interval = prune_interval;
        self
    }

    /// Starts the loop, which flushes and, once its pruning interval is due,
    /// prunes while `owner` or a clone of it lives. Needs a Tokio runtime; a
    /// second call is a no-op.
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
        let prune_interval = self.prune_interval;
        let join = tokio::spawn(async move {
            let _keep_open = keep_open;
            let mut next_prune_at = Instant::now() + prune_interval;
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
                if Instant::now() < next_prune_at {
                    continue;
                }
                next_prune_at = Instant::now() + prune_interval;
                let pruned = {
                    let transaction = transactions.lock().await;
                    // The flush, or a commit this waited for, may have
                    // outlived the last owner or a shutdown request: a stale
                    // instance must never prune and save its snapshot over a
                    // newer one.
                    if *stopping.borrow() || owned.has_changed().is_err() {
                        break;
                    }
                    prune_in_transaction(&state, &transaction, now_millis()).await
                };
                match pruned {
                    Ok(0) => {}
                    Ok(pruned) => {
                        info!(
                            pruned,
                            "moved old mirrored messages out of the control plane"
                        );
                    }
                    Err(error) => {
                        warn!(error = %error, "hot-tail pruning could not save; the messages stay in the control plane");
                    }
                }
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
    use crate::sessions::pruning::HOT_TAIL_MESSAGES;
    use crate::sessions::test_support::{agent_config, message, seed_messages};
    use crate::state::DaemonState;
    use anima_core::MessageRole;
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

    /// An agent with one more old message in `chat:a` than the hot tail keeps,
    /// none of them mirrored yet, and a worker that prunes on every pass.
    fn state_with_an_old_chat(store: Arc<FlakyHistoryStore>) -> (SharedDaemonState, String) {
        let mut daemon = DaemonState::new();
        daemon.set_history(HistoryService::new(store));
        let agent = daemon
            .create_agent(agent_config("companion"))
            .unwrap()
            .state
            .id;
        let messages = (0..=HOT_TAIL_MESSAGES)
            .map(|index| {
                let role = if index % 2 == 0 {
                    MessageRole::User
                } else {
                    MessageRole::Assistant
                };
                message(
                    &agent,
                    &format!("m{index:03}"),
                    "chat:a",
                    role,
                    "old",
                    1_000 + index as u64,
                )
            })
            .collect();
        seed_messages(&mut daemon, &agent, messages);
        (Arc::new(RwLock::new(daemon)), agent)
    }

    async fn hot_ids(state: &SharedDaemonState, agent_id: &str) -> Vec<String> {
        state.read().await.agents[agent_id]
            .messages()
            .iter()
            .map(|message| message.id.clone())
            .collect()
    }

    #[tokio::test]
    async fn the_loop_prunes_the_hot_tail_on_its_interval_while_an_owner_lives() {
        let store = Arc::new(FlakyHistoryStore::new());
        let (state, agent) = state_with_an_old_chat(Arc::clone(&store));
        let worker = HistoryWorker::new(Arc::clone(&state), Arc::new(Mutex::new(())))
            .with_prune_interval(Duration::ZERO);
        let owner = HistoryWorkerOwner::new();
        worker.start(&owner);

        // The first pass reconciles (mirroring every message), then prunes.
        let deadline = tokio::time::Instant::now() + 5 * HISTORY_FLUSH_INTERVAL;
        while hot_ids(&state, &agent).await.len() > HOT_TAIL_MESSAGES {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the loop prunes once its interval is due"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        assert_eq!(hot_ids(&state, &agent).await[0], "m001");
        assert!(
            store
                .get_message(&agent, "chat:a", "m000")
                .await
                .unwrap()
                .is_some(),
            "the pruned message stays in the history store"
        );
        assert!(!worker.has_stopped());
        drop(owner);
        worker.shutdown().await;
    }

    #[tokio::test]
    async fn a_flush_that_outlives_the_last_owner_is_not_followed_by_a_prune() {
        let store = Arc::new(FlakyHistoryStore::new());
        let (state, agent) = state_with_an_old_chat(Arc::clone(&store));
        let worker = HistoryWorker::new(Arc::clone(&state), Arc::new(Mutex::new(())))
            .with_prune_interval(Duration::ZERO);
        let owner = HistoryWorkerOwner::new();
        // Hold the first pass inside its reconcile's store round trip.
        let gate = store.hold_next_existence_check();
        worker.start(&owner);
        gate.entered.acquire().await.unwrap().forget();

        // The last owner goes (a stale app instance) while that flush runs.
        drop(owner);
        gate.release.add_permits(1);
        let deadline = tokio::time::Instant::now() + 5 * HISTORY_FLUSH_INTERVAL;
        while !worker.has_stopped() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the loop ends once its owners are gone"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        assert_eq!(
            hot_ids(&state, &agent).await.len(),
            HOT_TAIL_MESSAGES + 1,
            "a stale instance must not prune and save its snapshot"
        );
        let history = state.read().await.history.clone();
        assert!(
            history.is_mirrored("m000"),
            "the flush in progress finished"
        );
    }
}
