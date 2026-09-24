//! Bounded in-memory history store for ephemeral mode (spec §13.1): each
//! table keeps at most `EPHEMERAL_HISTORY_MAX_ROWS` rows, dropping the oldest.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Mutex, MutexGuard};

use async_trait::async_trait;

use super::{
    search_tokens, text_matches, HistoryError, HistoryMessage, HistoryStore, MessagePageQuery,
    EPHEMERAL_HISTORY_MAX_ROWS,
};
use crate::runs::RunRecord;

pub(crate) struct MemoryHistoryStore {
    max_rows: usize,
    tables: Mutex<Tables>,
}

#[derive(Default)]
struct Tables {
    next_seq: u64,
    messages: HashMap<String, (u64, HistoryMessage)>,
    message_seqs: BTreeMap<u64, String>,
    runs: HashMap<String, (u64, RunRecord)>,
    run_seqs: BTreeMap<u64, String>,
}

impl MemoryHistoryStore {
    pub(crate) fn new() -> Self {
        Self::with_max_rows(EPHEMERAL_HISTORY_MAX_ROWS)
    }

    pub(crate) fn with_max_rows(max_rows: usize) -> Self {
        Self {
            max_rows: max_rows.max(1),
            tables: Mutex::new(Tables::default()),
        }
    }

    fn tables(&self) -> MutexGuard<'_, Tables> {
        self.tables
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Inserts or replaces a row by id; a new row evicts the oldest past the cap.
fn upsert<T>(
    rows: &mut HashMap<String, (u64, T)>,
    order: &mut BTreeMap<u64, String>,
    next_seq: &mut u64,
    id: String,
    value: T,
    max_rows: usize,
) {
    if let Some(existing) = rows.get_mut(&id) {
        existing.1 = value;
        return;
    }
    *next_seq += 1;
    order.insert(*next_seq, id.clone());
    rows.insert(id, (*next_seq, value));
    while rows.len() > max_rows {
        let Some((_, oldest)) = order.pop_first() else {
            break;
        };
        rows.remove(&oldest);
    }
}

fn remove_where<T>(
    rows: &mut HashMap<String, (u64, T)>,
    order: &mut BTreeMap<u64, String>,
    mut matches: impl FnMut(&T) -> bool,
) {
    let doomed = rows
        .iter()
        .filter(|(_, (_, value))| matches(value))
        .map(|(id, (seq, _))| (id.clone(), *seq))
        .collect::<Vec<_>>();
    for (id, seq) in doomed {
        rows.remove(&id);
        order.remove(&seq);
    }
}

#[async_trait]
impl HistoryStore for MemoryHistoryStore {
    fn label(&self) -> &'static str {
        "memory"
    }

    fn is_ephemeral(&self) -> bool {
        true
    }

    async fn upsert_messages(&self, messages: &[HistoryMessage]) -> Result<(), HistoryError> {
        let mut guard = self.tables();
        let tables = &mut *guard;
        for row in messages {
            upsert(
                &mut tables.messages,
                &mut tables.message_seqs,
                &mut tables.next_seq,
                row.message.id.clone(),
                row.clone(),
                self.max_rows,
            );
        }
        Ok(())
    }

    async fn upsert_runs(&self, runs: &[RunRecord]) -> Result<(), HistoryError> {
        let mut guard = self.tables();
        let tables = &mut *guard;
        for run in runs {
            upsert(
                &mut tables.runs,
                &mut tables.run_seqs,
                &mut tables.next_seq,
                run.id.clone(),
                run.clone(),
                self.max_rows,
            );
        }
        Ok(())
    }

    async fn existing_message_ids(&self, ids: &[String]) -> Result<HashSet<String>, HistoryError> {
        let tables = self.tables();
        Ok(ids
            .iter()
            .filter(|id| tables.messages.contains_key(*id))
            .cloned()
            .collect())
    }

    async fn get_message(
        &self,
        agent_id: &str,
        session_id: &str,
        message_id: &str,
    ) -> Result<Option<HistoryMessage>, HistoryError> {
        Ok(self
            .tables()
            .messages
            .get(message_id)
            .map(|(_, row)| row)
            .filter(|row| row.agent_id == agent_id && row.session_id == session_id)
            .cloned())
    }

