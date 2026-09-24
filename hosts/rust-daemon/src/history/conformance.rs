//! Behaviour every history store shares (memory, SQLite, Postgres).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anima_core::{Content, Message, MessageRole};
use async_trait::async_trait;
use tokio::sync::Semaphore;

use super::{HistoryError, HistoryMessage, HistoryStore, MemoryHistoryStore, MessagePageQuery};
use crate::runs::{RunRecord, RunSource, RunStart, RunStatus};

pub(crate) fn history_message(
    id: &str,
    agent_id: &str,
    session_id: &str,
    role: MessageRole,
    text: &str,
    created_at_ms: u64,
) -> HistoryMessage {
    HistoryMessage {
        agent_id: agent_id.into(),
        session_id: session_id.into(),
        hidden: false,
        message: Message {
            id: id.into(),
            agent_id: agent_id.into(),
            room_id: session_id.into(),
            content: Content {
                text: text.into(),
                ..Content::default()
            },
            role,
            created_at_ms,
        },
    }
}

pub(crate) fn terminal_run(agent_id: &str, session_id: &str) -> RunRecord {
    let mut run = RunRecord::running(
        RunStart {
            agent_id: agent_id.into(),
            session_id: session_id.into(),
            source: RunSource::Api,
            source_ref: None,
            idempotency_key: None,
            text: "Deploy the build tonight".into(),
            model: "test-model".into(),
            provider: None,
            parent_run_id: None,
        },
        100,
    );
    run.finish(RunStatus::Completed, None, 110);
    run
}

fn ids(rows: &[HistoryMessage]) -> Vec<String> {
    rows.iter().map(|row| row.message.id.clone()).collect()
}

fn page(
    agent_id: &str,
    session_id: &str,
    before: Option<&HistoryMessage>,
    limit: usize,
    include_hidden: bool,
) -> MessagePageQuery {
    MessagePageQuery {
        agent_id: agent_id.into(),
        session_id: session_id.into(),
        before: before.map(HistoryMessage::order),
        limit,
        include_hidden,
    }
}

