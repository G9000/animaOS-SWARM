//! History outbox (spec §13.1): committed messages and terminal runs reach the
//! history store within about a second, in batches, idempotently by id, with
//! retries and backoff. Records stay in the control plane until mirrored.
//! After a restart or a queue overflow the hot transcript is reconciled
//! against the store; five minutes of failures become a readiness issue.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, MutexGuard};
use std::time::Duration;

use anima_core::primitives::now_millis;
use anima_core::Message;
use tokio::sync::{watch, Mutex, Notify};
use tokio::task::JoinHandle;
use tracing::warn;

use super::{HistoryError, HistoryMessage, HistoryStore, MemoryHistoryStore};
use crate::app::SharedDaemonState;
use crate::sessions::{hidden_message_ids, session_id_for_room};
use crate::state::DaemonState;

/// The outbox flushes at least this often (spec §13.1).
pub(crate) const HISTORY_FLUSH_INTERVAL: Duration = Duration::from_secs(1);
/// Messages per store write.
pub(crate) const HISTORY_FLUSH_BATCH: usize = 500;
/// Terminal runs per store write.
pub(crate) const HISTORY_RUN_BATCH: usize = 200;
/// The longest wait between retries while the store fails.
pub(crate) const HISTORY_MAX_BACKOFF: Duration = Duration::from_secs(30);
/// Failing this long is a readiness issue (spec §13.1).
pub(crate) const HISTORY_READINESS_GRACE_MS: u64 = 5 * 60 * 1000;
/// Queued items before the queue gives way to a reconciliation.
pub(crate) const MAX_OUTBOX_ITEMS: usize = 100_000;

fn lock<T>(mutex: &StdMutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Clone, Debug)]
enum OutboxItem {
    Message(HistoryMessage),
    DeleteSession {
        agent_id: String,
        session_id: String,
    },
}

#[derive(Debug)]
struct Queued {
    seq: u64,
    item: OutboxItem,
}

#[derive(Debug, Default)]
struct OutboxState {
    next_seq: u64,
    items: VecDeque<Queued>,
    /// The queue overflowed and dropped its message copies; the next flush
    /// reads the hot transcript for what is still unmirrored.
    needs_reconcile: bool,
    failing_since_ms: Option<u64>,
    consecutive_failures: u32,
    last_error: Option<String>,
}

impl OutboxState {
    fn push(&mut self, item: OutboxItem) {
        self.next_seq += 1;
        self.items.push_back(Queued {
            seq: self.next_seq,
            item,
        });
    }
}

enum Batch {
    Empty,
    Messages {
        through: u64,
        rows: Vec<HistoryMessage>,
    },
    Deletion {
        through: u64,
        agent_id: String,
        session_id: String,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FlushReport {
    pub(crate) messages: usize,
    pub(crate) runs: usize,
    pub(crate) deletions: usize,
    pub(crate) reconciled: usize,
}

pub(crate) struct HistoryService {
    store: Arc<dyn HistoryStore>,
    max_items: usize,
    outbox: StdMutex<OutboxState>,
    /// Hot message ids the store is known to hold; only these may be pruned.
    mirrored: StdMutex<HashSet<String>>,
    reconciled: AtomicBool,
    wake: Notify,
    flushing: Mutex<()>,
}

pub(crate) type SharedHistory = Arc<HistoryService>;

impl HistoryService {
    pub(crate) fn new(store: Arc<dyn HistoryStore>) -> SharedHistory {
        Self::with_capacity(store, MAX_OUTBOX_ITEMS)
    }

    pub(crate) fn with_capacity(store: Arc<dyn HistoryStore>, max_items: usize) -> SharedHistory {
        Arc::new(Self {
            store,
            max_items: max_items.max(1),
            outbox: StdMutex::new(OutboxState::default()),
            mirrored: StdMutex::new(HashSet::new()),
            reconciled: AtomicBool::new(false),
            wake: Notify::new(),
            flushing: Mutex::new(()),
        })
    }

    /// The ephemeral default: bounded in-memory tables (spec §13.1).
    pub(crate) fn ephemeral() -> SharedHistory {
        Self::new(Arc::new(MemoryHistoryStore::new()))
    }

    pub(crate) fn store(&self) -> Arc<dyn HistoryStore> {
        Arc::clone(&self.store)
    }

    pub(crate) fn is_ephemeral(&self) -> bool {
        self.store.is_ephemeral()
    }

    /// The hot transcript has been checked against the store since startup.
    pub(crate) fn reconciled(&self) -> bool {
        self.reconciled.load(Ordering::Acquire)
    }

    fn outbox(&self) -> MutexGuard<'_, OutboxState> {
        lock(&self.outbox)
    }

    fn mirrored(&self) -> MutexGuard<'_, HashSet<String>> {
        lock(&self.mirrored)
    }

