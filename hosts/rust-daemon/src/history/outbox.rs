//! History outbox (spec §13.1): committed messages, terminal runs, decided
//! approvals, and automation fire records reach the history store within about a second, in batches,
//! idempotently by id, with retries and backoff. Records stay in the control plane until mirrored, and
//! saved state is what is mirrored: the outbox reads the control plane under the
//! control-plane transaction. The exceptions are timeout and stop settlements
//! whose save failed, and in-memory orphan sweeps, which are mirrored too; a
//! restart before the next save may then record those approvals as `expired`. Deletions stay saved in the control plane
//! (`pendingHistoryDeletions`) until the store applies them. After a restart
//! or a queue overflow the hot transcript is reconciled against the store;
//! five minutes of failures become a readiness issue.
//!
//! Usage (spec §11): a terminal run's rows are derived from its record as it
//! is mirrored, so they are as durable as the run. Secondary calls (titles,
//! compaction, profile, agency) queue their rows in a separate bounded
//! in-memory queue, never ordered against deletions (usage survives them);
//! those rows are lost on a crash before the next flush.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, MutexGuard};
use std::time::Duration;

use anima_core::Message;
use tokio::sync::{Mutex, Notify};
use tracing::warn;

use super::{
    lock, HistoryDeletion, HistoryError, HistoryMessage, HistoryStore, MemoryHistoryStore,
};
use crate::app::SharedDaemonState;
use crate::sessions::{hidden_message_ids, session_id_for_room};
use crate::state::DaemonState;
use crate::usage::{usage_records_for_run, UsageRecord, HISTORY_USAGE_BATCH, USAGE_QUEUE_MAX};

/// The outbox flushes at least this often (spec §13.1).
pub(crate) const HISTORY_FLUSH_INTERVAL: Duration = Duration::from_secs(1);
/// Messages per store write.
pub(crate) const HISTORY_FLUSH_BATCH: usize = 500;
/// Terminal runs per store write.
pub(crate) const HISTORY_RUN_BATCH: usize = 200;
/// Decided approvals per store write.
pub(crate) const HISTORY_APPROVAL_BATCH: usize = 200;
/// Automation fire records per store write.
pub(crate) const HISTORY_FIRE_BATCH: usize = 200;
/// The longest wait between retries while the store fails.
pub(crate) const HISTORY_MAX_BACKOFF: Duration = Duration::from_secs(30);
/// Failing this long is a readiness issue (spec §13.1).
pub(crate) const HISTORY_READINESS_GRACE_MS: u64 = 5 * 60 * 1000;
/// Queued items before the queue gives way to a reconciliation.
pub(crate) const MAX_OUTBOX_ITEMS: usize = 100_000;

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
    /// Failed flushes since start (spec 11.3).
    flush_errors: u64,
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
    pub(crate) approvals: usize,
    pub(crate) fires: usize,
    pub(crate) deletions: usize,
    pub(crate) reconciled: usize,
    /// Usage rows written: those derived from mirrored runs and the queued
    /// secondary-call rows.
    pub(crate) usage: usize,
}

/// Secondary-call usage rows waiting for the store, oldest first.
#[derive(Debug, Default)]
struct UsageQueue {
    next_seq: u64,
    rows: VecDeque<(u64, UsageRecord)>,
    /// Set by the first drop of an overflow burst, so it warns once; cleared
    /// once the queue is back under its bound.
    overflowing: bool,
}

/// What the status and metrics routes read from the outbox (spec 11.3).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct HistoryStats {
    /// Queued items not yet written.
    pub(crate) pending: usize,
    /// Secondary-call usage rows not yet written.
    pub(crate) usage_queued: usize,
    pub(crate) failing_since_ms: Option<u64>,
    /// The latest failure's text, unredacted: the caller redacts it.
    pub(crate) last_error: Option<String>,
    pub(crate) flush_errors: u64,
    /// Messages the pruner has moved out of the hot tail since start.
    pub(crate) pruned_messages: u64,
}

