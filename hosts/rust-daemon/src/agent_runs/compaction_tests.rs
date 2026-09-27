//! Automatic compaction before a run (spec §5.4).

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anima_core::{
    AgentConfig, AgentStatus, Content, DataValue, Message, MessageRole, ModelAdapter,
    ModelGenerateRequest, ModelGenerateResponse, ModelStreamSink, RuntimeRunDelta, TokenUsage,
};
use async_trait::async_trait;

use super::compact::RunCompaction;
use super::test_support::{
    chat_request, companion_config, events_until, is_terminal, quiet_for, ScriptedModel, Step,
};
use super::AgentRunCoordinator;
use crate::app::SharedDaemonState;
use crate::history::conformance::FlakyHistoryStore;
use crate::history::{HistoryMessage, HistoryService};
use crate::live::LiveRun;
use crate::runs::{RunRecord, RunSource, RunStart, RunStatus};
use crate::sessions::{
    SessionKind, SessionOrigin, SessionPrunedThrough, SessionRecord, TitleSource,
};
use crate::state::{DaemonState, RunBuild};

/// A 26-token summary: with it, the newer hot turn (22) no longer fits.
const LONG_SUMMARY: &str = "They planned a trip to Lisbon in May and asked for two hotel options.";

/// A hot message of `chat:long`, newer than every pruned one below.
fn message(agent_id: &str, id: &str, role: MessageRole, text: &str) -> Message {
    Message {
        id: id.into(),
        agent_id: agent_id.into(),
        room_id: "chat:long".into(),
        content: Content {
            text: text.into(),
            ..Content::default()
        },
        role,
        created_at_ms: 1_000,
    }
}

/// Two hot turns in `chat:long`: 23 tokens ("plan a trip" 11 + "Lisbon in
/// May" 12), then 22 ("and hotels?" 11 + "Two options" 11), in an agent
/// whose context is `budget` tokens with a 50-token reply reserve.
async fn session(
    model: Arc<dyn ModelAdapter>,
    budget: f64,
    auto: bool,
) -> (AgentRunCoordinator, String) {
    let mut config = companion_config("companion");
    let settings = config.settings.as_mut().unwrap();
    settings.max_tokens = Some(50);
    settings
        .additional
        .insert("contextBudgetTokens".into(), DataValue::Number(budget));
    if !auto {
        settings
            .additional
            .insert("autoCompact".into(), DataValue::Bool(false));
    }
    let mut state = DaemonState::with_model_adapter(model);
    let agent_id = state.create_agent(config).unwrap().state.id;
    state.sessions.insert(SessionRecord::new(
        &agent_id,
        "chat:long",
        SessionKind::Chat,
        SessionOrigin::Web,
        "Trip".into(),
        TitleSource::Owner,
        1,
    ));
    state
        .agents
        .get_mut(&agent_id)
        .unwrap()
        .apply_run_delta(&RuntimeRunDelta {
            messages: vec![
                message(&agent_id, "u1", MessageRole::User, "plan a trip"),
                message(&agent_id, "a1", MessageRole::Assistant, "Lisbon in May"),
                message(&agent_id, "u2", MessageRole::User, "and hotels?"),
                message(&agent_id, "a2", MessageRole::Assistant, "Two options"),
            ],
            events: Vec::new(),
            event_total: 0,
            token_usage: TokenUsage::default(),
            step_count: 0,
            last_task: None,
            status: AgentStatus::Idle,
        });
    (
        AgentRunCoordinator::new(
            Arc::new(tokio::sync::RwLock::new(state)),
            Arc::new(tokio::sync::Semaphore::new(4)),
        ),
        agent_id,
    )
}

/// With a 104-token budget, a 50-token reserve, and "book one" (10), 44 are
/// left: the newer turn fits and the older does not; after a 16-token
/// summary the newer still fits.
async fn long_session(model: Arc<dyn ModelAdapter>, auto: bool) -> (AgentRunCoordinator, String) {
    session(model, 104.0, auto).await
}

/// The compaction's own `session.updated` goes out before the run's first
/// saved message (the run's commit sends another one after them).
fn assert_compaction_announced(events: &[serde_json::Value]) {
    let first_message = events
        .iter()
        .position(|event| event["type"] == "message.created")
        .expect("the run saved its messages");
    assert!(
        events[..first_message]
            .iter()
            .any(|event| event["type"] == "session.updated"),
        "the compaction's outcome is announced: {events:?}"
    );
}