    /// Queues one durable commit's messages. Call only after the control-plane
    /// save that made them durable succeeded.
    pub(crate) fn enqueue_committed(&self, agent_id: &str, session_id: &str, messages: &[Message]) {
        if messages.is_empty() {
            return;
        }
        let hidden = hidden_message_ids(messages.iter());
        {
            let mut outbox = self.outbox();
            for message in messages {
                outbox.push(OutboxItem::Message(HistoryMessage {
                    agent_id: agent_id.to_string(),
                    session_id: session_id.to_string(),
                    hidden: hidden.contains(&message.id),
                    message: message.clone(),
                }));
            }
            self.enforce_capacity(&mut outbox);
        }
        self.wake.notify_one();
    }

    /// Queues the removal of a deleted session's rows, after its deletion was saved.
    pub(crate) fn enqueue_session_deletion(&self, agent_id: &str, session_id: &str) {
        {
            let mut outbox = self.outbox();
            outbox.push(OutboxItem::DeleteSession {
                agent_id: agent_id.to_string(),
                session_id: session_id.to_string(),
            });
            self.enforce_capacity(&mut outbox);
        }
        self.wake.notify_one();
    }

    fn enforce_capacity(&self, outbox: &mut OutboxState) {
        if outbox.items.len() > self.max_items {
            // The hot transcript still holds every message; deletions must survive.
            outbox
                .items
                .retain(|queued| matches!(queued.item, OutboxItem::DeleteSession { .. }));
            outbox.needs_reconcile = true;
        }
    }

    pub(crate) fn is_mirrored(&self, message_id: &str) -> bool {
        self.mirrored().contains(message_id)
    }