/// Every store must pass this. Agent ids, timestamps, and message ids are
/// unique per call, so a shared Postgres database can run it repeatedly.
pub(crate) async fn assert_history_store_conformance(store: &dyn HistoryStore) {
    let agent = format!("agent-{}", uuid::Uuid::new_v4());
    let other = format!("agent-{}", uuid::Uuid::new_v4());
    let base = (uuid::Uuid::new_v4().as_u128() % 1_000_000_000) as u64 * 1_000;
    let at = |offset: u64| base + offset;
    let id = |offset: u64, ordinal: u64| format!("msg-{}-{ordinal}", base + offset);
    let mut silent = history_message(
        &id(300, 12),
        &agent,
        "chat:a",
        MessageRole::Assistant,
        "CHECKIN_OK deploy",
        at(300),
    );
    silent.hidden = true;
    // Two messages in the same millisecond keep their creation order through
    // the id counter (9 before 10, which string order would reverse).
    let rows = vec![
        history_message(
            &id(100, 9),
            &agent,
            "chat:a",
            MessageRole::User,
            "Deploy the build tonight",
            at(100),
        ),
        history_message(
            &id(100, 10),
            &agent,
            "chat:a",
            MessageRole::Assistant,
            "Build deployed.",
            at(100),
        ),
        history_message(
            &id(200, 11),
            &agent,
            "chat:a",
            MessageRole::User,
            "Thanks",
            at(200),
        ),
        silent,
        history_message(
            &id(150, 13),
            &agent,
            "chat:b",
            MessageRole::User,
            "deployment notes for later",
            at(150),
        ),
        history_message(
            &id(160, 14),
            &other,
            "chat:a",
            MessageRole::User,
            "deploy elsewhere",
            at(160),
        ),
    ];
    store.upsert_messages(&rows).await.expect("messages upsert");
    let mut edited = rows[2].clone();
    edited.message.content.text = "Thanks!".into();
    store
        .upsert_messages(&[edited.clone(), edited])
        .await
        .expect("an id written twice stays one row");

    let newest = store
        .page_messages(&page(&agent, "chat:a", None, 10, false))
        .await
        .unwrap();
    assert_eq!(ids(&newest), [id(200, 11), id(100, 10), id(100, 9)]);
    assert_eq!(newest[0].message.content.text, "Thanks!");
    assert_eq!(newest[0].agent_id, agent);
    assert_eq!(newest[0].session_id, "chat:a");
    let everything = store
        .page_messages(&page(&agent, "chat:a", None, 10, true))
        .await
        .unwrap();
    assert_eq!(
        ids(&everything),
        [id(300, 12), id(200, 11), id(100, 10), id(100, 9)]
    );
    assert!(everything[0].hidden);
    let second = store
        .page_messages(&page(&agent, "chat:a", Some(&rows[2]), 1, false))
        .await
        .unwrap();
    assert_eq!(ids(&second), [id(100, 10)]);
    let last = store
        .page_messages(&page(&agent, "chat:a", Some(&rows[1]), 5, false))
        .await
        .unwrap();
    assert_eq!(ids(&last), [id(100, 9)]);
    assert!(store
        .page_messages(&page(&agent, "chat:a", Some(&rows[0]), 5, false))
        .await
        .unwrap()
        .is_empty());

    let counts = store
        .visible_message_counts(
            &agent,
            &[
                "chat:a".to_string(),
                "chat:b".to_string(),
                "chat:none".to_string(),
            ],
        )
        .await
        .unwrap();
    assert_eq!(counts.get("chat:a"), Some(&3));
    assert_eq!(counts.get("chat:b"), Some(&1));
    assert_eq!(counts.get("chat:none").copied().unwrap_or(0), 0);
    assert!(store
        .visible_message_counts(&agent, &[])
        .await
        .unwrap()
        .is_empty());

    let agents = [agent.clone()];
    assert_eq!(
        ids(&store.search_messages(&agents, "deploy", 10).await.unwrap()),
        [id(150, 13), id(100, 10), id(100, 9)],
        "word prefixes, newest first, hidden rows and other agents excluded"
    );
    assert_eq!(
        ids(&store.search_messages(&agents, "DEPL", 10).await.unwrap()),
        [id(150, 13), id(100, 10), id(100, 9)]
    );
    assert_eq!(
        ids(&store
            .search_messages(&agents, "deploy tonight", 10)
            .await
            .unwrap()),
        [id(100, 9)],
        "every word must match"
    );
    assert_eq!(
        ids(&store.search_messages(&agents, "deploy", 1).await.unwrap()),
        [id(150, 13)]
    );
    assert!(store
        .search_messages(&agents, "!!", 10)
        .await
        .unwrap()
        .is_empty());
    assert!(store
        .search_messages(&[], "deploy", 10)
        .await
        .unwrap()
        .is_empty());
    let both = [agent.clone(), other.clone()];
    assert_eq!(
        ids(&store.search_messages(&both, "elsewhere", 10).await.unwrap()),
        [id(160, 14)]
    );

    // A fresh agent id keeps this block from perturbing any count or page
    // asserted above.
    let word_boundary_agent = format!("agent-{}", uuid::Uuid::new_v4());
    store
        .upsert_messages(&[
            history_message(
                &id(170, 15),
                &word_boundary_agent,
                "chat:a",
                MessageRole::User,
                "underdeployment fixed",
                at(170),
            ),
            history_message(
                &id(180, 16),
                &word_boundary_agent,
                "chat:a",
                MessageRole::User,
                "re-deploy tomorrow",
                at(180),
            ),
        ])
        .await
        .expect("word-boundary messages upsert");
    assert_eq!(
        ids(&store
            .search_messages(&[word_boundary_agent.clone()], "deploy", 10)
            .await
            .unwrap()),
        [id(180, 16)],
        "a mid-word occurrence must not match a word-prefix search"
    );

    let known = store
        .existing_message_ids(&[id(100, 9), format!("missing-{base}")])
        .await
        .unwrap();
    assert_eq!(known, HashSet::from([id(100, 9)]));
    assert!(store.existing_message_ids(&[]).await.unwrap().is_empty());
    assert_eq!(
        store
            .get_message(&agent, "chat:a", &id(100, 10))
            .await
            .unwrap()
            .map(|row| row.message.content.text),
        Some("Build deployed.".to_string())
    );
    assert_eq!(
        store
            .get_message(&agent, "chat:b", &id(100, 10))
            .await
            .unwrap(),
        None,
        "a message belongs to one session"
    );

    let run = terminal_run(&agent, "chat:a");
    let kept_run = terminal_run(&agent, "chat:b");
    store
        .upsert_runs(&[run.clone(), kept_run.clone()])
        .await
        .unwrap();
    store.upsert_runs(std::slice::from_ref(&run)).await.unwrap();
    assert_eq!(store.get_run(&run.id).await.unwrap(), Some(run.clone()));

    store.delete_session(&agent, "chat:a").await.unwrap();
    assert!(store
        .page_messages(&page(&agent, "chat:a", None, 10, true))
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        ids(&store.search_messages(&agents, "deploy", 10).await.unwrap()),
        [id(150, 13)]
    );
    assert_eq!(store.get_run(&run.id).await.unwrap(), None);
    assert_eq!(
        store.get_run(&kept_run.id).await.unwrap(),
        Some(kept_run.clone())
    );
    assert_eq!(
        store
            .page_messages(&page(&other, "chat:a", None, 10, false))
            .await
            .unwrap()
            .len(),
        1,
        "other agents' sessions are untouched"
    );
    store
        .delete_session(&agent, "chat:a")
        .await
        .expect("deleting twice is harmless");

    // Deleting an agent removes the rows of every one of its sessions.
    let doomed = format!("agent-{}", uuid::Uuid::new_v4());
    store
        .upsert_messages(&[
            history_message(
                &id(400, 17),
                &doomed,
                "chat:a",
                MessageRole::User,
                "farewell",
                at(400),
            ),
            history_message(
                &id(410, 18),
                &doomed,
                "chat:b",
                MessageRole::User,
                "farewell again",
                at(410),
            ),
        ])
        .await
        .expect("the doomed agent's messages upsert");
    let doomed_run = terminal_run(&doomed, "chat:b");
    store
        .upsert_runs(std::slice::from_ref(&doomed_run))
        .await
        .unwrap();
    store.delete_agent(&doomed).await.unwrap();
    assert!(store
        .existing_message_ids(&[id(400, 17), id(410, 18)])
        .await
        .unwrap()
        .is_empty());
    assert_eq!(store.get_run(&doomed_run.id).await.unwrap(), None);
    assert_eq!(
        store
            .page_messages(&page(&agent, "chat:b", None, 10, false))
            .await
            .unwrap()
            .len(),
        1,
        "another agent's rows survive"
    );
    assert_eq!(
        store.get_run(&kept_run.id).await.unwrap(),
        Some(kept_run),
        "another agent's runs survive"
    );
    store
        .delete_agent(&doomed)
        .await
        .expect("deleting an agent twice is harmless");
}