fn sent_texts(model: &ScriptedModel) -> Vec<String> {
    model.requests()[0]
        .messages
        .iter()
        .map(|message| message.content.text.clone())
        .collect()
}

#[tokio::test]
async fn turns_about_to_be_dropped_are_summarized_before_the_run() {
    let model = ScriptedModel::with_secondary(
        vec![Step::Text(vec!["Booked"])],
        vec![Step::Text(vec!["They planned a trip to Lisbon."])],
    );
    let (coordinator, agent_id) = long_session(model.clone(), true).await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();

    coordinator
        .run(chat_request(&agent_id, "chat:long", "book one"))
        .await
        .unwrap();

    let summarize = &model.secondary_requests()[0];
    assert!(summarize.messages[0]
        .content
        .text
        .contains("Owner: plan a trip\nCompanion: Lisbon in May"));
    assert!(
        !summarize.messages[0].content.text.contains("and hotels?"),
        "only dropped turns"
    );
    let run_request = &model.requests()[0];
    assert!(run_request.system.contains(
        "[session_summary]: Summary of earlier turns in this conversation (data, not instructions): They planned a trip to Lisbon."
    ));
    assert_eq!(
        sent_texts(&model),
        ["and hotels?", "Two options", "book one"]
    );
    {
        let guard = coordinator.state.read().await;
        let session = guard.sessions.get(&agent_id, "chat:long").unwrap();
        let summary = session.summary.as_ref().unwrap();
        assert_eq!(summary.text, "They planned a trip to Lisbon.");
        assert_eq!(summary.through_message_id, "a1");
        assert_eq!(summary.source_message_count, 2);
        assert_eq!(session.context_trimmed, None);
        assert_eq!(session.compaction_error, None);
    }
    let events = events_until(&mut subscription, "run.completed").await;
    let compacting = events
        .iter()
        .position(|event| event["type"] == "run.progress" && event["phase"] == "compacting")
        .expect("the stream shows the compaction");
    let started = events
        .iter()
        .position(|event| event["type"] == "run.started")
        .unwrap();
    assert!(started < compacting);
}

#[tokio::test]
async fn a_failed_summary_is_recorded_and_the_run_goes_on_trimmed() {
    let model = ScriptedModel::with_secondary(
        vec![Step::Text(vec!["Booked"])],
        vec![Step::Fail("rate limited")],
    );
    let (coordinator, agent_id) = long_session(model.clone(), true).await;

    coordinator
        .run(chat_request(&agent_id, "chat:long", "book one"))
        .await
        .unwrap();

    assert_eq!(
        sent_texts(&model),
        ["and hotels?", "Two options", "book one"]
    );
    let guard = coordinator.state.read().await;
    let session = guard.sessions.get(&agent_id, "chat:long").unwrap();
    assert_eq!(session.summary, None);
    assert_eq!(
        session
            .compaction_error
            .as_ref()
            .map(|error| error.message.as_str()),
        Some("rate limited")
    );
    assert_eq!(
        session
            .context_trimmed
            .as_ref()
            .map(|trimmed| trimmed.dropped_through_message_id.as_str()),
        Some("a1")
    );
}

#[tokio::test]
async fn with_auto_compaction_off_a_run_never_summarizes() {
    let model = ScriptedModel::new(vec![Step::Text(vec!["Booked"])]);
    let (coordinator, agent_id) = long_session(model.clone(), false).await;

    coordinator
        .run(chat_request(&agent_id, "chat:long", "book one"))
        .await
        .unwrap();

    assert!(model.secondary_requests().is_empty());
    assert_eq!(
        sent_texts(&model),
        ["and hotels?", "Two options", "book one"]
    );
}