    /// Drops ids that are no longer hot (pruned or deleted).
    pub(crate) fn forget_mirrored<'a>(&self, message_ids: impl IntoIterator<Item = &'a str>) {
        let mut mirrored = self.mirrored();
        for id in message_ids {
            mirrored.remove(id);
        }
    }

    fn mark_mirrored<'a>(&self, message_ids: impl IntoIterator<Item = &'a str>) {
        let mut mirrored = self.mirrored();
        for id in message_ids {
            mirrored.insert(id.to_string());
        }
    }

    /// Queued items not yet written.
    #[cfg(test)]
    pub(crate) fn pending_count(&self) -> usize {
        self.outbox().items.len()
    }

    fn is_failing(&self) -> bool {
        self.outbox().failing_since_ms.is_some()
    }

    /// The wait before the next attempt: the flush interval, doubled per
    /// consecutive failure up to `HISTORY_MAX_BACKOFF`.
    pub(crate) fn retry_delay(&self) -> Duration {
        let failures = self.outbox().consecutive_failures;
        if failures == 0 {
            return HISTORY_FLUSH_INTERVAL;
        }
        HISTORY_FLUSH_INTERVAL
            .saturating_mul(2u32.saturating_pow(failures - 1))
            .min(HISTORY_MAX_BACKOFF)
    }

    pub(crate) fn readiness_issue(&self, now_ms: u64) -> Option<String> {
        let outbox = self.outbox();
        let since = outbox.failing_since_ms?;
        let failing_for = now_ms.saturating_sub(since);
        (failing_for >= HISTORY_READINESS_GRACE_MS).then(|| {
            format!(
                "history store writes have failed for {} minutes; records stay in the control plane until it recovers ({})",
                failing_for / 60_000,
                outbox.last_error.as_deref().unwrap_or("unknown error")
            )
        })
    }

    async fn wait_for_work(&self) {
        if self.is_failing() {
            tokio::time::sleep(self.retry_delay()).await;
        } else {
            let _ = tokio::time::timeout(HISTORY_FLUSH_INTERVAL, self.wake.notified()).await;
        }
    }

    /// Reconciles when needed, writes queued items in order, then writes
    /// terminal runs the ledger has not mirrored yet.
    pub(crate) async fn flush_once(
        &self,
        state: &SharedDaemonState,
        now_ms: u64,
    ) -> Result<FlushReport, HistoryError> {
        let _flushing = self.flushing.lock().await;
        let mut report = FlushReport::default();
        let result = self.flush_locked(state, &mut report).await;
        self.record_result(&result, now_ms);
        result.map(|()| report)
    }

    async fn flush_locked(
        &self,
        state: &SharedDaemonState,
        report: &mut FlushReport,
    ) -> Result<(), HistoryError> {
        if !self.reconciled() || self.outbox().needs_reconcile {
            report.reconciled = self.reconcile(state).await?;
        }
        loop {
            match self.next_batch() {
                Batch::Empty => break,
                Batch::Messages { through, rows } => {
                    self.store.upsert_messages(&rows).await?;
                    self.mark_mirrored(rows.iter().map(|row| row.message.id.as_str()));
                    self.complete_through(through);
                    report.messages += rows.len();
                }
                Batch::Deletion {
                    through,
                    agent_id,
                    session_id,
                } => {
                    self.store.delete_session(&agent_id, &session_id).await?;
                    self.complete_through(through);
                    report.deletions += 1;
                }
            }
        }
        loop {
            let runs = state
                .read()
                .await
                .runs
                .unmirrored_terminal(HISTORY_RUN_BATCH);
            if runs.is_empty() {
                break;
            }
            self.store.upsert_runs(&runs).await?;
            let marked = state.write().await.runs.mark_mirrored(&runs);
            report.runs += marked;
            if marked == 0 || runs.len() < HISTORY_RUN_BATCH {
                break;
            }
        }
        Ok(())
    }

    /// The queue prefix to write next: up to a batch of messages, or one deletion.
    fn next_batch(&self) -> Batch {
        let outbox = self.outbox();
        let Some(first) = outbox.items.front() else {
            return Batch::Empty;
        };
        if let OutboxItem::DeleteSession {
            agent_id,
            session_id,
        } = &first.item
        {
            return Batch::Deletion {
                through: first.seq,
                agent_id: agent_id.clone(),
                session_id: session_id.clone(),
            };
        }
        let mut rows = Vec::new();
        let mut through = first.seq;
        for queued in outbox.items.iter().take(HISTORY_FLUSH_BATCH) {
            match &queued.item {
                OutboxItem::Message(row) => {
                    rows.push(row.clone());
                    through = queued.seq;
                }
                OutboxItem::DeleteSession { .. } => break,
            }
        }
        Batch::Messages { through, rows }
    }

    fn complete_through(&self, seq: u64) {
        let mut outbox = self.outbox();
        while outbox.items.front().is_some_and(|queued| queued.seq <= seq) {
            outbox.items.pop_front();
        }
    }

    /// Marks hot messages the store already holds as mirrored and queues the
    /// ones it is missing (spec §13.1 restart rule, §13.3 step 3).
    async fn reconcile(&self, state: &SharedDaemonState) -> Result<usize, HistoryError> {
        let hot = hot_messages_by_session(&*state.read().await);
        let queued = self
            .outbox()
            .items
            .iter()
            .filter_map(|queued| match &queued.item {
                OutboxItem::Message(row) => Some(row.message.id.clone()),
                OutboxItem::DeleteSession { .. } => None,
            })
            .collect::<HashSet<_>>();
        let mut missing = Vec::new();
        for (agent_id, session_id, messages) in hot {
            let hidden = hidden_message_ids(messages.iter());
            for chunk in messages.chunks(HISTORY_FLUSH_BATCH) {
                let ids = chunk
                    .iter()
                    .map(|message| message.id.clone())
                    .collect::<Vec<_>>();
                let existing = self.store.existing_message_ids(&ids).await?;
                self.mark_mirrored(existing.iter().map(String::as_str));
                for message in chunk.iter().filter(|message| {
                    !existing.contains(&message.id) && !queued.contains(&message.id)
                }) {
                    missing.push(HistoryMessage {
                        agent_id: agent_id.clone(),
                        session_id: session_id.clone(),
                        hidden: hidden.contains(&message.id),
                        message: message.clone(),
                    });
                }
            }
        }
        let count = missing.len();
        {
            let mut outbox = self.outbox();
            for row in missing {
                outbox.push(OutboxItem::Message(row));
            }
            outbox.needs_reconcile = false;
        }
        self.reconciled.store(true, Ordering::Release);
        Ok(count)
    }

    fn record_result(&self, result: &Result<(), HistoryError>, now_ms: u64) {
        let mut outbox = self.outbox();
        match result {
            Ok(()) => {
                outbox.failing_since_ms = None;
                outbox.consecutive_failures = 0;
                outbox.last_error = None;
            }
            Err(error) => {
                outbox.failing_since_ms.get_or_insert(now_ms);
                outbox.consecutive_failures = outbox.consecutive_failures.saturating_add(1);
                outbox.last_error = Some(error.to_string());
                warn!(
                    error = %error,
                    pending = outbox.items.len(),
                    "history store write failed; records stay in the control plane"
                );
            }
        }
    }
}