pub(crate) struct HistoryService {
    store: Arc<dyn HistoryStore>,
    max_items: usize,
    outbox: StdMutex<OutboxState>,
    max_usage: usize,
    usage: StdMutex<UsageQueue>,
    pruned_messages: AtomicU64,
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
        Self::with_capacities(store, max_items, USAGE_QUEUE_MAX)
    }

    fn with_capacities(
        store: Arc<dyn HistoryStore>,
        max_items: usize,
        max_usage: usize,
    ) -> SharedHistory {
        Arc::new(Self {
            store,
            max_items: max_items.max(1),
            outbox: StdMutex::new(OutboxState::default()),
            max_usage: max_usage.max(1),
            usage: StdMutex::new(UsageQueue::default()),
            pruned_messages: AtomicU64::new(0),
            mirrored: StdMutex::new(HashSet::new()),
            reconciled: AtomicBool::new(false),
            wake: Notify::new(),
            flushing: Mutex::new(()),
        })
    }

    /// A service whose usage queue holds at most `max_usage` rows.
    #[cfg(test)]
    pub(crate) fn with_usage_capacity(
        store: Arc<dyn HistoryStore>,
        max_usage: usize,
    ) -> SharedHistory {
        Self::with_capacities(store, MAX_OUTBOX_ITEMS, max_usage)
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

    fn usage(&self) -> MutexGuard<'_, UsageQueue> {
        lock(&self.usage)
    }

    /// Queues secondary-call usage rows (titles, compaction, profile,
    /// agency) for the next flush. The queue holds at most `USAGE_QUEUE_MAX`
    /// rows: past it the oldest are dropped, with one warning per overflow
    /// burst (controller ruling 2), logged once the queue's lock is released.
    pub(crate) fn enqueue_usage(&self, records: Vec<UsageRecord>) {
        if records.is_empty() {
            return;
        }
        let (dropped, first_of_burst) = {
            let mut usage = self.usage();
            for record in records {
                usage.next_seq += 1;
                let seq = usage.next_seq;
                usage.rows.push_back((seq, record));
            }
            let excess = usage.rows.len().saturating_sub(self.max_usage);
            usage.rows.drain(..excess);
            let first_of_burst = excess > 0 && !usage.overflowing;
            if excess > 0 {
                usage.overflowing = true;
            }
            (excess, first_of_burst)
        };
        if first_of_burst {
            warn!(
                dropped,
                limit = self.max_usage,
                "the usage queue is full; the oldest usage rows were dropped"
            );
        }
        self.wake.notify_one();
    }

    /// Usage rows queued and not yet written, oldest first.
    #[cfg(test)]
    pub(crate) fn pending_usage(&self) -> Vec<UsageRecord> {
        self.usage()
            .rows
            .iter()
            .map(|(_, record)| record.clone())
            .collect()
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

    /// Adds ids to the mirrored set. Callers pass the control-plane
    /// transaction they hold across their check of the saved state and this
    /// call, so a deletion cannot be recorded (and `forget_mirrored` run)
    /// between the two and leave its ids marked (residual round R6).
    fn mark_mirrored<'a>(
        &self,
        _transaction: &tokio::sync::MutexGuard<'_, ()>,
        message_ids: impl IntoIterator<Item = &'a str>,
    ) {
        let mut mirrored = self.mirrored();
        for id in message_ids {
            mirrored.insert(id.to_string());
        }
    }

    /// A snapshot of the outbox's health for the status and metrics routes.
    pub(crate) fn stats(&self) -> HistoryStats {
        let usage_queued = self.usage().rows.len();
        let outbox = self.outbox();
        HistoryStats {
            pending: outbox.items.len(),
            usage_queued,
            failing_since_ms: outbox.failing_since_ms,
            last_error: outbox.last_error.clone(),
            flush_errors: outbox.flush_errors,
            pruned_messages: self.pruned_messages.load(Ordering::Relaxed),
        }
    }

    /// Counts messages the pruner moved out of the hot tail.
    pub(crate) fn note_pruned(&self, count: usize) {
        self.pruned_messages
            .fetch_add(count as u64, Ordering::Relaxed);
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

    pub(super) async fn wait_for_work(&self) {
        if self.is_failing() {
            tokio::time::sleep(self.retry_delay()).await;
        } else {
            let _ = tokio::time::timeout(HISTORY_FLUSH_INTERVAL, self.wake.notified()).await;
        }
    }

    /// Reconciles when needed, writes queued items in order, then writes
    /// terminal runs and decided approvals not mirrored yet. `transactions` is the
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
        self.write_runs(state, transactions, report).await?;
        self.write_approvals(state, transactions, report).await?;
        self.write_schedule_fires(state, transactions, report)
            .await?;
        self.write_usage(report).await
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
                    // D1 (mirrored-set leak, final fix wave): a row whose
                    // session or agent already has a deletion recorded is
                    // written (so a store that has not applied that deletion
                    // yet stays consistent once it does) but never marked
                    // mirrored -- the deletion is always queued behind every
                    // message it covers (`enqueue_committed` runs before
                    // `enqueue_session_deletion`/`enqueue_agent_deletion`
                    // ever queues, in the same control-plane transaction
                    // this read is under), so it is about to remove the row
                    // regardless, and nothing else ever forgets a mirrored id
                    // once the deletion has already applied and cleared. The
                    // check and the marking share one hold of the transaction
                    // (residual round R6), taken only once the store write
                    // has returned, so no deletion lands between them.
                    {
                        let transaction = transactions.lock().await;
                        let daemon = state.read().await;
                        let newly_mirrored = rows.iter().filter_map(|row| {
                            let deleted = daemon.pending_history_deletions.iter().any(|deletion| {
                                deletion_covers(deletion, &row.agent_id, &row.session_id)
                            });
                            (!deleted).then(|| row.message.id.as_str())
                        });
                        self.mark_mirrored(&transaction, newly_mirrored);
                    }
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
    /// Each run's usage rows (one per step, plus any remainder) are derived
    /// and written first, priced with the overrides read under the same
    /// hold; both writes are idempotent by id, so a failure of either leaves
    /// the runs unmirrored and the next flush repeats both. A row the store
    /// already holds keeps its price (controller ruling 3).
    async fn write_runs(
        &self,
        state: &SharedDaemonState,
        transactions: &Mutex<()>,
        report: &mut FlushReport,
    ) -> Result<(), HistoryError> {
        loop {
            let (runs, overrides) = {
                let _transaction = transactions.lock().await;
                let guard = state.read().await;
                (
                    guard.unmirrored_terminal_runs(HISTORY_RUN_BATCH),
                    guard.pricing_overrides.clone(),
                )
            };
            if runs.is_empty() {
                return Ok(());
            }
            let usage = runs
                .iter()
                .flat_map(|run| usage_records_for_run(run, &overrides))
                .collect::<Vec<_>>();
            for chunk in usage.chunks(HISTORY_USAGE_BATCH) {
                self.store.upsert_usage(chunk).await?;
            }
            report.usage += usage.len();
            self.store.upsert_runs(&runs).await?;
            let marked = state.write().await.runs.mark_mirrored(&runs);
            report.runs += marked;
            if marked == 0 || runs.len() < HISTORY_RUN_BATCH {
                return Ok(());
            }
        }
    }

    /// Writes decided approvals in batches, read under the control-plane
    /// transaction so only saved decisions are mirrored; each one the store
    /// then holds unchanged leaves the control plane (spec §7.3).
    async fn write_approvals(
        &self,
        state: &SharedDaemonState,
        transactions: &Mutex<()>,
        report: &mut FlushReport,
    ) -> Result<(), HistoryError> {
        loop {
            let approvals = {
                let _transaction = transactions.lock().await;
                state
                    .write()
                    .await
                    .unmirrored_decided_approvals(HISTORY_APPROVAL_BATCH)
            };
            if approvals.is_empty() {
                return Ok(());
            }
            self.store.upsert_approvals(&approvals).await?;
            let removed = state.write().await.approvals.mark_mirrored(&approvals);
            report.approvals += removed;
            if removed == 0 || approvals.len() < HISTORY_APPROVAL_BATCH {
                return Ok(());
            }
        }
    }

    /// Writes automation fire records in batches, read under the
    /// control-plane transaction so only saved outcomes are mirrored; each
    /// one the store then holds unchanged leaves the control plane, and a
    /// deleted agent's are dropped instead (spec §9.1, §13.1).
    async fn write_schedule_fires(
        &self,
        state: &SharedDaemonState,
        transactions: &Mutex<()>,
        report: &mut FlushReport,
    ) -> Result<(), HistoryError> {
        loop {
            let fires = {
                let _transaction = transactions.lock().await;
                state
                    .write()
                    .await
                    .unmirrored_schedule_fires(HISTORY_FIRE_BATCH)
            };
            if fires.is_empty() {
                return Ok(());
            }
            self.store.upsert_schedule_runs(&fires).await?;
            let removed = state.write().await.schedule_fires.mark_mirrored(&fires);
            report.fires += removed;
            if removed == 0 || fires.len() < HISTORY_FIRE_BATCH {
                return Ok(());
            }
        }
    }

    /// Writes the queued secondary-call usage rows in batches. A batch
    /// leaves the queue only once the store holds it (by sequence, so rows
    /// an overflow dropped meanwhile are not dropped twice); a failure keeps
    /// it queued for the next flush.
    async fn write_usage(&self, report: &mut FlushReport) -> Result<(), HistoryError> {
        loop {
            let (through, records) = {
                let usage = self.usage();
                let Some((through, _)) = usage.rows.iter().take(HISTORY_USAGE_BATCH).last() else {
                    return Ok(());
                };
                let records = usage
                    .rows
                    .iter()
                    .take(HISTORY_USAGE_BATCH)
                    .map(|(_, record)| record.clone())
                    .collect::<Vec<_>>();
                (*through, records)
            };
            self.store.upsert_usage(&records).await?;
            report.usage += records.len();
            let mut usage = self.usage();
            while usage.rows.front().is_some_and(|(seq, _)| *seq <= through) {
                usage.rows.pop_front();
            }
            if usage.rows.len() < self.max_usage {
                usage.overflowing = false;
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

        let transaction = transactions.lock().await;
        let guard = state.read().await;
        let (missing, held) = reconcile_rows(&guard, &checked, &stored);
        self.mark_mirrored(&transaction, held.iter().map(String::as_str));
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
                outbox.flush_errors = outbox.flush_errors.saturating_add(1);
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

/// Whether a recorded deletion (an agent, or one of its sessions) covers a
/// row: an agent-wide deletion (`session_id: None`) covers every one of that
/// agent's sessions.
fn deletion_covers(deletion: &HistoryDeletion, agent_id: &str, session_id: &str) -> bool {
    deletion.agent_id == agent_id
        && match deletion.session_id.as_deref() {
            Some(deleted_session) => deleted_session == session_id,
            None => true,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_runs::{AgentRunCoordinator, AgentRunRequest, RunRoom};
    use crate::history::conformance::{history_message, FlakyHistoryStore};
    use crate::history::MessagePageQuery;
    use crate::runs::RunSource;
    use anima_core::primitives::now_millis;
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
                approvals: 0,
                fires: 0,
                deletions: 0,
                reconciled: 0,
                // The run's one model call.
                usage: 1
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
    async fn stats_report_pending_failures_and_pruned_counts() {
        let store = Arc::new(FlakyHistoryStore::new());
        let (state, coordinator, agent_id) = state_with(HistoryService::new(store.clone())).await;
        let transactions = coordinator.control_plane_transactions();
        coordinator
            .run(request(&agent_id, "chat:one", "hello"))
            .await
            .unwrap();
        let history = state.read().await.history.clone();
        history.enqueue_usage(vec![secondary_row("title_1", &agent_id, 5)]);
        let before = history.stats();
        assert_eq!((before.pending, before.usage_queued), (2, 1));
        assert_eq!(
            (
                before.failing_since_ms,
                before.last_error,
                before.flush_errors
            ),
            (None, None, 0)
        );

        store.set_failing(true);
        assert!(history
            .flush_once(&state, &transactions, 1_000)
            .await
            .is_err());
        assert!(history
            .flush_once(&state, &transactions, 2_000)
            .await
            .is_err());
        history.note_pruned(3);
        history.note_pruned(2);
        let failing = history.stats();
        assert_eq!(failing.pending, 2);
        assert_eq!(failing.failing_since_ms, Some(1_000));
        assert_eq!(
            failing.last_error.as_deref(),
            Some("injected history store failure")
        );
        assert_eq!(failing.flush_errors, 2);
        assert_eq!(failing.pruned_messages, 5);

        store.set_failing(false);
        history
            .flush_once(&state, &transactions, 3_000)
            .await
            .unwrap();
        let healed = history.stats();
        assert_eq!(
            (
                healed.pending,
                healed.usage_queued,
                healed.failing_since_ms,
                healed.last_error
            ),
            (0, 0, None, None)
        );
        assert_eq!(healed.flush_errors, 2, "the counter never resets");
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
    async fn a_session_deletion_recorded_before_the_flush_leaves_its_queued_message_unmirrored() {
        // D1 (mirrored-set leak, final fix wave): the session-delete route
        // records the deletion (`DaemonState::record_history_deletion`) and
        // queues it (`enqueue_session_deletion`) only after its own session's
        // messages are already queued, so the same flush that finally mirrors
        // a message queued before its session was deleted also deletes that
        // same row right after. `write_queue` must not mark such a message
        // mirrored in the first place -- `forget_mirrored`, called by the
        // route before this flush ever runs, is a no-op for an id that was
        // never mirrored yet, so nothing else ever forgets it again.
        let store = Arc::new(MemoryHistoryStore::new());
        let history = HistoryService::new(store.clone());
        let mut daemon = DaemonState::new();
        daemon.set_history(Arc::clone(&history));
        let state = Arc::new(RwLock::new(daemon));
        let transactions = Mutex::new(());

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
        state
            .write()
            .await
            .record_history_deletion(HistoryDeletion::session("agent-1", "chat:one"));
        history.enqueue_session_deletion("agent-1", "chat:one");

        let report = history
            .flush_once(&state, &transactions, now_millis())
            .await
            .unwrap();

        assert_eq!((report.messages, report.deletions), (1, 1));
        assert!(store
            .page_messages(&page("agent-1", "chat:one"))
            .await
            .unwrap()
            .is_empty());
        assert!(
            !history.is_mirrored("msg-1-1"),
            "a message mirrored in the same flush that deletes its session must not stay mirrored"
        );
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

    /// `agent_id`'s chat `session_id` with one decided and one pending approval.
    async fn with_approvals(state: &SharedDaemonState, agent_id: &str, session_id: &str) -> String {
        use crate::history::conformance::decided_approval;
        let mut guard = state.write().await;
        guard.sessions.insert(crate::sessions::SessionRecord::new(
            agent_id,
            session_id,
            crate::sessions::SessionKind::Chat,
            crate::sessions::SessionOrigin::Web,
            "Chat".into(),
            crate::sessions::TitleSource::Owner,
            1,
        ));
        let decided = decided_approval(&format!("apr_done_{session_id}"), agent_id, session_id, 10);
        let mut pending =
            decided_approval(&format!("apr_wait_{session_id}"), agent_id, session_id, 20);
        pending.status = crate::approvals::ApprovalStatus::Pending;
        pending.resolution = None;
        guard.approvals.insert(decided.clone());
        guard.approvals.insert(pending);
        decided.id
    }

    #[tokio::test]
    async fn decided_approvals_move_to_the_store_and_leave_the_control_plane() {
        let store = Arc::new(MemoryHistoryStore::new());
        let (state, coordinator, agent_id) = state_with(HistoryService::new(store.clone())).await;
        let transactions = coordinator.control_plane_transactions();
        let decided = with_approvals(&state, &agent_id, "chat:one").await;
        let history = state.read().await.history.clone();

        let report = history
            .flush_once(&state, &transactions, now_millis())
            .await
            .unwrap();

        assert_eq!(report.approvals, 1);
        assert_eq!(
            store.get_approval(&decided).await.unwrap().unwrap().id,
            decided
        );
        let guard = state.read().await;
        assert!(
            guard.approvals.get(&decided).is_none(),
            "the store holds it now"
        );
        assert!(
            guard.approvals.get("apr_wait_chat:one").is_some(),
            "a pending approval is never written"
        );
        assert!(store
            .get_approval("apr_wait_chat:one")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn a_failing_store_keeps_decided_approvals_and_a_deleted_session_drops_its_own() {
        let store = Arc::new(FlakyHistoryStore::new());
        let (state, coordinator, agent_id) = state_with(HistoryService::new(store.clone())).await;
        let transactions = coordinator.control_plane_transactions();
        let kept = with_approvals(&state, &agent_id, "chat:kept").await;
        let orphan = with_approvals(&state, &agent_id, "chat:gone").await;
        state.write().await.sessions.remove(&agent_id, "chat:gone");
        let history = state.read().await.history.clone();
        store.set_failing(true);

        assert!(history
            .flush_once(&state, &transactions, now_millis())
            .await
            .is_err());
        assert!(state.read().await.approvals.get(&kept).is_some());

        store.set_failing(false);
        let report = history
            .flush_once(&state, &transactions, now_millis())
            .await
            .unwrap();
        assert_eq!(report.approvals, 1);
        assert!(store.get_approval(&kept).await.unwrap().is_some());
        assert!(
            store.get_approval(&orphan).await.unwrap().is_none(),
            "a deleted session's approval is never written"
        );
        assert!(state.read().await.approvals.get(&orphan).is_none());
    }

    #[tokio::test]
    async fn fires_reach_the_store_and_leave_the_control_plane() {
        let store = Arc::new(FlakyHistoryStore::new());
        let (state, coordinator, agent_id) = state_with(HistoryService::new(store.clone())).await;
        let transactions = coordinator.control_plane_transactions();
        let kept = crate::schedules::history::tests_support_fire(&agent_id);
        let mut orphan = kept.clone();
        orphan.id = "schedule:s1:20".into();
        orphan.agent_id = "agent-deleted".into();
        {
            let mut guard = state.write().await;
            guard.schedule_fires.record(kept.clone());
            guard.schedule_fires.record(orphan);
        }
        let history = state.read().await.history.clone();
        store.set_failing(true);

        assert!(history
            .flush_once(&state, &transactions, now_millis())
            .await
            .is_err());
        assert_eq!(
            state.read().await.schedule_fires.snapshot(),
            vec![kept.clone()],
            "a failing store keeps the fire; the deleted agent's was dropped, not written"
        );

        store.set_failing(false);
        let report = history
            .flush_once(&state, &transactions, now_millis())
            .await
            .unwrap();
        assert_eq!(report.fires, 1);
        assert_eq!(
            store.page_schedule_runs(&agent_id, "s1", 50).await.unwrap(),
            vec![kept]
        );
        assert!(
            store
                .page_schedule_runs("agent-deleted", "s1", 50)
                .await
                .unwrap()
                .is_empty(),
            "a deleted agent's fire is dropped, never written"
        );
        assert_eq!(state.read().await.schedule_fires.len(), 0);
    }

    fn tokens(prompt: u64, completion: u64) -> anima_core::TokenUsage {
        anima_core::TokenUsage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: prompt + completion,
            ..anima_core::TokenUsage::default()
        }
    }

    fn all_usage(agent_id: &str) -> crate::history::UsagePageQuery {
        crate::history::UsagePageQuery {
            from_ms: 0,
            to_ms: u64::MAX,
            agent_id: Some(agent_id.into()),
            session_id: None,
            before: None,
            limit: 10_000,
        }
    }

    /// A finished run of `agent_id` in `chat:one` whose `steps` model calls
    /// each spent 10 + 2 tokens, saved in the ledger unmirrored.
    async fn finished_run(
        state: &SharedDaemonState,
        agent_id: &str,
        run_id: &str,
        provider: &str,
        model: &str,
        steps: u64,
    ) -> crate::runs::RunRecord {
        let mut run = crate::runs::RunRecord::running(
            crate::runs::RunStart {
                agent_id: agent_id.into(),
                session_id: "chat:one".into(),
                source: RunSource::Web,
                source_ref: None,
                idempotency_key: None,
                text: "hi".into(),
                model: model.into(),
                provider: Some(provider.into()),
                parent_run_id: None,
            },
            1_000,
        );
        run.id = run_id.into();
        run.steps = (1..=steps)
            .map(|step| crate::runs::RunStepUsage {
                step_id: format!("{run_id}:{step}"),
                usage: tokens(10, 2),
                at_ms: 1_000 + step,
                duration_ms: 5,
            })
            .collect();
        run.usage = tokens(10 * steps, 2 * steps);
        run.finish(crate::runs::RunStatus::Completed, None, 2_000);
        state.write().await.runs.insert(run.clone());
        run
    }

    fn secondary_row(id: &str, agent_id: &str, at_ms: u64) -> UsageRecord {
        crate::usage::usage_record(
            &crate::usage::UsageCall {
                id: id.into(),
                agent_id: agent_id.into(),
                session_id: Some("chat:one".into()),
                run_id: None,
                source: crate::usage::UsageSource::Title,
                provider: "anthropic".into(),
                model: "claude-fable-5-1".into(),
                duration_ms: 1,
                created_at_ms: at_ms,
            },
            &tokens(1, 1),
            &[],
        )
    }

    fn ids(rows: &[UsageRecord]) -> Vec<String> {
        let mut ids = rows.iter().map(|row| row.id.clone()).collect::<Vec<_>>();
        ids.sort();
        ids
    }

    #[tokio::test]
    async fn mirroring_a_run_writes_one_usage_row_per_step() {
        let store = Arc::new(MemoryHistoryStore::new());
        let (state, coordinator, agent_id) = state_with(HistoryService::new(store.clone())).await;
        let transactions = coordinator.control_plane_transactions();
        finished_run(
            &state,
            &agent_id,
            "run_a",
            "anthropic",
            "claude-fable-5-1",
            2,
        )
        .await;
        let history = state.read().await.history.clone();

        let report = history
            .flush_once(&state, &transactions, now_millis())
            .await
            .unwrap();

        assert_eq!((report.runs, report.usage), (1, 2));
        let rows = store.page_usage(&all_usage(&agent_id)).await.unwrap();
        assert_eq!(ids(&rows), ["run_a:1", "run_a:2"]);
        for row in &rows {
            assert_eq!(row.session_id.as_deref(), Some("chat:one"));
            assert_eq!(row.run_id.as_deref(), Some("run_a"));
            assert_eq!(row.source, crate::usage::UsageSource::Chat);
            assert_eq!(row.total_tokens, 12);
            assert_eq!(row.pricing_source, crate::usage::PricingSource::Table);
        }
    }

    #[tokio::test]
    async fn a_remirrored_run_does_not_duplicate_usage() {
        let store = Arc::new(MemoryHistoryStore::new());
        let (state, coordinator, agent_id) = state_with(HistoryService::new(store.clone())).await;
        let transactions = coordinator.control_plane_transactions();
        finished_run(
            &state,
            &agent_id,
            "run_a",
            "anthropic",
            "claude-fable-5-1",
            2,
        )
        .await;
        let history = state.read().await.history.clone();
        history
            .flush_once(&state, &transactions, now_millis())
            .await
            .unwrap();

        // A restart re-mirrors a run whose mirrored mark was not saved.
        state.write().await.runs.get_mut("run_a").unwrap().mirrored = false;
        let report = history
            .flush_once(&state, &transactions, now_millis())
            .await
            .unwrap();

        assert_eq!((report.runs, report.usage), (1, 2));
        assert_eq!(
            store.page_usage(&all_usage(&agent_id)).await.unwrap().len(),
            2,
            "the same ids are upserted, not added"
        );
    }

    #[tokio::test]
    async fn a_failed_usage_write_keeps_the_run_unmirrored_and_a_retry_succeeds() {
        let store = Arc::new(FlakyHistoryStore::new());
        let (state, coordinator, agent_id) = state_with(HistoryService::new(store.clone())).await;
        let transactions = coordinator.control_plane_transactions();
        finished_run(
            &state,
            &agent_id,
            "run_a",
            "anthropic",
            "claude-fable-5-1",
            2,
        )
        .await;
        let history = state.read().await.history.clone();
        store.set_usage_failing(true);

        assert!(history
            .flush_once(&state, &transactions, now_millis())
            .await
            .is_err());
        assert_eq!(state.read().await.unmirrored_terminal_runs(10).len(), 1);
        assert!(store.get_run("run_a").await.unwrap().is_none());

        store.set_usage_failing(false);
        let report = history
            .flush_once(&state, &transactions, now_millis())
            .await
            .unwrap();
        assert_eq!((report.runs, report.usage), (1, 2));
        assert!(state.read().await.unmirrored_terminal_runs(10).is_empty());
        assert!(store.get_run("run_a").await.unwrap().is_some());
        assert_eq!(
            store.page_usage(&all_usage(&agent_id)).await.unwrap().len(),
            2
        );
    }

    #[tokio::test]
    async fn secondary_usage_flushes_in_batches_of_500() {
        let store = Arc::new(FlakyHistoryStore::new());
        let (state, coordinator, agent_id) = state_with(HistoryService::new(store.clone())).await;
        let transactions = coordinator.control_plane_transactions();
        let history = state.read().await.history.clone();
        history.enqueue_usage(
            (0..1_201)
                .map(|index| secondary_row(&format!("usage_{index:04}"), &agent_id, 10 + index))
                .collect(),
        );
        assert_eq!(history.pending_usage().len(), 1_201);

        let report = history
            .flush_once(&state, &transactions, now_millis())
            .await
            .unwrap();

        assert_eq!(report.usage, 1_201);
        assert_eq!(store.usage_batches(), [500, 500, 201]);
        assert!(history.pending_usage().is_empty());
        assert_eq!(
            store.page_usage(&all_usage(&agent_id)).await.unwrap().len(),
            1_201
        );
    }

    #[tokio::test]
    async fn the_usage_queue_drops_the_oldest_past_its_bound() {
        let store = Arc::new(FlakyHistoryStore::new());
        let history = HistoryService::with_usage_capacity(store.clone(), 3);
        let mut daemon = DaemonState::new();
        daemon.set_history(Arc::clone(&history));
        let state = Arc::new(RwLock::new(daemon));
        let transactions = Mutex::new(());
        history.enqueue_usage(vec![
            secondary_row("usage_1", "agent-1", 1),
            secondary_row("usage_2", "agent-1", 2),
        ]);
        history.enqueue_usage(vec![
            secondary_row("usage_3", "agent-1", 3),
            secondary_row("usage_4", "agent-1", 4),
            secondary_row("usage_5", "agent-1", 5),
        ]);
        assert_eq!(
            ids(&history.pending_usage()),
            ["usage_3", "usage_4", "usage_5"]
        );

        store.set_usage_failing(true);
        assert!(history
            .flush_once(&state, &transactions, now_millis())
            .await
            .is_err());
        assert_eq!(
            history.pending_usage().len(),
            3,
            "a failed write keeps the rows queued"
        );

        store.set_usage_failing(false);
        let report = history
            .flush_once(&state, &transactions, now_millis())
            .await
            .unwrap();
        assert_eq!(report.usage, 3);
        assert_eq!(
            ids(&store.page_usage(&all_usage("agent-1")).await.unwrap()),
            ["usage_3", "usage_4", "usage_5"]
        );
    }

    #[tokio::test]
    async fn usage_rows_survive_deleting_the_session_and_the_agent() {
        let store = Arc::new(MemoryHistoryStore::new());
        let (state, coordinator, agent_id) = state_with(HistoryService::new(store.clone())).await;
        let transactions = coordinator.control_plane_transactions();
        finished_run(
            &state,
            &agent_id,
            "run_a",
            "anthropic",
            "claude-fable-5-1",
            2,
        )
        .await;
        let history = state.read().await.history.clone();
        history.enqueue_usage(vec![secondary_row("usage_title", &agent_id, 3_000)]);
        history
            .flush_once(&state, &transactions, now_millis())
            .await
            .unwrap();
        assert_eq!(
            store.page_usage(&all_usage(&agent_id)).await.unwrap().len(),
            3
        );

        history.enqueue_session_deletion(&agent_id, "chat:one");
        let report = history
            .flush_once(&state, &transactions, now_millis())
            .await
            .unwrap();
        assert_eq!(report.deletions, 1);
        history.enqueue_agent_deletion(&agent_id);
        let report = history
            .flush_once(&state, &transactions, now_millis())
            .await
            .unwrap();
        assert_eq!(report.deletions, 1);

        assert_eq!(
            store.page_usage(&all_usage(&agent_id)).await.unwrap().len(),
            3,
            "usage outlives its session and its agent"
        );
    }

    #[tokio::test]
    async fn usage_rows_use_the_overrides_at_mirror_time() {
        let store = Arc::new(MemoryHistoryStore::new());
        let (state, coordinator, agent_id) = state_with(HistoryService::new(store.clone())).await;
        let transactions = coordinator.control_plane_transactions();
        let run = finished_run(&state, &agent_id, "run_a", "openai", "gpt-x-1", 1).await;
        let at_mirror = vec![crate::usage::PricingOverride {
            provider: "openai".into(),
            model: "gpt-x".into(),
            input_micros_per_mtok: 1_000_000,
            output_micros_per_mtok: 4_000_000,
            cached_input_micros_per_mtok: None,
        }];
        // Set after the run ended and before its mirror: the mirror prices.
        state.write().await.set_pricing_overrides(at_mirror.clone());
        let history = state.read().await.history.clone();
        history
            .flush_once(&state, &transactions, now_millis())
            .await
            .unwrap();
        let expected =
            crate::usage::price_call("openai", "gpt-x-1", &run.steps[0].usage, &at_mirror).0;
        let stored = store.page_usage(&all_usage(&agent_id)).await.unwrap();
        assert_eq!(
            stored[0].pricing_source,
            crate::usage::PricingSource::Override
        );
        assert_eq!(stored[0].cost_micros, expected);
        assert_eq!(expected, Some(10 + 8));

        // Controller ruling 3: a re-mirror under other prices keeps the cost.
        state
            .write()
            .await
            .set_pricing_overrides(vec![crate::usage::PricingOverride {
                input_micros_per_mtok: 9_000_000,
                ..at_mirror[0].clone()
            }]);
        state.write().await.runs.get_mut("run_a").unwrap().mirrored = false;
        history
            .flush_once(&state, &transactions, now_millis())
            .await
            .unwrap();
        let again = store.page_usage(&all_usage(&agent_id)).await.unwrap();
        assert_eq!(again[0].cost_micros, expected);
    }

    #[tokio::test]
    async fn steps_past_the_cap_add_one_remainder_row() {
        let store = Arc::new(MemoryHistoryStore::new());
        let (state, coordinator, agent_id) = state_with(HistoryService::new(store.clone())).await;
        let transactions = coordinator.control_plane_transactions();
        let steps = crate::runs::MAX_RUN_STEPS as u64;
        let mut run = finished_run(
            &state,
            &agent_id,
            "run_a",
            "anthropic",
            "claude-fable-5-1",
            steps,
        )
        .await;
        // Three more model calls than the record keeps steps for.
        run.usage = tokens(10 * (steps + 3), 2 * (steps + 3));
        state.write().await.runs.insert(run.clone());
        let history = state.read().await.history.clone();

        let report = history
            .flush_once(&state, &transactions, now_millis())
            .await
            .unwrap();

        assert_eq!(report.usage, crate::runs::MAX_RUN_STEPS + 1);
        let rows = store.page_usage(&all_usage(&agent_id)).await.unwrap();
        assert_eq!(rows.len(), crate::runs::MAX_RUN_STEPS + 1);
        let rest = rows.iter().find(|row| row.id == "run_a:rest").unwrap();
        assert_eq!(rest.total_tokens, 36);
        assert_eq!(
            rows.iter().map(|row| row.total_tokens).sum::<u64>(),
            run.usage.total_tokens,
            "the rows add up to the run"
        );
    }

    // The hand-simulated agent-delete test that used to live here (Task 12) is
    // gone (fix round 1, M2 review): it never called `ConnectorManager::
    // delete_agent`, so it could not catch a regression in that method itself.
    // `connectors::runtime::tests::
    // deleting_an_agent_through_the_manager_is_durable_and_a_flush_clears_its_rows`
    // covers the same ground (store rows removed, runs not re-mirrored, an
    // unrelated agent's rows untouched, the pending entry cleared) through the
    // real method, plus a real JSON control-plane store and session-registry
    // cleanup this test never checked.
}