/// Session search (Controller ruling 2, M2 pre-flight audit): sessions are
/// ranked by their newest matching message, so a session whose only match is
/// older than another session's flood of matches is still returned. Agent id
/// and timestamps are unique per call, so a shared Postgres database can run
/// it repeatedly.
pub(crate) async fn assert_history_store_session_search_conformance(store: &dyn HistoryStore) {
    let agent = format!("agent-{}", uuid::Uuid::new_v4());
    let base = (uuid::Uuid::new_v4().as_u128() % 1_000_000_000) as u64 * 1_000;
    let at = |offset: u64| base + offset;
    let id = |offset: u64, ordinal: u64| format!("msg-{}-{ordinal}", base + offset);

    // "chat:busy" gets 4 matches, more than the `limit: 3` passed below, so a
    // row-limited search (the pre-ruling behaviour: take the newest `limit`
    // *rows*, then group) would fill its whole budget with "chat:busy" rows
    // alone. "chat:quiet" has a single, older match that must still surface
    // once results are grouped into sessions before the limit is applied.
    let mut rows = (0u64..4)
        .map(|n| {
            history_message(
                &id(100 + n * 10, 20 + n),
                &agent,
                "chat:busy",
                MessageRole::User,
                "deploy the build",
                at(100 + n * 10),
            )
        })
        .collect::<Vec<_>>();
    rows.push(history_message(
        &id(10, 30),
        &agent,
        "chat:quiet",
        MessageRole::User,
        "deploy notes",
        at(10),
    ));
    let mut hidden = history_message(
        &id(500, 40),
        &agent,
        "chat:busy",
        MessageRole::Assistant,
        "deploy hidden",
        at(500),
    );
    hidden.hidden = true;
    rows.push(hidden);
    store
        .upsert_messages(&rows)
        .await
        .expect("session-search messages upsert");

    let newest_busy = id(130, 23);
    let quiet = id(10, 30);
    let agents = [agent.clone()];
    let found = store.search_sessions(&agents, "deploy", 3).await.unwrap();
    assert_eq!(
        found
            .iter()
            .map(|row| (row.session_id.as_str(), row.message.id.as_str()))
            .collect::<Vec<_>>(),
        [
            ("chat:busy", newest_busy.as_str()),
            ("chat:quiet", quiet.as_str())
        ],
        "a session with more than the limit's worth of matches must not crowd out an older session's match"
    );
    assert!(
        found.iter().all(|row| !row.hidden),
        "hidden messages never match"
    );

    assert_eq!(
        ids(&store.search_sessions(&agents, "deploy", 1).await.unwrap()),
        [newest_busy],
        "the limit caps the number of sessions, newest session first"
    );
    assert!(store
        .search_sessions(&[], "deploy", 3)
        .await
        .unwrap()
        .is_empty());
    assert!(store
        .search_sessions(&agents, "!!", 3)
        .await
        .unwrap()
        .is_empty());
}