/// Every hot message, grouped by (agent, session) in transcript order.
fn hot_messages_by_session(state: &DaemonState) -> Vec<(String, String, Vec<Message>)> {
    let mut sessions = Vec::new();
    for (agent_id, runtime) in &state.agents {
        let mut rooms: HashMap<&str, Vec<Message>> = HashMap::new();
        for message in runtime.messages() {
            rooms
                .entry(message.room_id.as_str())
                .or_default()
                .push(message.clone());
        }
        for (room_id, messages) in rooms {
            sessions.push((agent_id.clone(), session_id_for_room(room_id), messages));
        }
    }
    sessions
}

struct WorkerHandle {
    cancel: watch::Sender<bool>,
    join: JoinHandle<()>,
}

/// Runs the outbox flush loop; Task 13 adds hot-tail pruning to it.
#[derive(Clone)]
pub(crate) struct HistoryWorker {
    state: SharedDaemonState,
    /// The control-plane transaction; hot-tail pruning (Task 13) takes it.
    #[allow(dead_code)]
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

    /// Starts the flush loop. Needs a Tokio runtime; a second call is a no-op.
    pub(crate) fn start(&self) {
        let mut running = lock(&self.running);
        if running.is_some() {
            return;
        }
        let (cancel, mut cancelled) = watch::channel(false);
        let state = Arc::clone(&self.state);
        let join = tokio::spawn(async move {
            loop {
                let history = state.read().await.history.clone();
                tokio::select! {
                    changed = cancelled.changed() => {
                        if changed.is_err() || *cancelled.borrow() {
                            break;
                        }
                    }
                    () = history.wait_for_work() => {}
                }
                let _ = history.flush_once(&state, now_millis()).await;
            }
        });
        *running = Some(WorkerHandle { cancel, join });
    }

