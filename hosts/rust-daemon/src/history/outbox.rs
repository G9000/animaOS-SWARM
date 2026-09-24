//! History outbox (spec §13.1): committed messages and terminal runs reach the
//! history store within about a second, in batches, idempotently by id, with
//! retries and backoff. Records stay in the control plane until mirrored, and
//! only saved state is mirrored: the outbox reads the control plane under the
//! control-plane transaction. Deletions stay saved in the control plane
//! (`pendingHistoryDeletions`) until the store applies them. After a restart
//! or a queue overflow the hot transcript is reconciled against the store;
//! five minutes of failures become a readiness issue.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, MutexGuard};
use std::time::Duration;

use anima_core::primitives::now_millis;
use anima_core::Message;
use serde::{Deserialize, Serialize};
use tokio::sync::{watch, Mutex, Notify};
use tokio::task::JoinHandle;
use tracing::{error, warn};

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

/// A history deletion the control plane has saved and the store may not have
/// applied yet: one session, or without `session_id` the whole agent. The
/// control plane keeps these as `pendingHistoryDeletions`, so a restart
/// replays them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HistoryDeletion {
    pub(crate) agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) session_id: Option<String>,
}

impl HistoryDeletion {
    pub(crate) fn session(agent_id: &str, session_id: &str) -> Self {
        Self {
            agent_id: agent_id.to_string(),
            session_id: Some(session_id.to_string()),
        }
    }

    pub(crate) fn agent(agent_id: &str) -> Self {
        Self {
            agent_id: agent_id.to_string(),
            session_id: None,
        }
    }
}

#[derive(Clone, Debug)]
enum OutboxItem {
    Message(HistoryMessage),
    Delete(HistoryDeletion),
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
    /// Counts overflows, so a reconcile that overlapped one runs again.
    overflows: u64,
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
        deletion: HistoryDeletion,
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
    /// save that made them durable succeeded, and before that save's
    /// transaction ends, so a later deletion of the session queues after them.
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

    /// Queues the removal of a deleted session's rows. Record the deletion in
    /// the same control-plane save as the session's removal
    /// (`DaemonState::record_history_deletion`), and queue it once that save
    /// succeeded, inside the same control-plane transaction.
    pub(crate) fn enqueue_session_deletion(&self, agent_id: &str, session_id: &str) {
        self.enqueue_deletion(HistoryDeletion::session(agent_id, session_id));
    }

    /// Queues the removal of every row of a deleted agent; recorded and
    /// queued like a session deletion.
    pub(crate) fn enqueue_agent_deletion(&self, agent_id: &str) {
        self.enqueue_deletion(HistoryDeletion::agent(agent_id));
    }

    fn enqueue_deletion(&self, deletion: HistoryDeletion) {
        {
            let mut outbox = self.outbox();
            outbox.push(OutboxItem::Delete(deletion));
            self.enforce_capacity(&mut outbox);
        }
        self.wake.notify_one();
    }

    /// Queues the saved deletions of a restored control plane (the restart
    /// rule); the next flush applies them before it reconciles.
    /// `DaemonState::set_history` calls this for a new service.
    pub(crate) fn replay_deletions(&self, deletions: &[HistoryDeletion]) {
        if deletions.is_empty() {
            return;
        }
        {
            let mut outbox = self.outbox();
            for deletion in deletions {
                outbox.push(OutboxItem::Delete(deletion.clone()));
            }
        }
        self.wake.notify_one();
    }