    async fn get_run(&self, run_id: &str) -> Result<Option<RunRecord>, HistoryError> {
        Ok(self.tables().runs.get(run_id).map(|(_, run)| run.clone()))
    }

    async fn page_messages(
        &self,
        query: &MessagePageQuery,
    ) -> Result<Vec<HistoryMessage>, HistoryError> {
        let tables = self.tables();
        let mut rows = tables
            .messages
            .values()
            .map(|(_, row)| row)
            .filter(|row| {
                row.agent_id == query.agent_id
                    && row.session_id == query.session_id
                    && (query.include_hidden || !row.hidden)
                    && query
                        .before
                        .as_ref()
                        .is_none_or(|before| row.order() < *before)
            })
            .cloned()
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| right.order().cmp(&left.order()));
        rows.truncate(query.limit);
        Ok(rows)
    }

    async fn visible_message_counts(
        &self,
        agent_id: &str,
        session_ids: &[String],
    ) -> Result<HashMap<String, usize>, HistoryError> {
        let wanted = session_ids.iter().collect::<HashSet<_>>();
        let mut counts = HashMap::new();
        for (_, row) in self.tables().messages.values() {
            if row.agent_id == agent_id && !row.hidden && wanted.contains(&row.session_id) {
                *counts.entry(row.session_id.clone()).or_insert(0) += 1;
            }
        }
        Ok(counts)
    }

    async fn search_messages(
        &self,
        agent_ids: &[String],
        query: &str,
        limit: usize,
    ) -> Result<Vec<HistoryMessage>, HistoryError> {
        let tokens = search_tokens(query);
        if tokens.is_empty() || agent_ids.is_empty() {
            return Ok(Vec::new());
        }
        let tables = self.tables();
        let mut rows = tables
            .messages
            .values()
            .map(|(_, row)| row)
            .filter(|row| {
                !row.hidden
                    && agent_ids.contains(&row.agent_id)
                    && text_matches(&row.message.content.text, &tokens)
            })
            .cloned()
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| right.order().cmp(&left.order()));
        rows.truncate(limit);
        Ok(rows)
    }

    async fn delete_session(&self, agent_id: &str, session_id: &str) -> Result<(), HistoryError> {
        let mut guard = self.tables();
        let tables = &mut *guard;
        remove_where(&mut tables.messages, &mut tables.message_seqs, |row| {
            row.agent_id == agent_id && row.session_id == session_id
        });
        remove_where(&mut tables.runs, &mut tables.run_seqs, |run| {
            run.agent_id == agent_id && run.session_id == session_id
        });
        Ok(())
    }

    async fn delete_agent(&self, agent_id: &str) -> Result<(), HistoryError> {
        let mut guard = self.tables();
        let tables = &mut *guard;
        remove_where(&mut tables.messages, &mut tables.message_seqs, |row| {
            row.agent_id == agent_id
        });
        remove_where(&mut tables.runs, &mut tables.run_seqs, |run| {
            run.agent_id == agent_id
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::conformance::{assert_history_store_conformance, history_message};
    use anima_core::MessageRole;

    #[tokio::test]
    async fn memory_store_meets_the_conformance_suite() {
        assert_history_store_conformance(&MemoryHistoryStore::new()).await;
    }

    #[tokio::test]
    async fn ephemeral_tables_drop_the_oldest_rows_beyond_the_cap() {
        let store = MemoryHistoryStore::with_max_rows(2);
        let rows = (0..3u64)
            .map(|n| {
                history_message(
                    &format!("msg-{n}-{n}"),
                    "agent-1",
                    "chat:a",
                    MessageRole::User,
                    "hello",
                    n,
                )
            })
            .collect::<Vec<_>>();
        let all_ids = rows
            .iter()
            .map(|row| row.message.id.clone())
            .collect::<Vec<_>>();
        store.upsert_messages(&rows).await.unwrap();
        assert_eq!(
            store.existing_message_ids(&all_ids).await.unwrap(),
            HashSet::from(["msg-1-1".to_string(), "msg-2-2".to_string()])
        );

        // Rewriting a kept row neither makes it newer nor evicts another row.
        store.upsert_messages(&rows[1..2]).await.unwrap();
        assert_eq!(store.existing_message_ids(&all_ids).await.unwrap().len(), 2);
        assert!(store.is_ephemeral());
        assert_eq!(store.label(), "memory");
    }
}