    /// Stops the loop and makes one final flush attempt.
    pub(crate) async fn shutdown(&self) {
        let handle = lock(&self.running).take();
        if let Some(handle) = handle {
            let _ = handle.cancel.send(true);
            let _ = handle.join.await;
        }
        let history = self.state.read().await.history.clone();
        if let Err(error) = history.flush_once(&self.state, now_millis()).await {
            warn!(error = %error, "final history flush failed; records stay in the control plane");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_runs::{AgentRunCoordinator, AgentRunRequest, RunRoom};
    use crate::history::conformance::{history_message, FlakyHistoryStore};
    use crate::history::MessagePageQuery;
    use crate::runs::RunSource;
    use anima_core::{AgentConfig, AgentSettings, Content, MessageRole};
    use tokio::sync::{RwLock, Semaphore};

    fn config(name: &str) -> AgentConfig {
        AgentConfig {
            name: name.into(),
            model: "deterministic".into(),
            bio: None,
            lore: None,
            knowledge: None,
            topics: None,
            adjectives: None,
            style: None,
            provider: None,
            system: None,
            tools: None,
            plugins: None,
            settings: Some(AgentSettings::default()),
        }
    }

    fn request(agent_id: &str, room: &str, text: &str) -> AgentRunRequest {
        AgentRunRequest {
            agent_id: agent_id.into(),
            content: Content {
                text: text.into(),
                ..Content::default()
            },
            room: RunRoom::Stable(room.into()),
            idempotency_key: None,
            source: RunSource::Api,
            source_ref: None,
        }
    }

    fn page(agent_id: &str, session_id: &str) -> MessagePageQuery {
        MessagePageQuery {
            agent_id: agent_id.into(),
            session_id: session_id.into(),
            before: None,
            limit: 100,
            include_hidden: true,
        }
    }

    async fn state_with(
        history: SharedHistory,
    ) -> (SharedDaemonState, AgentRunCoordinator, String) {
        let mut daemon = DaemonState::new();
        daemon.set_history(history);
        let agent_id = daemon.create_agent(config("historian")).unwrap().state.id;
        let state = Arc::new(tokio::sync::RwLock::new(daemon));
        let coordinator = AgentRunCoordinator::new(Arc::clone(&state), Arc::new(Semaphore::new(4)));
        (state, coordinator, agent_id)
    }

    #[tokio::test]
    async fn committed_turns_and_terminal_runs_reach_the_store_and_runs_are_marked_mirrored() {
        let store = Arc::new(MemoryHistoryStore::new());
        let (state, coordinator, agent_id) = state_with(HistoryService::new(store.clone())).await;
        coordinator
            .run(request(&agent_id, "chat:one", "hello"))
            .await
            .unwrap();
        let history = state.read().await.history.clone();
        assert_eq!(
            history.pending_count(),
            2,
            "the committed turn waits in the outbox"
        );

        let report = history.flush_once(&state, now_millis()).await.unwrap();
        assert_eq!(
            report,
            FlushReport {
                messages: 2,
                runs: 1,
                deletions: 0,
                reconciled: 0
            }
        );
        assert_eq!(history.pending_count(), 0);
        assert!(history.reconciled());
        let rows = store
            .page_messages(&page(&agent_id, "chat:one"))
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| history.is_mirrored(&row.message.id)));
        let run = state.read().await.runs.for_agent(&agent_id)[0].clone();
        assert!(run.mirrored);
        let mut stored = store
            .get_run(&run.id)
            .await
            .unwrap()
            .expect("the terminal run is stored");
        stored.mirrored = true;
        assert_eq!(stored, run);
        assert_eq!(
            history.flush_once(&state, now_millis()).await.unwrap(),
            FlushReport::default(),
            "nothing is written twice"
        );
    }

    #[tokio::test]
    async fn a_failing_store_keeps_records_until_it_recovers_and_reports_after_five_minutes() {
        let store = Arc::new(FlakyHistoryStore::new());
        let (state, coordinator, agent_id) = state_with(HistoryService::new(store.clone())).await;
        coordinator
            .run(request(&agent_id, "chat:one", "hello"))
            .await
            .unwrap();
        let history = state.read().await.history.clone();
        store.set_failing(true);
        let started = 1_000_000;

        assert!(history.flush_once(&state, started).await.is_err());
        assert_eq!(history.pending_count(), 2);
        assert!(!state.read().await.runs.for_agent(&agent_id)[0].mirrored);
        assert_eq!(
            history.readiness_issue(started + HISTORY_READINESS_GRACE_MS - 1),
            None
        );
        assert!(history.flush_once(&state, started + 60_000).await.is_err());
        assert_eq!(history.retry_delay(), HISTORY_FLUSH_INTERVAL * 2);
        let issue = history
            .readiness_issue(started + HISTORY_READINESS_GRACE_MS)
            .expect("five minutes of failures is a readiness issue");
        assert!(
            issue.starts_with("history store writes have failed for 5 minutes; records stay in the control plane until it recovers (injected history store failure)"),
            "{issue}"
        );

        store.set_failing(false);
        let report = history
            .flush_once(&state, started + HISTORY_READINESS_GRACE_MS + 1)
            .await
            .unwrap();
        assert_eq!((report.messages, report.runs), (2, 1));
        assert_eq!(
            history.readiness_issue(started + 2 * HISTORY_READINESS_GRACE_MS),
            None
        );
        assert_eq!(history.retry_delay(), HISTORY_FLUSH_INTERVAL);
        assert!(state.read().await.runs.for_agent(&agent_id)[0].mirrored);
    }