    fn enforce_capacity(&self, outbox: &mut OutboxState) {
        if outbox.items.len() > self.max_items {
            // The hot transcript still holds every message; deletions must survive.
            outbox
                .items
                .retain(|queued| matches!(queued.item, OutboxItem::Delete(_)));
            outbox.needs_reconcile = true;
            outbox.overflows += 1;
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
    /// terminal runs the ledger has not mirrored yet. `transactions` is the
    /// control-plane transaction: the control plane is read only while it is
    /// held, so a commit whose save may still fail is never mirrored. The
    /// flush takes it itself (clearing an applied deletion saves under it), so
    /// never call this while holding it.
    pub(crate) async fn flush_once(
        &self,
        state: &SharedDaemonState,
        transactions: &Mutex<()>,
        now_ms: u64,
    ) -> Result<FlushReport, HistoryError> {
        let _flushing = self.flushing.lock().await;
        let mut report = FlushReport::default();
        let result = self.flush_locked(state, transactions, &mut report).await;
        self.record_result(&result, now_ms);
        result.map(|()| report)
    }

    async fn flush_locked(
        &self,
        state: &SharedDaemonState,
        transactions: &Mutex<()>,
        report: &mut FlushReport,
    ) -> Result<(), HistoryError> {
        if !self.reconciled() || self.outbox().needs_reconcile {
            // Queued deletions first, including the ones replayed at startup:
            // the reconcile must not count rows they are about to remove.
            self.write_queue(state, transactions, report).await?;
            report.reconciled = self.reconcile(state, transactions).await?;
        }
        self.write_queue(state, transactions, report).await?;
        self.write_runs(state, transactions, report).await
    }

    /// Writes queued items in order until the queue is empty. Each applied
    /// deletion's saved entry is cleared before the next item is written.
    async fn write_queue(
        &self,
        state: &SharedDaemonState,
        transactions: &Mutex<()>,
        report: &mut FlushReport,
    ) -> Result<(), HistoryError> {
        loop {
            match self.next_batch() {
                Batch::Empty => return Ok(()),
                Batch::Messages { through, rows } => {
                    self.store.upsert_messages(&rows).await?;
                    self.mark_mirrored(rows.iter().map(|row| row.message.id.as_str()));
                    self.complete_through(through);
                    report.messages += rows.len();
                }
                Batch::Deletion { through, deletion } => {
                    match &deletion.session_id {
                        Some(session_id) => {
                            self.store
                                .delete_session(&deletion.agent_id, session_id)
                                .await?
                        }
                        None => self.store.delete_agent(&deletion.agent_id).await?,
                    }
                    self.complete_through(through);
                    report.deletions += 1;
                    clear_saved_deletion(state, transactions, &deletion).await;
                }
            }
        }
    }

    /// Writes terminal runs in batches, read under the control-plane
    /// transaction; `mark_mirrored` skips any record that changed since. Runs
    /// of deleted agents are never saved, so they are never mirrored either.
    async fn write_runs(
        &self,
        state: &SharedDaemonState,
        transactions: &Mutex<()>,
        report: &mut FlushReport,
    ) -> Result<(), HistoryError> {
        loop {
            let runs = {
                let _transaction = transactions.lock().await;
                state
                    .read()
                    .await
                    .unmirrored_terminal_runs(HISTORY_RUN_BATCH)
            };
            if runs.is_empty() {
                return Ok(());
            }
            self.store.upsert_runs(&runs).await?;
            let marked = state.write().await.runs.mark_mirrored(&runs);
            report.runs += marked;
            if marked == 0 || runs.len() < HISTORY_RUN_BATCH {
                return Ok(());
            }
        }
    }

    /// The queue prefix to write next: up to a batch of messages, or one deletion.
    fn next_batch(&self) -> Batch {
        let outbox = self.outbox();
        let Some(first) = outbox.items.front() else {
            return Batch::Empty;
        };
        if let OutboxItem::Delete(deletion) = &first.item {
            return Batch::Deletion {
                through: first.seq,
                deletion: deletion.clone(),
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
                OutboxItem::Delete(_) => break,
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
    /// ones it is missing (spec §13.1 restart rule, §13.3 step 3). The hot
    /// transcript is read under the control-plane transaction twice: for the
    /// ids to check, and after the store round trip for the rows to queue.
    /// The second read skips messages removed meanwhile (a deleted session)
    /// and queues before it ends, so a deletion queued later stays behind the
    /// rows it must remove.
    ///
    /// Memory: on a first boot over a large transcript almost every hot
    /// message is missing, and each one is cloned into the queue at once,
    /// before capacity is enforced, so the transcript is briefly held twice.
    async fn reconcile(
        &self,
        state: &SharedDaemonState,
        transactions: &Mutex<()>,
    ) -> Result<usize, HistoryError> {
        let (hot_ids, overflows) = {
            let _transaction = transactions.lock().await;
            let guard = state.read().await;
            (hot_message_ids(&guard), self.outbox().overflows)
        };
        let mut stored = HashSet::new();
        for chunk in hot_ids.chunks(HISTORY_FLUSH_BATCH) {
            stored.extend(self.store.existing_message_ids(chunk).await?);
        }
        let checked = hot_ids.into_iter().collect::<HashSet<_>>();

        let _transaction = transactions.lock().await;
        let guard = state.read().await;
        let (missing, held) = reconcile_rows(&guard, &checked, &stored);
        self.mark_mirrored(held.iter().map(String::as_str));
        let mut outbox = self.outbox();
        let queued = outbox
            .items
            .iter()
            .filter_map(|queued| match &queued.item {
                OutboxItem::Message(row) => Some(row.message.id.clone()),
                OutboxItem::Delete(_) => None,
            })
            .collect::<HashSet<_>>();
        let mut count = 0;
        for row in missing {
            if !queued.contains(&row.message.id) {
                outbox.push(OutboxItem::Message(row));
                count += 1;
            }
        }
        // An overflow during the store round trip dropped messages the first
        // read never saw; the next flush reconciles again.
        if outbox.overflows == overflows {
            outbox.needs_reconcile = false;
        }
        drop(outbox);
        drop(guard);
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

/// Drops the saved entry of a deletion the store applied, in a normal save.
/// A failed save only delays that: the entry is already gone from memory, so
/// the next successful save drops it, and a restart before then repeats an
/// idempotent delete.
async fn clear_saved_deletion(
    state: &SharedDaemonState,
    transactions: &Mutex<()>,
    deletion: &HistoryDeletion,
) {
    let _transaction = transactions.lock().await;
    let persist = {
        let mut guard = state.write().await;
        if !guard.clear_history_deletion(deletion) {
            return;
        }
        guard.control_plane_persist_request()
    };
    if let Err(error) = persist.save().await {
        warn!(
            error = %error,
            agent_id = %deletion.agent_id,
            "could not save a finished history deletion; a restart repeats it"
        );
    }
}

/// The id of every hot message.
fn hot_message_ids(state: &DaemonState) -> Vec<String> {
    state
        .agents
        .values()
        .flat_map(|runtime| runtime.messages().iter().map(|message| message.id.clone()))
        .collect()
}

/// Rows for the `checked` hot messages the store is missing, and the ids of
/// those it holds; messages no longer hot are skipped.
fn reconcile_rows(
    state: &DaemonState,
    checked: &HashSet<String>,
    stored: &HashSet<String>,
) -> (Vec<HistoryMessage>, Vec<String>) {
    let mut missing = Vec::new();
    let mut held = Vec::new();
    for (agent_id, runtime) in &state.agents {
        let mut rooms: HashMap<&str, Vec<&Message>> = HashMap::new();
        for message in runtime.messages() {
            rooms
                .entry(message.room_id.as_str())
                .or_default()
                .push(message);
        }
        for (room_id, messages) in rooms {
            let session_id = session_id_for_room(room_id);
            let hidden = hidden_message_ids(messages.iter().copied());
            for message in messages {
                if !checked.contains(&message.id) {
                    continue;
                }
                if stored.contains(&message.id) {
                    held.push(message.id.clone());
                } else {
                    missing.push(HistoryMessage {
                        agent_id: agent_id.clone(),
                        session_id: session_id.clone(),
                        hidden: hidden.contains(&message.id),
                        message: message.clone(),
                    });
                }
            }
        }
    }
    (missing, held)
}

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
            parent: None,
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
        let transactions = coordinator.control_plane_transactions();
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

        let report = history
            .flush_once(&state, &transactions, now_millis())
            .await
            .unwrap();
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
            history
                .flush_once(&state, &transactions, now_millis())
                .await
                .unwrap(),
            FlushReport::default(),
            "nothing is written twice"
        );
    }

    #[tokio::test]
    async fn a_failing_store_keeps_records_until_it_recovers_and_reports_after_five_minutes() {
        let store = Arc::new(FlakyHistoryStore::new());
        let (state, coordinator, agent_id) = state_with(HistoryService::new(store.clone())).await;
        let transactions = coordinator.control_plane_transactions();
        coordinator
            .run(request(&agent_id, "chat:one", "hello"))
            .await
            .unwrap();
        let history = state.read().await.history.clone();
        store.set_failing(true);
        let started = 1_000_000;

        assert!(history
            .flush_once(&state, &transactions, started)
            .await
            .is_err());
        assert_eq!(history.pending_count(), 2);
        assert!(!state.read().await.runs.for_agent(&agent_id)[0].mirrored);
        assert_eq!(
            history.readiness_issue(started + HISTORY_READINESS_GRACE_MS - 1),
            None
        );
        assert!(history
            .flush_once(&state, &transactions, started + 60_000)
            .await
            .is_err());
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
            .flush_once(
                &state,
                &transactions,
                started + HISTORY_READINESS_GRACE_MS + 1,
            )
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
        let transactions = coordinator.control_plane_transactions();
        coordinator
            .run(request(&agent_id, "chat:one", "before the crash"))
            .await
            .unwrap();
        // The process died before the outbox flushed: a fresh service, same store.
        let restarted = HistoryService::new(store.clone());
        state.write().await.set_history(Arc::clone(&restarted));
        assert!(!restarted.reconciled());

        let report = restarted
            .flush_once(&state, &transactions, now_millis())
            .await
            .unwrap();
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
        let transactions = coordinator.control_plane_transactions();
        coordinator
            .run(request(&agent_id, "chat:one", "first"))
            .await
            .unwrap();
        let history = state.read().await.history.clone();
        history
            .flush_once(&state, &transactions, now_millis())
            .await
            .unwrap();

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

        let report = history
            .flush_once(&state, &transactions, now_millis())
            .await
            .unwrap();
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
        let transactions = Mutex::new(());
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

        let report = history
            .flush_once(&state, &transactions, now_millis())
            .await
            .unwrap();

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
        let transactions = Mutex::new(());
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
        history
            .flush_once(&state, &transactions, now_millis())
            .await
            .unwrap();

        let rows = store
            .page_messages(&page("agent-1", "schedule:s1"))
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.hidden));
    }

    #[tokio::test]
    async fn a_session_deleted_while_a_reconcile_awaits_the_store_stays_deleted() {
        let store = Arc::new(FlakyHistoryStore::new());
        let (state, coordinator, agent_id) = state_with(HistoryService::new(store.clone())).await;
        let transactions = coordinator.control_plane_transactions();
        coordinator
            .run(request(&agent_id, "chat:doomed", "delete me"))
            .await
            .unwrap();
        coordinator
            .run(request(&agent_id, "chat:kept", "keep me"))
            .await
            .unwrap();
        // A restart before any flush: the reconcile finds every message missing.
        let restarted = HistoryService::new(store.clone());
        state.write().await.set_history(Arc::clone(&restarted));
        let gate = store.hold_next_existence_check();
        let flush = {
            let restarted = Arc::clone(&restarted);
            let state = Arc::clone(&state);
            let transactions = Arc::clone(&transactions);
            tokio::spawn(async move {
                restarted
                    .flush_once(&state, &transactions, now_millis())
                    .await
            })
        };
        gate.entered.acquire().await.unwrap().forget();

        // The session delete (Task 12): drop the hot messages and finished
        // runs, save, then queue the history deletion, all in one transaction.
        {
            let _transaction = coordinator.control_plane_transaction().await;
            let persist = {
                let mut guard = state.write().await;
                guard
                    .agents
                    .get_mut(&agent_id)
                    .unwrap()
                    .retain_messages(|message| message.room_id != "chat:doomed");
                let doomed_runs = guard
                    .runs
                    .for_agent(&agent_id)
                    .into_iter()
                    .filter(|run| run.session_id == "chat:doomed")
                    .map(|run| run.id.clone())
                    .collect::<Vec<_>>();
                for run_id in doomed_runs {
                    guard.runs.remove(&run_id);
                }
                guard.control_plane_persist_request()
            };
            persist.save().await.unwrap();
            restarted.enqueue_session_deletion(&agent_id, "chat:doomed");
        }
        gate.release.add_permits(1);
        flush.await.unwrap().unwrap();
        restarted
            .flush_once(&state, &transactions, now_millis())
            .await
            .unwrap();

        assert!(
            store
                .page_messages(&page(&agent_id, "chat:doomed"))
                .await
                .unwrap()
                .is_empty(),
            "the reconcile must not bring the deleted session back"
        );
        assert_eq!(
            store
                .page_messages(&page(&agent_id, "chat:kept"))
                .await
                .unwrap()
                .len(),
            2
        );
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

    #[tokio::test]
    async fn saved_deletions_replay_before_the_reconcile_and_clear_once_applied() {
        let store = Arc::new(FlakyHistoryStore::new());
        let (state, coordinator, agent_id) = state_with(HistoryService::new(store.clone())).await;
        let transactions = coordinator.control_plane_transactions();
        // A deleted chat's id was reused and the new chat mirrored, then the
        // daemon stopped before the deletion's entry was cleared.
        coordinator
            .run(request(&agent_id, "chat:reused", "a new chat"))
            .await
            .unwrap();
        let history = state.read().await.history.clone();
        history
            .flush_once(&state, &transactions, now_millis())
            .await
            .unwrap();
        state
            .write()
            .await
            .record_history_deletion(HistoryDeletion::session(&agent_id, "chat:reused"));
        // A deleted agent's rows the store still holds.
        store
            .upsert_messages(&[history_message(
                "msg-7-7",
                "agent-gone",
                "chat:old",
                MessageRole::User,
                "from a deleted agent",
                7,
            )])
            .await
            .unwrap();
        state
            .write()
            .await
            .record_history_deletion(HistoryDeletion::agent("agent-gone"));

        let restarted = HistoryService::new(store.clone());
        state.write().await.set_history(Arc::clone(&restarted));
        assert_eq!(
            restarted.pending_count(),
            2,
            "the saved deletions are queued"
        );
        store.set_failing(true);
        assert!(restarted
            .flush_once(&state, &transactions, now_millis())
            .await
            .is_err());
        assert_eq!(
            state.read().await.pending_history_deletions.len(),
            2,
            "an unapplied deletion stays saved"
        );

        store.set_failing(false);
        let report = restarted
            .flush_once(&state, &transactions, now_millis())
            .await
            .unwrap();
        assert_eq!(
            (report.deletions, report.reconciled, report.messages),
            (2, 2, 2)
        );
        assert_eq!(
            store
                .page_messages(&page(&agent_id, "chat:reused"))
                .await
                .unwrap()
                .len(),
            2,
            "the reconcile after the replay mirrors the hot chat again"
        );
        assert!(
            store
                .page_messages(&page("agent-gone", "chat:old"))
                .await
                .unwrap()
                .is_empty(),
            "a replayed agent deletion removes every row of the agent"
        );
        assert!(
            state.read().await.pending_history_deletions.is_empty(),
            "cleared once the store applied it"
        );
    }

    #[tokio::test]
    async fn an_agent_deletion_removes_its_rows_and_keeps_its_runs_out_of_the_store() {
        let store = Arc::new(MemoryHistoryStore::new());
        let (state, coordinator, agent_id) = state_with(HistoryService::new(store.clone())).await;
        let transactions = coordinator.control_plane_transactions();
        let survivor = state
            .write()
            .await
            .create_agent(config("survivor"))
            .unwrap()
            .state
            .id;
        coordinator
            .run(request(&agent_id, "chat:one", "mirrored"))
            .await
            .unwrap();
        let history = state.read().await.history.clone();
        history
            .flush_once(&state, &transactions, now_millis())
            .await
            .unwrap();
        coordinator
            .run(request(&agent_id, "chat:two", "still queued"))
            .await
            .unwrap();
        coordinator
            .run(request(&survivor, "chat:one", "unrelated"))
            .await
            .unwrap();
        let run_ids = state
            .read()
            .await
            .runs
            .for_agent(&agent_id)
            .into_iter()
            .map(|run| run.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(run_ids.len(), 2);

        // The agent delete (Task 12): remove the agent and its terminal runs,
        // and record its history deletion, in one save, then queue the
        // deletion. Unlike Task 6, the runs leave the ledger immediately
        // instead of lingering until a restart (Controller ruling 2, M2
        // pre-flight audit): `unmirrored_terminal` skips agents that no
        // longer exist, so they would otherwise never be mirrored or pruned.
        {
            let _transaction = coordinator.control_plane_transaction().await;
            let persist = {
                let mut guard = state.write().await;
                guard.remove_agent(&agent_id);
                guard.runs.remove_terminal_for_agent(&agent_id);
                guard.record_history_deletion(HistoryDeletion::agent(&agent_id));
                guard.control_plane_persist_request()
            };
            persist.save().await.unwrap();
            history.enqueue_agent_deletion(&agent_id);
        }
        let report = history
            .flush_once(&state, &transactions, now_millis())
            .await
            .unwrap();

        assert_eq!(report.deletions, 1);
        for session in ["chat:one", "chat:two"] {
            assert!(
                store
                    .page_messages(&page(&agent_id, session))
                    .await
                    .unwrap()
                    .is_empty(),
                "{session}"
            );
        }
        assert!(
            state.read().await.runs.for_agent(&agent_id).is_empty(),
            "the deleted agent's runs are dropped from the ledger in the same save as the agent"
        );
        for run_id in run_ids {
            assert_eq!(
                store.get_run(&run_id).await.unwrap(),
                None,
                "a deleted agent's runs are not mirrored"
            );
        }
        assert_eq!(
            store
                .page_messages(&page(&survivor, "chat:one"))
                .await
                .unwrap()
                .len(),
            2
        );
        assert!(state.read().await.pending_history_deletions.is_empty());
    }
}