/// Controller ruling 3 (Task 12): the run goes on from the rebuilt
/// selection, and what that selection still leaves out is the session's
/// `contextTrimmed`, set in the summary's own save.
#[tokio::test]
async fn what_the_rebuilt_context_still_leaves_out_is_saved_with_the_summary() {
    let model = ScriptedModel::with_secondary(
        vec![Step::Text(vec!["Booked"])],
        vec![Step::Text(vec![LONG_SUMMARY])],
    );
    let (coordinator, agent_id) = long_session(model.clone(), true).await;

    coordinator
        .run(chat_request(&agent_id, "chat:long", "book one"))
        .await
        .unwrap();

    assert_eq!(
        sent_texts(&model),
        ["book one"],
        "the long summary leaves no room for the newer turn"
    );
    assert!(model.requests()[0].system.contains(LONG_SUMMARY));
    let guard = coordinator.state.read().await;
    let session = guard.sessions.get(&agent_id, "chat:long").unwrap();
    assert_eq!(
        session
            .summary
            .as_ref()
            .map(|summary| summary.through_message_id.as_str()),
        Some("a1")
    );
    assert_eq!(
        session
            .context_trimmed
            .as_ref()
            .map(|trimmed| trimmed.dropped_through_message_id.as_str()),
        Some("a2")
    );
    assert_eq!(session.compaction_error, None);
}

/// Streams like its `ScriptedModel`; its summarizing call first runs `hook`
/// on the daemon state (with no other lock held, as compaction calls it).
struct BeforeSummary {
    inner: Arc<ScriptedModel>,
    state: OnceLock<SharedDaemonState>,
    hook: fn(&mut DaemonState),
}

impl BeforeSummary {
    fn new(inner: Arc<ScriptedModel>, hook: fn(&mut DaemonState)) -> Arc<Self> {
        Arc::new(Self {
            inner,
            state: OnceLock::new(),
            hook,
        })
    }
}

#[async_trait]
impl ModelAdapter for BeforeSummary {
    fn provider(&self) -> &str {
        "before-summary"
    }