    #[tokio::test]
    async fn a_restart_mirrors_hot_messages_the_store_is_missing() {
        let store = Arc::new(MemoryHistoryStore::new());
        let (state, coordinator, agent_id) = state_with(HistoryService::new(store.clone())).await;
        coordinator
            .run(request(&agent_id, "chat:one", "before the crash"))
            .await
            .unwrap();
        // The process died before the outbox flushed: a fresh service, same store.
        let restarted = HistoryService::new(store.clone());
        state.write().await.set_history(Arc::clone(&restarted));
        assert!(!restarted.reconciled());

        let report = restarted.flush_once(&state, now_millis()).await.unwrap();
        assert_eq!((report.reconciled, report.messages, report.runs), (2, 2, 1));
        assert!(restarted.reconciled());
        assert_eq!(
            store
                .page_messages(&page(&agent_id, "chat:one"))
                .await
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn an_overflowing_queue_falls_back_to_reconciling_the_hot_transcript() {
        let store = Arc::new(MemoryHistoryStore::new());
        let (state, coordinator, agent_id) =
            state_with(HistoryService::with_capacity(store.clone(), 3)).await;
        coordinator
            .run(request(&agent_id, "chat:one", "first"))
            .await
            .unwrap();
        let history = state.read().await.history.clone();
        history.flush_once(&state, now_millis()).await.unwrap();

        coordinator
            .run(request(&agent_id, "chat:one", "second"))
            .await
            .unwrap();
        coordinator
            .run(request(&agent_id, "chat:one", "third"))
            .await
            .unwrap();
        assert_eq!(
            history.pending_count(),
            0,
            "four queued messages overflowed three slots"
        );

        let report = history.flush_once(&state, now_millis()).await.unwrap();
        assert_eq!((report.reconciled, report.messages), (4, 4));
        assert_eq!(
            store
                .page_messages(&page(&agent_id, "chat:one"))
                .await
                .unwrap()
                .len(),
            6
        );
    }

    #[tokio::test]
    async fn a_session_deletion_removes_the_rows_queued_before_it() {
        let store = Arc::new(MemoryHistoryStore::new());
        let history = HistoryService::new(store.clone());
        let mut daemon = DaemonState::new();
        daemon.set_history(Arc::clone(&history));
        let state = Arc::new(RwLock::new(daemon));
        let turn = [
            history_message(
                "msg-1-1",
                "agent-1",
                "chat:one",
                MessageRole::User,
                "hello",
                1,
            )
            .message,
            history_message(
                "msg-2-2",
                "agent-1",
                "chat:one",
                MessageRole::Assistant,
                "hi",
                2,
            )
            .message,
        ];
        history.enqueue_committed("agent-1", "chat:one", &turn);
        history.enqueue_session_deletion("agent-1", "chat:one");
        history.enqueue_committed(
            "agent-1",
            "chat:two",
            &[history_message(
                "msg-3-3",
                "agent-1",
                "chat:two",
                MessageRole::User,
                "other",
                3,
            )
            .message],
        );

        let report = history.flush_once(&state, now_millis()).await.unwrap();

        assert_eq!((report.messages, report.deletions), (3, 1));
        assert!(store
            .page_messages(&page("agent-1", "chat:one"))
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            store
                .page_messages(&page("agent-1", "chat:two"))
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(history.pending_count(), 0);
    }

    #[tokio::test]
    async fn silent_checkin_turns_are_stored_hidden() {
        let store = Arc::new(MemoryHistoryStore::new());
        let history = HistoryService::new(store.clone());
        let mut daemon = DaemonState::new();
        daemon.set_history(Arc::clone(&history));
        let state = Arc::new(RwLock::new(daemon));
        let mut prompt = history_message(
            "msg-1-1",
            "agent-1",
            "schedule:s1",
            MessageRole::User,
            "Check",
            1,
        )
        .message;
        prompt.content.metadata = Some(std::collections::BTreeMap::from([(
            "kind".to_string(),
            anima_core::DataValue::String("checkin".into()),
        )]));
        let reply = history_message(
            "msg-2-2",
            "agent-1",
            "schedule:s1",
            MessageRole::Assistant,
            "CHECKIN_OK",
            2,
        )
        .message;
        history.enqueue_committed("agent-1", "schedule:s1", &[prompt, reply]);
        history.flush_once(&state, now_millis()).await.unwrap();

        let rows = store
            .page_messages(&page("agent-1", "schedule:s1"))
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.hidden));
    }
}