/// A memory store whose every call fails while `failing` is set. Unlike the
/// memory store it is not ephemeral, so pruning tests can use it.
pub(crate) struct FlakyHistoryStore {
    inner: MemoryHistoryStore,
    failing: AtomicBool,
    panic_on_write: AtomicBool,
    existence_gate: Mutex<Option<StoreGate>>,
}

/// Holds one store call: the call adds a permit to `entered`, then waits for
/// one on `release`.
#[derive(Clone)]
pub(crate) struct StoreGate {
    pub(crate) entered: Arc<Semaphore>,
    pub(crate) release: Arc<Semaphore>,
}

impl FlakyHistoryStore {
    pub(crate) fn new() -> Self {
        Self {
            inner: MemoryHistoryStore::new(),
            failing: AtomicBool::new(false),
            panic_on_write: AtomicBool::new(false),
            existence_gate: Mutex::new(None),
        }
    }

    pub(crate) fn set_failing(&self, failing: bool) {
        self.failing.store(failing, Ordering::SeqCst);
    }

    /// Makes the next `upsert_messages` call panic.
    pub(crate) fn panic_on_next_write(&self) {
        self.panic_on_write.store(true, Ordering::SeqCst);
    }

    /// Holds the next `existing_message_ids` call (a reconcile's store round
    /// trip) at a gate.
    pub(crate) fn hold_next_existence_check(&self) -> StoreGate {
        let gate = StoreGate {
            entered: Arc::new(Semaphore::new(0)),
            release: Arc::new(Semaphore::new(0)),
        };
        *self
            .existence_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(gate.clone());
        gate
    }

    fn check(&self) -> Result<(), HistoryError> {
        if self.failing.load(Ordering::SeqCst) {
            Err(HistoryError::new("injected history store failure"))
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl HistoryStore for FlakyHistoryStore {
    fn label(&self) -> &'static str {
        "flaky"
    }

    async fn upsert_messages(&self, messages: &[HistoryMessage]) -> Result<(), HistoryError> {
        self.check()?;
        if self.panic_on_write.swap(false, Ordering::SeqCst) {
            panic!("injected history store panic");
        }
        self.inner.upsert_messages(messages).await
    }

    async fn upsert_runs(&self, runs: &[RunRecord]) -> Result<(), HistoryError> {
        self.check()?;
        self.inner.upsert_runs(runs).await
    }

    async fn existing_message_ids(&self, ids: &[String]) -> Result<HashSet<String>, HistoryError> {
        self.check()?;
        let gate = self
            .existence_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(gate) = gate {
            gate.entered.add_permits(1);
            gate.release
                .acquire()
                .await
                .expect("the store gate stays open")
                .forget();
        }
        self.inner.existing_message_ids(ids).await
    }

    async fn get_message(
        &self,
        agent_id: &str,
        session_id: &str,
        message_id: &str,
    ) -> Result<Option<HistoryMessage>, HistoryError> {
        self.check()?;
        self.inner
            .get_message(agent_id, session_id, message_id)
            .await
    }

    async fn get_run(&self, run_id: &str) -> Result<Option<RunRecord>, HistoryError> {
        self.check()?;
        self.inner.get_run(run_id).await
    }

    async fn page_messages(
        &self,
        query: &MessagePageQuery,
    ) -> Result<Vec<HistoryMessage>, HistoryError> {
        self.check()?;
        self.inner.page_messages(query).await
    }

    async fn visible_message_counts(
        &self,
        agent_id: &str,
        session_ids: &[String],
    ) -> Result<HashMap<String, usize>, HistoryError> {
        self.check()?;
        self.inner
            .visible_message_counts(agent_id, session_ids)
            .await
    }

    async fn search_messages(
        &self,
        agent_ids: &[String],
        query: &str,
        limit: usize,
    ) -> Result<Vec<HistoryMessage>, HistoryError> {
        self.check()?;
        self.inner.search_messages(agent_ids, query, limit).await
    }

    async fn search_sessions(
        &self,
        agent_ids: &[String],
        query: &str,
        limit: usize,
    ) -> Result<Vec<HistoryMessage>, HistoryError> {
        self.check()?;
        self.inner.search_sessions(agent_ids, query, limit).await
    }

    async fn delete_session(&self, agent_id: &str, session_id: &str) -> Result<(), HistoryError> {
        self.check()?;
        self.inner.delete_session(agent_id, session_id).await
    }

    async fn delete_agent(&self, agent_id: &str) -> Result<(), HistoryError> {
        self.check()?;
        self.inner.delete_agent(agent_id).await
    }
}