    async fn generate(
        &self,
        config: &AgentConfig,
        request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String> {
        let state = self.state.get().expect("the test sets the state");
        (self.hook)(&mut *state.write().await);
        self.inner.generate(config, request).await
    }

    async fn stream(
        &self,
        config: &AgentConfig,
        request: &ModelGenerateRequest,
        sink: &dyn ModelStreamSink,
    ) -> Result<(), String> {
        self.inner.stream(config, request, sink).await
    }
}

/// Spec §5.4: a summary whose save fails is an error on the session, and
/// the summary and the trimmed indicator it would have changed stay as the
/// run's start saved them.
#[tokio::test]
async fn a_summary_that_cannot_be_saved_is_recorded_and_the_run_goes_on_trimmed() {
    let adapter = BeforeSummary::new(
        ScriptedModel::with_secondary(
            vec![Step::Text(vec!["Booked"])],
            vec![Step::Text(vec!["They planned a trip to Lisbon."])],
        ),
        // The next save, the summary's, fails.
        |state| {
            state
                .install_test_control_plane_save_gate(true)
                .release
                .add_permits(1);
        },
    );
    let (coordinator, agent_id) = long_session(adapter.clone(), true).await;
    assert!(adapter.state.set(Arc::clone(&coordinator.state)).is_ok());
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();

    coordinator
        .run(chat_request(&agent_id, "chat:long", "book one"))
        .await
        .unwrap();

    // Fix round 1: the unsaved error is announced at once, not at the next save.
    assert_compaction_announced(&events_until(&mut subscription, "run.completed").await);
    assert_eq!(
        sent_texts(&adapter.inner),
        ["and hotels?", "Two options", "book one"]
    );
    assert!(!adapter.inner.requests()[0]
        .system
        .contains("[session_summary]"));
    let guard = coordinator.state.read().await;
    let session = guard.sessions.get(&agent_id, "chat:long").unwrap();
    assert_eq!(session.summary, None);
    assert_eq!(
        session
            .context_trimmed
            .as_ref()
            .map(|trimmed| trimmed.dropped_through_message_id.as_str()),
        Some("a1")
    );
    assert_eq!(
        session
            .compaction_error
            .as_ref()
            .map(|error| error.message.as_str()),
        Some("The summary could not be saved: injected control-plane save failure")
    );
}

/// Spec §4.4: a configuration change while the summary is written applies
/// to later runs; the rebuilt run keeps the configuration it started with.
#[tokio::test]
async fn a_rebuilt_run_keeps_the_configuration_it_started_with() {
    let adapter = BeforeSummary::new(
        ScriptedModel::with_secondary(
            vec![Step::Text(vec!["Booked"]), Step::Text(vec!["Done"])],
            vec![Step::Text(vec!["They planned a trip to Lisbon."])],
        ),
        |state| {
            for runtime in state.agents.values_mut() {
                runtime.update_config(anima_core::AgentConfigUpdate {
                    system: Some("Answer in French.".into()),
                    ..Default::default()
                });
            }
        },
    );
    let (coordinator, agent_id) = long_session(adapter.clone(), true).await;
    assert!(adapter.state.set(Arc::clone(&coordinator.state)).is_ok());

    coordinator
        .run(chat_request(&agent_id, "chat:long", "book one"))
        .await
        .unwrap();
    coordinator
        .run(chat_request(&agent_id, "chat:long", "and then?"))
        .await
        .unwrap();

    let requests = adapter.inner.requests();
    assert!(
        requests[0]
            .system
            .contains("They planned a trip to Lisbon."),
        "the run was rebuilt from the summary"
    );
    assert!(!requests[0].system.contains("Answer in French."));
    assert!(
        requests[1].system.contains("Answer in French."),
        "the next run has the change"
    );
}

/// Controller ruling 3 (audit M7): a stop while the summary is being
/// written ends the run at once, stopped, and changes nothing else.
#[tokio::test]
async fn a_stop_during_compaction_ends_the_run_at_once_as_stopped() {
    let model = ScriptedModel::with_secondary(
        vec![Step::Text(vec!["Booked"])],
        vec![Step::Hold(Vec::new())],
    );
    let (coordinator, agent_id) = long_session(model.clone(), true).await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();
    let running = {
        let coordinator = coordinator.clone();
        let request = chat_request(&agent_id, "chat:long", "book one");
        tokio::spawn(async move { coordinator.run(request).await })
    };
    for _ in 0..500 {
        if !model.secondary_requests().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        model.secondary_requests().len(),
        1,
        "the summary is underway"
    );
    let run_id = coordinator
        .state
        .read()
        .await
        .runs
        .active_records()
        .into_iter()
        .find(|record| record.agent_id == agent_id)
        .map(|record| record.id.clone())
        .unwrap();

    coordinator.stop_run(&agent_id, &run_id).await.unwrap();

    let envelope = tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .expect("the run ends at once")
        .unwrap()
        .unwrap();
    assert_eq!(envelope.result.error.as_deref(), Some("stopped"));
    assert!(
        model.requests().is_empty(),
        "the model is never asked to reply"
    );
    {
        let guard = coordinator.state.read().await;
        assert_eq!(
            guard.runs.get(&run_id).unwrap().status,
            RunStatus::Cancelled
        );
        let session = guard.sessions.get(&agent_id, "chat:long").unwrap();
        assert_eq!(session.summary, None);
        assert_eq!(session.compaction_error, None);
        assert_eq!(
            session
                .context_trimmed
                .as_ref()
                .map(|trimmed| trimmed.dropped_through_message_id.as_str()),
            Some("a1")
        );
    }
    let mut events = events_until(&mut subscription, "run.cancelled").await;
    events.extend(quiet_for(&mut subscription).await);
    assert!(events
        .iter()
        .any(|event| event["type"] == "run.progress" && event["phase"] == "compacting"));
    assert_eq!(
        events.iter().filter(|event| is_terminal(event)).count(),
        1,
        "one terminal event: {events:?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event["type"] == "session.updated")
            .count(),
        1,
        "only the run's commit updates the session: {events:?}"
    );
}

/// Fix round 1 (Important 2): a summary still unwritten at its deadline is
/// a failed compaction, saved and announced; the run goes on trimmed.
#[tokio::test]
async fn a_summary_past_its_deadline_is_recorded_and_the_run_goes_on_trimmed() {
    let model = ScriptedModel::with_secondary(
        vec![Step::Text(vec!["Booked"])],
        vec![Step::Hold(Vec::new())],
    );
    let (coordinator, agent_id) = long_session(model.clone(), true).await;
    let coordinator = coordinator.with_compaction_timeout(Duration::from_millis(50));
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();

    tokio::time::timeout(
        Duration::from_secs(5),
        coordinator.run(chat_request(&agent_id, "chat:long", "book one")),
    )
    .await
    .expect("the deadline ends the summary")
    .unwrap();

    assert_eq!(
        sent_texts(&model),
        ["and hotels?", "Two options", "book one"]
    );
    {
        let guard = coordinator.state.read().await;
        let session = guard.sessions.get(&agent_id, "chat:long").unwrap();
        assert_eq!(session.summary, None);
        assert_eq!(
            session
                .compaction_error
                .as_ref()
                .map(|error| error.message.as_str()),
            Some("The summary took longer than 2 minutes")
        );
        assert_eq!(
            session
                .context_trimmed
                .as_ref()
                .map(|trimmed| trimmed.dropped_through_message_id.as_str()),
            Some("a1")
        );
    }
    assert_compaction_announced(&events_until(&mut subscription, "run.completed").await);
}

/// What `run_locked` hands `compact_before_run` for "book one" in
/// `chat:long`: the run's build, its registered live run, and its record.
async fn run_parts(
    coordinator: &AgentRunCoordinator,
    agent_id: &str,
    input: &Content,
) -> (RunBuild, LiveRun, RunRecord) {
    let (build, hub) = {
        let guard = coordinator.state.read().await;
        (
            guard
                .build_run_runtime(agent_id, "chat:long", input)
                .unwrap(),
            guard.live.clone(),
        )
    };
    let record = RunRecord::running(
        RunStart {
            agent_id: agent_id.into(),
            session_id: "chat:long".into(),
            source: RunSource::Api,
            source_ref: None,
            idempotency_key: None,
            text: input.text.clone(),
            model: "gpt-5.4".into(),
            provider: Some("openai".into()),
            parent_run_id: None,
        },
        1,
    );
    let live_run = LiveRun::register(hub, &record, None);
    (build, live_run, record)
}

fn book_one() -> Content {
    Content {
        text: "book one".into(),
        ..Content::default()
    }
}

/// Fix round 1 (Important 1): a helper's compaction ends by the helper's
/// own deadline when that comes first, so the summary counts toward the
/// helper's two minutes; its execution then runs to the same deadline.
#[tokio::test]
async fn a_helper_deadline_ends_its_compaction() {
    let model = ScriptedModel::with_secondary(vec![], vec![Step::Hold(Vec::new())]);
    let (coordinator, agent_id) = long_session(model, true).await;
    let input = book_one();
    let (build, live_run, record) = run_parts(&coordinator, &agent_id, &input).await;
    let mut context = build.context;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(50);

    let rebuilt = tokio::time::timeout(
        Duration::from_secs(5),
        coordinator.compact_before_run(
            &live_run,
            &record,
            RunCompaction {
                room_id: "chat:long",
                input: &input,
                config: build.runtime.config(),
                deadline: Some(deadline),
            },
            &mut context,
        ),
    )
    .await
    .expect("the helper's deadline ends the summary");

    assert!(rebuilt.is_none(), "the run goes on as it was built");
    assert!(tokio::time::Instant::now() >= deadline);
    let guard = coordinator.state.read().await;
    let session = guard.sessions.get(&agent_id, "chat:long").unwrap();
    assert_eq!(session.summary, None);
    assert_eq!(
        session
            .compaction_error
            .as_ref()
            .map(|error| error.message.as_str()),
        Some("The summary took longer than 2 minutes")
    );
}

/// Task 11 review carry (controller ruling 4): a run lets go of its copy
/// of the dropped turns once its compaction is decided or done, and the
/// rebuilt selection's copy too.
#[tokio::test]
async fn a_run_lets_go_of_its_dropped_turns_once_compaction_is_decided() {
    for auto in [false, true] {
        let model = ScriptedModel::with_secondary(vec![], vec![Step::Text(vec![LONG_SUMMARY])]);
        let (coordinator, agent_id) = long_session(model, auto).await;
        let input = book_one();
        let (build, live_run, record) = run_parts(&coordinator, &agent_id, &input).await;
        let mut context = build.context;
        assert_eq!(context.dropped.len(), 2, "auto {auto}");

        let rebuilt = coordinator
            .compact_before_run(
                &live_run,
                &record,
                RunCompaction {
                    room_id: "chat:long",
                    input: &input,
                    config: build.runtime.config(),
                    deadline: None,
                },
                &mut context,
            )
            .await;

        assert!(context.dropped.is_empty(), "auto {auto}");
        match rebuilt {
            None => assert!(!auto, "a compaction rebuilds the run"),
            Some(rebuilt) => {
                assert!(auto, "no compaction, no rebuild");
                assert_eq!(rebuilt.context.trimmed_through.as_deref(), Some("a2"));
                assert!(rebuilt.context.dropped.is_empty());
            }
        }
    }
}

/// A pruned message of `chat:long` that only the history store holds.
fn pruned(
    agent_id: &str,
    id: &str,
    role: MessageRole,
    text: &str,
    created_at_ms: u64,
    metadata: Option<(&str, DataValue)>,
) -> HistoryMessage {
    let mut message = message(agent_id, id, role, text);
    message.created_at_ms = created_at_ms;
    message.content.metadata =
        metadata.map(|(key, value)| BTreeMap::from([(key.to_string(), value)]));
    HistoryMessage {
        agent_id: agent_id.into(),
        session_id: "chat:long".into(),
        hidden: false,
        message,
    }
}

/// Turns of `chat:long` that pruning moved to the history store, older than
/// its hot turns and marked on the session through `p6`, with a silent
/// check-in pair, a revised draft, and the evaluator's request to revise it
/// among them.
async fn seed_pruned(coordinator: &AgentRunCoordinator, agent_id: &str) {
    let mut checkin = pruned(
        agent_id,
        "c1",
        MessageRole::User,
        "Check status",
        3,
        Some(("kind", DataValue::String("checkin".into()))),
    );
    checkin.hidden = true;
    let mut silent = pruned(
        agent_id,
        "c2",
        MessageRole::Assistant,
        "CHECKIN_OK",
        4,
        None,
    );
    silent.hidden = true;
    let rows = [
        pruned(agent_id, "p1", MessageRole::User, "where to?", 1, None),
        pruned(
            agent_id,
            "p2",
            MessageRole::Assistant,
            "Somewhere warm",
            2,
            None,
        ),
        checkin,
        silent,
        pruned(
            agent_id,
            "p3",
            MessageRole::User,
            "and the budget?",
            5,
            None,
        ),
        pruned(
            agent_id,
            "p4",
            MessageRole::Assistant,
            "Lots",
            6,
            Some((anima_core::REVISED_METADATA_KEY, DataValue::Bool(true))),
        ),
        pruned(
            agent_id,
            "p5",
            MessageRole::System,
            "Evaluator requested a revision: be precise",
            7,
            None,
        ),
        pruned(
            agent_id,
            "p6",
            MessageRole::Assistant,
            "About 2,000 euros",
            8,
            None,
        ),
    ];
    let store = {
        let mut guard = coordinator.state.write().await;
        guard
            .sessions
            .get_mut(agent_id, "chat:long")
            .unwrap()
            .pruned_through = Some(SessionPrunedThrough {
            message_id: "p6".into(),
            created_at_ms: 8,
        });
        guard.history.store()
    };
    store.upsert_messages(&rows).await.unwrap();
}

const PRUNED_TRANSCRIPT: &str =
    "Owner: where to?\nCompanion: Somewhere warm\nOwner: and the budget?\nCompanion: About 2,000 euros";

/// Controller ruling 2 (audit I5): turns pruning removed, which no summary
/// covers, are read back from the history store and summarized as the
/// model would have seen them, even when every hot turn fits.
#[tokio::test]
async fn pruned_turns_no_summary_covers_are_folded_into_the_summary() {
    let model = ScriptedModel::with_secondary(
        vec![Step::Text(vec!["Booked"])],
        vec![Step::Text(vec!["They wanted somewhere warm."])],
    );
    let (coordinator, agent_id) = session(model.clone(), 2_000.0, true).await;
    seed_pruned(&coordinator, &agent_id).await;

    coordinator
        .run(chat_request(&agent_id, "chat:long", "book one"))
        .await
        .unwrap();

    let input = &model.secondary_requests()[0].messages[0].content.text;
    assert!(
        input.ends_with(&format!("New turns:\n{PRUNED_TRANSCRIPT}")),
        "{input}"
    );
    for hidden in [
        "CHECKIN_OK",
        "Check status",
        "Lots",
        "Evaluator",
        "plan a trip",
    ] {
        assert!(!input.contains(hidden), "{hidden} is not summarized");
    }
    assert!(model.requests()[0]
        .system
        .contains("They wanted somewhere warm."));
    assert_eq!(
        sent_texts(&model),
        [
            "plan a trip",
            "Lisbon in May",
            "and hotels?",
            "Two options",
            "book one"
        ]
    );
    let guard = coordinator.state.read().await;
    let session = guard.sessions.get(&agent_id, "chat:long").unwrap();
    let summary = session.summary.as_ref().unwrap();
    assert_eq!(summary.through_message_id, "p6");
    assert_eq!(summary.source_message_count, 4);
    assert_eq!(session.context_trimmed, None);
}

/// Ruling 2 with dropped hot turns too: the pruned turns come first, and
/// the summary runs through the newest dropped hot message.
#[tokio::test]
async fn pruned_and_dropped_turns_are_summarized_oldest_first() {
    let model = ScriptedModel::with_secondary(
        vec![Step::Text(vec!["Booked"])],
        vec![Step::Text(vec!["They planned a trip to Lisbon."])],
    );
    let (coordinator, agent_id) = long_session(model.clone(), true).await;
    seed_pruned(&coordinator, &agent_id).await;

    coordinator
        .run(chat_request(&agent_id, "chat:long", "book one"))
        .await
        .unwrap();

    let input = &model.secondary_requests()[0].messages[0].content.text;
    assert!(
        input.ends_with(&format!(
            "New turns:\n{PRUNED_TRANSCRIPT}\nOwner: plan a trip\nCompanion: Lisbon in May"
        )),
        "{input}"
    );
    assert_eq!(
        sent_texts(&model),
        ["and hotels?", "Two options", "book one"]
    );
    let guard = coordinator.state.read().await;
    let session = guard.sessions.get(&agent_id, "chat:long").unwrap();
    let summary = session.summary.as_ref().unwrap();
    assert_eq!(summary.through_message_id, "a1");
    assert_eq!(summary.source_message_count, 6);
    assert_eq!(session.context_trimmed, None);
}

/// A pruned span the history store cannot read is a failed compaction:
/// nothing is summarized, and the run goes on without those turns.
#[tokio::test]
async fn pruned_turns_the_store_cannot_read_are_a_compaction_error() {
    let model = ScriptedModel::with_secondary(vec![Step::Text(vec!["Booked"])], vec![]);
    let (coordinator, agent_id) = session(model.clone(), 2_000.0, true).await;
    let store = Arc::new(FlakyHistoryStore::new());
    coordinator
        .state
        .write()
        .await
        .set_history(HistoryService::new(store.clone()));
    seed_pruned(&coordinator, &agent_id).await;
    store.set_page_messages_failing(true);

    coordinator
        .run(chat_request(&agent_id, "chat:long", "book one"))
        .await
        .unwrap();

    assert!(
        model.secondary_requests().is_empty(),
        "nothing to summarize"
    );
    assert_eq!(sent_texts(&model).len(), 5);
    let guard = coordinator.state.read().await;
    let session = guard.sessions.get(&agent_id, "chat:long").unwrap();
    assert_eq!(session.summary, None);
    let error = session.compaction_error.as_ref().unwrap();
    assert!(
        error
            .message
            .starts_with("Earlier turns could not be read: "),
        "{}",
        error.message
    );
    assert_eq!(
        session
            .context_trimmed
            .as_ref()
            .map(|trimmed| trimmed.dropped_through_message_id.as_str()),
        Some("p6")
    );
}

/// Fix round 1 (Minor 6): an announced compaction whose pruned turns the
/// history store does not have records that instead of doing nothing.
#[tokio::test]
async fn a_pruned_span_the_store_does_not_have_is_a_compaction_error() {
    let model = ScriptedModel::with_secondary(vec![Step::Text(vec!["Booked"])], vec![]);
    let (coordinator, agent_id) = session(model.clone(), 2_000.0, true).await;
    coordinator
        .state
        .write()
        .await
        .sessions
        .get_mut(&agent_id, "chat:long")
        .unwrap()
        .pruned_through = Some(SessionPrunedThrough {
        message_id: "p6".into(),
        created_at_ms: 8,
    });

    coordinator
        .run(chat_request(&agent_id, "chat:long", "book one"))
        .await
        .unwrap();

    assert!(
        model.secondary_requests().is_empty(),
        "nothing to summarize"
    );
    let guard = coordinator.state.read().await;
    let session = guard.sessions.get(&agent_id, "chat:long").unwrap();
    assert_eq!(
        session
            .compaction_error
            .as_ref()
            .map(|error| error.message.as_str()),
        Some("Earlier turns could not be read: none were found")
    );
    assert_eq!(
        session
            .context_trimmed
            .as_ref()
            .map(|trimmed| trimmed.dropped_through_message_id.as_str()),
        Some("p6")
    );
}
