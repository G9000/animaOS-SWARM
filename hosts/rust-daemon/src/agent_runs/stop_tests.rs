//! Stopping runs (spec §4.6).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::Duration;

use anima_core::{
    AgentConfig, AgentStatus, Content, DataValue, Message, MessageRole, ModelAdapter,
    ModelGenerateRequest, ModelGenerateResponse, ModelStopReason, TokenUsage, ToolCall,
    CANCELLED_TOOL_RESULT,
};
use async_trait::async_trait;
use futures::future::BoxFuture;
use tokio::sync::Semaphore;

use super::test_support::{
    accept, accept_web, add_chat, calculate_call, chat_request, coordinator_with, events_until,
    is_terminal, lead_config, quiet_for, wait_for, Gate, ScriptedModel, Step,
};
use super::{
    AcceptedRun, AgentRunCoordinator, QueuedRunStart, QueuedStartError, ACCEPTED_AT_METADATA_KEY,
    CLIENT_REQUEST_ID_METADATA_KEY, RUN_NOT_QUEUED,
};
use crate::app::SharedDaemonState;
use crate::runs::{RunSource, RunStatus};

fn tool_call_id(message: &Message) -> Option<&str> {
    match message
        .content
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("toolCallId"))
    {
        Some(DataValue::String(id)) => Some(id),
        _ => None,
    }
}

/// The id of the running run whose streamed text reads `text`, once one does.
async fn running_with_text(coordinator: &AgentRunCoordinator, text: &str) -> String {
    for _ in 0..500 {
        {
            let guard = coordinator.state.read().await;
            let found = guard
                .runs
                .active_records()
                .into_iter()
                .filter(|record| record.status == RunStatus::Running)
                .find(|record| {
                    guard
                        .live
                        .runs()
                        .view(&record.id)
                        .is_some_and(|view| view.text == text)
                })
                .map(|record| record.id.clone());
            if let Some(run_id) = found {
                return run_id;
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no run streamed {text:?}");
}

/// Answers its first call with two tool calls and cancels the running run's
/// control just before returning them: a stop that lands after the model
/// asked for tools and before they run (Review Focus 2). Later calls say "ok".
struct StopBeforeToolsModel {
    state: OnceLock<SharedDaemonState>,
    calls: AtomicUsize,
    requests: StdMutex<Vec<ModelGenerateRequest>>,
}

#[async_trait]
impl ModelAdapter for StopBeforeToolsModel {
    fn provider(&self) -> &str {
        "stop-before-tools"
    }

    async fn generate(
        &self,
        _config: &AgentConfig,
        request: &ModelGenerateRequest,
    ) -> Result<ModelGenerateResponse, String> {
        self.requests.lock().unwrap().push(request.clone());
        let first = self.calls.fetch_add(1, Ordering::SeqCst) == 0;
        let (text, tool_calls, stop_reason) = if first {
            let state = Arc::clone(self.state.get().expect("the test sets the state"));
            let guard = state.read().await;
            let running = guard
                .runs
                .active_records()
                .into_iter()
                .find(|record| record.status == RunStatus::Running)
                .expect("the run is running")
                .id
                .clone();
            guard
                .live
                .runs()
                .control(&running)
                .expect("the run is registered")
                .cancel
                .cancel();
            (
                String::new(),
                Some(vec![
                    calculate_call("call-1", "1+1"),
                    calculate_call("call-2", "2+2"),
                ]),
                ModelStopReason::ToolCall,
            )
        } else {
            ("ok".to_string(), None, ModelStopReason::End)
        };
        Ok(ModelGenerateResponse {
            content: Content {
                text,
                ..Content::default()
            },
            tool_calls,
            usage: TokenUsage::default(),
            stop_reason,
        })
    }
}

#[tokio::test]
async fn a_stop_between_the_tool_request_and_the_tool_batch_answers_every_call() {
    let model = Arc::new(StopBeforeToolsModel {
        state: OnceLock::new(),
        calls: AtomicUsize::new(0),
        requests: StdMutex::new(Vec::new()),
    });
    let (coordinator, agent_id) = coordinator_with(model.clone()).await;
    assert!(model.state.set(Arc::clone(&coordinator.state)).is_ok());

    let stopped = coordinator
        .run(chat_request(&agent_id, "chat:tools", "compute both"))
        .await
        .unwrap();
    assert_eq!(stopped.result.error.as_deref(), Some("stopped"));
    {
        let guard = coordinator.state.read().await;
        let record = guard
            .runs
            .for_session(&agent_id, "chat:tools")
            .first()
            .map(|record| (*record).clone())
            .unwrap();
        assert_eq!(record.status, RunStatus::Cancelled);
        assert_eq!(record.error.as_ref().unwrap().code, "stopped");
        let tools: Vec<&Message> = guard.agents[&agent_id]
            .messages()
            .iter()
            .filter(|message| message.role == MessageRole::Tool)
            .collect();
        assert_eq!(tools.len(), 2, "every requested call has a result");
        for message in tools {
            assert!(message.content.text.contains(CANCELLED_TOOL_RESULT));
        }
        assert_ne!(
            guard.get_agent(&agent_id).unwrap().state.status,
            AgentStatus::Failed
        );
    }

    // The next run in the session sends a history every provider accepts.
    coordinator
        .run(chat_request(&agent_id, "chat:tools", "try again"))
        .await
        .unwrap();
    let requests = model.requests.lock().unwrap().clone();
    let history = &requests[1].messages;
    for id in ["call-1", "call-2"] {
        assert!(
            history
                .iter()
                .any(|message| message.role == MessageRole::Tool
                    && tool_call_id(message) == Some(id)
                    && message.content.text.contains(CANCELLED_TOOL_RESULT)),
            "{id} is answered in the next run's history"
        );
    }
}

#[tokio::test]
async fn a_stopped_run_keeps_its_partial_text_and_the_next_message_runs_normally() {
    let model = ScriptedModel::new(vec![
        Step::Hold(vec!["Half an "]),
        Step::Text(vec!["Carrying on"]),
    ]);
    let (coordinator, agent_id) = coordinator_with(model.clone()).await;
    let running = {
        let coordinator = coordinator.clone();
        let request = chat_request(&agent_id, "chat:stop", "write a long answer");
        tokio::spawn(async move { coordinator.run(request).await })
    };
    let run_id = running_with_text(&coordinator, "Half an ").await;

    let stopping = coordinator.stop_run(&agent_id, &run_id).await.unwrap();
    assert_eq!(
        stopping.status,
        RunStatus::Running,
        "the stop is saved; the run ends at its next checkpoint"
    );
    let requested = stopping
        .stop
        .clone()
        .expect("the stop is recorded")
        .requested_at_ms;

    let envelope = running.await.unwrap().unwrap();
    assert_eq!(envelope.result.error.as_deref(), Some("stopped"));
    {
        let guard = coordinator.state.read().await;
        let record = guard.runs.get(&run_id).unwrap();
        assert_eq!(record.status, RunStatus::Cancelled);
        assert_eq!(record.error.as_ref().unwrap().code, "stopped");
        assert_eq!(record.error.as_ref().unwrap().message, "Stopped by owner");
        assert_eq!(record.stop.as_ref().unwrap().requested_at_ms, requested);
        assert_eq!(record.reply_message_id, None);
        let partial = guard.agents[&agent_id]
            .messages()
            .iter()
            .find(|message| message.role == MessageRole::Assistant)
            .unwrap();
        assert_eq!(partial.content.text, "Half an ");
        assert_eq!(
            partial.content.metadata.as_ref().unwrap()["stopped"],
            DataValue::Bool(true)
        );
        assert_ne!(
            guard.get_agent(&agent_id).unwrap().state.status,
            AgentStatus::Failed,
            "a stop is not a failure"
        );
    }

    coordinator
        .run(chat_request(&agent_id, "chat:stop", "go on"))
        .await
        .unwrap();
    assert!(model.requests()[1]
        .messages
        .iter()
        .any(
            |message| message.role == MessageRole::Assistant && message.content.text == "Half an "
        ));
}

#[tokio::test]
async fn stopping_a_run_stops_the_helpers_it_started() {
    let spawn = ToolCall {
        id: "spawn-1".into(),
        name: "spawn_helper".into(),
        args: BTreeMap::from([
            ("name".to_string(), DataValue::String("Researcher".into())),
            ("task".to_string(), DataValue::String("Look into it".into())),
        ]),
    };
    let model = ScriptedModel::new(vec![Step::Tools(vec![spawn]), Step::Hold(vec!["Looking"])]);
    let (coordinator, _) = coordinator_with(model.clone()).await;
    let companion = coordinator
        .state
        .write()
        .await
        .create_agent(lead_config("Companion"))
        .unwrap()
        .state
        .id;
    let running = {
        let coordinator = coordinator.clone();
        let request = chat_request(&companion, "chat:lead", "research this");
        tokio::spawn(async move { coordinator.run(request).await })
    };
    let helper_run = running_with_text(&coordinator, "Looking").await;
    let lead_run = coordinator
        .state
        .read()
        .await
        .runs
        .get(&helper_run)
        .unwrap()
        .parent_run_id
        .clone()
        .expect("the helper run links to the companion's run");

    coordinator.stop_run(&companion, &lead_run).await.unwrap();

    let envelope = running.await.unwrap().unwrap();
    assert_eq!(envelope.result.error.as_deref(), Some("stopped"));
    let guard = coordinator.state.read().await;
    for id in [&lead_run, &helper_run] {
        let record = guard.runs.get(id).unwrap();
        assert_eq!(record.status, RunStatus::Cancelled, "{id}");
        assert_eq!(record.error.as_ref().unwrap().code, "stopped");
        assert!(record.stop.is_some(), "the stop of {id} was saved first");
    }
    assert_eq!(
        model.requests().len(),
        2,
        "the companion never called its model again"
    );
}

/// A stopped run's stream hears exactly one terminal event, `run.cancelled`
/// with the stop's error, and nothing after it.
#[tokio::test]
async fn a_stopped_run_ends_with_one_run_cancelled_event() {
    let model = ScriptedModel::new(vec![Step::Hold(vec!["Half"])]);
    let (coordinator, agent_id) = coordinator_with(model).await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();
    let running = {
        let coordinator = coordinator.clone();
        let request = chat_request(&agent_id, "chat:one", "think");
        tokio::spawn(async move { coordinator.run(request).await })
    };
    let run_id = running_with_text(&coordinator, "Half").await;

    coordinator.stop_run(&agent_id, &run_id).await.unwrap();
    running.await.unwrap().unwrap();

    let events = events_until(&mut subscription, "run.cancelled").await;
    assert_eq!(
        events.iter().filter(|event| is_terminal(event)).count(),
        1,
        "{events:?}"
    );
    let cancelled = events.last().unwrap();
    assert_eq!(cancelled["runId"], run_id.as_str());
    assert_eq!(cancelled["run"]["status"], "cancelled");
    assert_eq!(cancelled["run"]["error"]["code"], "stopped");
    assert_eq!(cancelled["run"]["error"]["message"], "Stopped by owner");
    assert!(cancelled["run"]["stop"]["requestedAtMs"].as_u64().is_some());
    let after = quiet_for(&mut subscription).await;
    assert!(after.iter().all(|event| !is_terminal(event)), "{after:?}");
}

/// Controller ruling (M3 pre-flight audit M25): a helper's own run can be
/// stopped through the helper; its companion's run goes on with the stopped
/// result and finishes normally.
#[tokio::test]
async fn stopping_a_helpers_own_run_leaves_its_companion_running() {
    let spawn = ToolCall {
        id: "spawn-1".into(),
        name: "spawn_helper".into(),
        args: BTreeMap::from([
            ("name".to_string(), DataValue::String("Researcher".into())),
            ("task".to_string(), DataValue::String("Look into it".into())),
        ]),
    };
    let model = ScriptedModel::new(vec![
        Step::Tools(vec![spawn]),
        Step::Hold(vec!["Looking"]),
        Step::Text(vec!["I will look myself"]),
    ]);
    let (coordinator, _) = coordinator_with(model.clone()).await;
    let companion = coordinator
        .state
        .write()
        .await
        .create_agent(lead_config("Companion"))
        .unwrap()
        .state
        .id;
    let running = {
        let coordinator = coordinator.clone();
        let request = chat_request(&companion, "chat:lead", "research this");
        tokio::spawn(async move { coordinator.run(request).await })
    };
    let helper_run = running_with_text(&coordinator, "Looking").await;
    let (helper, lead_run) = {
        let guard = coordinator.state.read().await;
        let record = guard.runs.get(&helper_run).unwrap();
        (
            record.agent_id.clone(),
            record.parent_run_id.clone().unwrap(),
        )
    };

    // The run is the helper's: not found through its companion.
    let through_companion = coordinator
        .stop_run(&companion, &helper_run)
        .await
        .unwrap_err();
    assert_eq!(
        through_companion.status(),
        axum::http::StatusCode::NOT_FOUND
    );
    let stopping = coordinator.stop_run(&helper, &helper_run).await.unwrap();
    assert!(stopping.stop.is_some());

    let envelope = running.await.unwrap().unwrap();
    assert_eq!(envelope.result.error, None, "the companion finished");
    let guard = coordinator.state.read().await;
    let helper_record = guard.runs.get(&helper_run).unwrap();
    assert_eq!(helper_record.status, RunStatus::Cancelled);
    assert_eq!(helper_record.error.as_ref().unwrap().code, "stopped");
    let lead_record = guard.runs.get(&lead_run).unwrap();
    assert_eq!(lead_record.status, RunStatus::Completed);
    assert!(
        lead_record.stop.is_none(),
        "only the helper's run was stopped"
    );
    assert_eq!(model.requests().len(), 3);
    assert_ne!(
        guard.get_agent(&helper).unwrap().state.status,
        AgentStatus::Failed
    );
}

/// A helper the companion's tool starts after the companion's stop was saved
/// (spawn_helper was already running) is stopped too, before its model call:
/// spec §4.6 stops every helper run the stopped run starts.
#[tokio::test]
async fn a_helper_started_after_its_companions_stop_never_calls_its_model() {
    let spawn = ToolCall {
        id: "spawn-1".into(),
        name: "spawn_helper".into(),
        args: BTreeMap::from([
            ("name".to_string(), DataValue::String("Researcher".into())),
            ("task".to_string(), DataValue::String("Look into it".into())),
        ]),
    };
    let gate = Gate::new();
    let model = ScriptedModel::gated(vec![Step::Tools(vec![spawn])], gate.clone());
    let (coordinator, _) = coordinator_with(model.clone()).await;
    let companion = coordinator
        .state
        .write()
        .await
        .create_agent(lead_config("Companion"))
        .unwrap()
        .state
        .id;
    let running = {
        let coordinator = coordinator.clone();
        let request = chat_request(&companion, "chat:lead", "research this");
        tokio::spawn(async move { coordinator.run(request).await })
    };
    gate.entered().await;
    let lead_run = coordinator
        .state
        .read()
        .await
        .runs
        .active_records()
        .into_iter()
        .find(|record| record.agent_id == companion)
        .unwrap()
        .id
        .clone();
    // Holds spawn_helper's save of the helper it creates, inside its
    // control-plane transaction, so the stop lands before the helper run.
    let save_gate = coordinator
        .state
        .write()
        .await
        .install_test_control_plane_save_gate(false);
    gate.release();
    tokio::time::timeout(Duration::from_secs(5), save_gate.entered.acquire())
        .await
        .expect("spawn_helper saves its helper")
        .unwrap()
        .forget();
    let stopping = {
        let coordinator = coordinator.clone();
        let (companion, lead_run) = (companion.clone(), lead_run.clone());
        tokio::spawn(async move { coordinator.stop_run(&companion, &lead_run).await })
    };
    // The stop waits for spawn_helper's transaction, ahead of the helper run.
    tokio::time::sleep(Duration::from_millis(50)).await;
    save_gate.release.add_permits(1);
    stopping.await.unwrap().unwrap();
    // Were the helper to call its model anyway, these let it finish.
    gate.release();
    gate.release();

    let envelope = tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .expect("the companion's run ends")
        .unwrap()
        .unwrap();
    assert_eq!(envelope.result.error.as_deref(), Some("stopped"));
    let guard = coordinator.state.read().await;
    assert!(
        guard.runs.active_records().is_empty(),
        "nothing is still running"
    );
    let helper = guard
        .list_agents()
        .into_iter()
        .find(|agent| super::config_helper_parent(&agent.state.config) == Some(companion.as_str()))
        .expect("a helper was created")
        .state
        .id;
    let helper_run = guard
        .runs
        .for_agent(&helper)
        .first()
        .map(|record| (*record).clone())
        .expect("the helper run is recorded");
    assert_eq!(helper_run.parent_run_id.as_deref(), Some(lead_run.as_str()));
    assert_eq!(helper_run.status, RunStatus::Cancelled);
    assert_eq!(helper_run.error.as_ref().unwrap().code, "stopped");
    assert!(
        helper_run.stop.is_some(),
        "its stop is saved with its start"
    );
    assert_eq!(
        model.requests().len(),
        1,
        "only the companion's first call reached the model"
    );
}

/// What Task 9's steer acceptance pushes into a run's inbox: the text with
/// its key and acceptance time.
fn steer(key: &str, text: &str, accepted_at_ms: u64) -> Content {
    Content {
        text: text.into(),
        attachments: None,
        metadata: Some(BTreeMap::from([
            (
                CLIENT_REQUEST_ID_METADATA_KEY.to_string(),
                DataValue::String(key.into()),
            ),
            (
                ACCEPTED_AT_METADATA_KEY.to_string(),
                DataValue::Number(accepted_at_ms as f64),
            ),
        ])),
    }
}

/// Controller ruling (M3 pre-flight audit M8, a deliberate deviation from
/// spec §4.7): an explicit stop does not requeue the steers its run never
/// read; each becomes an `interrupted` run the owner can send again. A steer
/// pushed once the inbox closed is refused, so its sender queues it instead
/// (review race iii).
#[tokio::test]
async fn a_stopped_runs_unread_steers_become_interrupted_runs_to_send_again() {
    let model = ScriptedModel::new(vec![Step::Hold(vec!["Working"])]);
    let (coordinator, agent_id) = coordinator_with(model.clone()).await;
    add_chat(&coordinator, &agent_id, "chat:s").await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();
    let run_id = accept_web(&coordinator, &agent_id, "chat:s", "key-1").await;
    assert_eq!(running_with_text(&coordinator, "Working").await, run_id);
    let control = hub.runs().control(&run_id).unwrap();
    let now = anima_core::primitives::now_millis();
    control
        .steering
        .push(steer("key-2", "and the weather?", now))
        .unwrap();
    control
        .steering
        .push(steer("key-3", "and tomorrow?", now + 1))
        .unwrap();

    coordinator.stop_run(&agent_id, &run_id).await.unwrap();
    let events = events_until(&mut subscription, "run.cancelled").await;

    assert!(
        control
            .steering
            .push(steer("key-4", "too late", now + 2))
            .is_err(),
        "the inbox closed with the run"
    );
    let interrupted: Vec<&serde_json::Value> = events
        .iter()
        .filter(|event| event["type"] == "run.interrupted")
        .collect();
    assert_eq!(interrupted.len(), 2, "{events:?}");
    let guard = coordinator.state.read().await;
    let saved = guard.control_plane_snapshot().runs;
    for (key, text, accepted_at_ms) in [
        ("key-2", "and the weather?", now),
        ("key-3", "and tomorrow?", now + 1),
    ] {
        let record = guard
            .runs
            .find_by_idempotency_key(&agent_id, key, 0)
            .unwrap_or_else(|| panic!("{key} is recorded"));
        assert_eq!(record.status, RunStatus::Interrupted);
        assert_eq!(record.session_id, "chat:s");
        assert_eq!(record.source, RunSource::Web);
        assert_eq!(record.input.text, text);
        assert_eq!(record.created_at_ms, accepted_at_ms);
        assert_eq!(record.started_at_ms, None);
        let error = record.error.as_ref().unwrap();
        assert_eq!(error.code, "stopped_before_start");
        assert_eq!(
            error.message,
            "The run was stopped before this message reached it; it is safe to send it again."
        );
        assert!(
            interrupted
                .iter()
                .any(|event| event["runId"] == record.id.as_str()),
            "{key} is announced"
        );
        assert!(
            saved.iter().any(|saved| saved.id == record.id),
            "{key} is saved"
        );
    }
    assert_eq!(model.requests().len(), 1, "no steer ran or was queued");
    assert_eq!(guard.runs.queued_count(&agent_id), 0);
}

/// A start closure that signals `entered` once its session queue calls it,
/// then waits for `hold` before starting the run as a web message would.
fn held_start(
    coordinator: &AgentRunCoordinator,
    agent_id: &str,
    session_id: &str,
    entered: Arc<Semaphore>,
    hold: Arc<Semaphore>,
) -> QueuedRunStart {
    let coordinator = coordinator.clone();
    let request = chat_request(agent_id, session_id, "held");
    Box::new(
        move |run_id: String| -> BoxFuture<'static, Result<(), QueuedStartError>> {
            Box::pin(async move {
                entered.add_permits(1);
                hold.acquire().await.unwrap().forget();
                coordinator
                    .run_accepted(request, run_id)
                    .await
                    .map(|_| ())
                    .map_err(|error| QueuedStartError::Failed(error.message().to_string()))
            })
        },
    )
}

/// Review race (i): a stop that lands after the session queue decided to
/// start the run still keeps it from starting, and the start's refusal does
/// not overwrite the stop. Phase A itself refuses a record that is no longer
/// queued, even with a live control.
#[tokio::test]
async fn a_stop_that_lands_as_the_queue_starts_the_run_keeps_it_from_starting() {
    let model = ScriptedModel::new(vec![]);
    let (coordinator, agent_id) = coordinator_with(model.clone()).await;
    add_chat(&coordinator, &agent_id, "chat:r").await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();
    let (entered, hold) = (Arc::new(Semaphore::new(0)), Arc::new(Semaphore::new(0)));
    let start = held_start(
        &coordinator,
        &agent_id,
        "chat:r",
        entered.clone(),
        hold.clone(),
    );
    let AcceptedRun::Created(record) = coordinator
        .accept_run(accept(&agent_id, "chat:r", "key-1"), start)
        .await
        .unwrap()
    else {
        panic!("a new run");
    };
    tokio::time::timeout(Duration::from_secs(5), entered.acquire())
        .await
        .expect("the queue starts the run")
        .unwrap()
        .forget();

    let stopped = coordinator.stop_run(&agent_id, &record.id).await.unwrap();
    assert_eq!(stopped.status, RunStatus::Cancelled);
    hold.add_permits(1);
    for _ in 0..500 {
        if !coordinator.has_session_queue(&agent_id, "chat:r") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!coordinator.has_session_queue(&agent_id, "chat:r"));

    let events = events_until(&mut subscription, "run.cancelled").await;
    let after = quiet_for(&mut subscription).await;
    assert_eq!(
        events
            .iter()
            .chain(&after)
            .filter(|event| is_terminal(event))
            .count(),
        1,
        "{events:?} {after:?}"
    );
    {
        let guard = coordinator.state.read().await;
        let settled = guard.runs.get(&record.id).unwrap();
        assert_eq!(settled.status, RunStatus::Cancelled);
        assert_eq!(settled.error.as_ref().unwrap().code, "stopped");
        assert_eq!(settled.started_at_ms, None);
        assert!(guard.live.runs().control(&record.id).is_none());
    }

    // Phase A refuses it even when a control is registered for it.
    hub.runs().register(&record.id);
    let refused = coordinator
        .run_accepted(chat_request(&agent_id, "chat:r", "held"), record.id.clone())
        .await
        .unwrap_err();
    assert_eq!(refused.message(), RUN_NOT_QUEUED);
    hub.runs().remove(&record.id);
    assert!(model.requests().is_empty(), "it never ran");
    assert_eq!(coordinator.lock_counts(), (0, 0));
}

/// Review race (ii): stopping a message that waits for an agent slot ends
/// the wait at once; the start that gives up finds the control gone and
/// settles nothing, so the run stays `cancelled` with one terminal event.
#[tokio::test]
async fn stopping_a_message_waiting_for_a_slot_ends_the_wait_with_one_event() {
    let gate = Gate::new();
    let model = ScriptedModel::gated(vec![], gate.clone());
    let (coordinator, agent_id) = coordinator_with(model.clone()).await;
    let coordinator = coordinator.with_max_runs_per_agent(1);
    add_chat(&coordinator, &agent_id, "chat:b").await;
    let holding = {
        let coordinator = coordinator.clone();
        let request = chat_request(&agent_id, "room-a", "hold");
        tokio::spawn(async move { coordinator.run(request).await })
    };
    gate.entered().await;
    let hub = coordinator.state.read().await.live.clone();
    let mut subscription = hub.subscribe(&agent_id).unwrap();
    let waiting = accept_web(&coordinator, &agent_id, "chat:b", "key-b").await;
    tokio::time::sleep(Duration::from_millis(20)).await;

    coordinator.stop_run(&agent_id, &waiting).await.unwrap();
    for _ in 0..500 {
        if !coordinator.has_session_queue(&agent_id, "chat:b") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        !coordinator.has_session_queue(&agent_id, "chat:b"),
        "the wait ended"
    );
    let after = quiet_for(&mut subscription).await;
    let terminal: Vec<&serde_json::Value> = after
        .iter()
        .filter(|event| is_terminal(event) && event["runId"] == waiting.as_str())
        .collect();
    assert_eq!(terminal.len(), 1, "{after:?}");
    assert_eq!(terminal[0]["type"], "run.cancelled");
    {
        let guard = coordinator.state.read().await;
        let record = guard.runs.get(&waiting).unwrap();
        assert_eq!(record.status, RunStatus::Cancelled);
        assert_eq!(record.error.as_ref().unwrap().code, "stopped");
        assert!(guard.live.runs().control(&waiting).is_none());
    }
    assert_eq!(coordinator.lock_counts(), (1, 1), "only the held run's");

    gate.release();
    holding.await.unwrap().unwrap();
    assert_eq!(coordinator.lock_counts(), (0, 0));
    assert_eq!(model.requests().len(), 1);
}

/// A stop that cannot be saved changes nothing: the running run keeps
/// running unsignalled, and a second stop, once saving works, stops it.
#[tokio::test]
async fn a_stop_that_cannot_be_saved_leaves_the_running_run_alone() {
    let model = ScriptedModel::new(vec![Step::Hold(vec!["Busy"])]);
    let (coordinator, agent_id) = coordinator_with(model).await;
    let running = {
        let coordinator = coordinator.clone();
        let request = chat_request(&agent_id, "chat:u", "work");
        tokio::spawn(async move { coordinator.run(request).await })
    };
    let run_id = running_with_text(&coordinator, "Busy").await;
    let save_gate = coordinator
        .state
        .write()
        .await
        .install_test_control_plane_save_gate(true);
    save_gate.release.add_permits(1);

    let error = coordinator.stop_run(&agent_id, &run_id).await.unwrap_err();
    assert_eq!(error.status(), axum::http::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error.message(), "injected control-plane save failure");
    {
        let guard = coordinator.state.read().await;
        let record = guard.runs.get(&run_id).unwrap();
        assert_eq!(record.status, RunStatus::Running);
        assert_eq!(record.stop, None, "the unsaved stop is reverted");
        assert!(!guard
            .live
            .runs()
            .control(&run_id)
            .unwrap()
            .cancel
            .is_cancelled());
    }

    coordinator.stop_run(&agent_id, &run_id).await.unwrap();
    let envelope = running.await.unwrap().unwrap();
    assert_eq!(envelope.result.error.as_deref(), Some("stopped"));
}

/// A stop of a queued message that cannot be saved leaves it queued, and it
/// still runs: its session queue never acts on a stop whose save is still in
/// flight (it could be reverted, and the queue would have dropped the run).
#[tokio::test]
async fn a_queued_message_whose_stop_cannot_be_saved_still_runs() {
    let model = ScriptedModel::new(vec![Step::Text(vec!["Ran"])]);
    let (coordinator, agent_id) = coordinator_with(model.clone()).await;
    add_chat(&coordinator, &agent_id, "chat:q").await;
    // The session's first message is held by its own start, outside any
    // transaction, so the queue reaches the second while the stop saves.
    let (entered, hold) = (Arc::new(Semaphore::new(0)), Arc::new(Semaphore::new(0)));
    let first: QueuedRunStart = {
        let (entered, hold) = (entered.clone(), hold.clone());
        Box::new(move |_run_id| {
            Box::pin(async move {
                entered.add_permits(1);
                hold.acquire().await.unwrap().forget();
                Ok(())
            })
        })
    };
    coordinator
        .accept_run(accept(&agent_id, "chat:q", "key-1"), first)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    let second = accept_web(&coordinator, &agent_id, "chat:q", "key-2").await;
    let save_gate = coordinator
        .state
        .write()
        .await
        .install_test_control_plane_save_gate(true);
    let stopping = {
        let coordinator = coordinator.clone();
        let (agent_id, second) = (agent_id.clone(), second.clone());
        tokio::spawn(async move { coordinator.stop_run(&agent_id, &second).await })
    };
    tokio::time::timeout(Duration::from_secs(5), save_gate.entered.acquire())
        .await
        .expect("the stop saves")
        .unwrap()
        .forget();
    // The queue moves on to the second message while the stop is unsaved.
    hold.add_permits(1);
    tokio::time::sleep(Duration::from_millis(100)).await;
    save_gate.release.add_permits(1);

    let error = stopping.await.unwrap().unwrap_err();
    assert_eq!(error.status(), axum::http::StatusCode::SERVICE_UNAVAILABLE);
    wait_for(&coordinator, &second, RunStatus::Completed).await;
    let guard = coordinator.state.read().await;
    assert_eq!(guard.runs.get(&second).unwrap().stop, None);
    assert_eq!(model.requests().len(), 1);
}

/// The bash polling loop kills its child when the run is stopped (spec
/// §4.6), so a stop does not wait for a long command.
#[cfg(unix)]
#[tokio::test]
async fn a_stop_kills_the_bash_command_its_run_waits_for() {
    let bash = ToolCall {
        id: "bash-1".into(),
        name: "bash".into(),
        args: BTreeMap::from([(
            "command".to_string(),
            DataValue::String("exec sleep 30".into()),
        )]),
    };
    let model = ScriptedModel::new(vec![Step::Tools(vec![bash])]);
    let (coordinator, _) = coordinator_with(model.clone()).await;
    let workspace =
        std::env::temp_dir().join(format!("anima-daemon-stop-bash-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&workspace).unwrap();
    let agent_id = {
        let mut guard = coordinator.state.write().await;
        guard.workspace = Some(crate::control_plane_store::WorkspaceConfig {
            root_path: workspace.clone(),
            company_name: "Acme".into(),
            mission: "Ship carefully".into(),
            values: vec![],
        });
        let mut config = super::test_support::companion_config("Shell");
        config.tools = Some(
            crate::tools::ToolRegistry::new()
                .resolve_descriptors(["bash"])
                .unwrap(),
        );
        guard.create_agent(config).unwrap().state.id
    };
    let running = {
        let coordinator = coordinator.clone();
        let request = chat_request(&agent_id, "chat:bash", "sleep a while");
        tokio::spawn(async move { coordinator.run(request).await })
    };
    let run_id = 'found: {
        for _ in 0..500 {
            {
                let guard = coordinator.state.read().await;
                let found = guard.runs.active_records().into_iter().find(|record| {
                    guard.live.runs().view(&record.id).is_some_and(|view| {
                        view.tools
                            .iter()
                            .any(|tool| tool.name == "bash" && tool.status == "running")
                    })
                });
                if let Some(record) = found {
                    break 'found record.id.clone();
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("bash never started");
    };
    let started = std::time::Instant::now();

    coordinator.stop_run(&agent_id, &run_id).await.unwrap();
    let envelope = tokio::time::timeout(Duration::from_secs(10), running)
        .await
        .expect("the stop ends the command")
        .unwrap()
        .unwrap();

    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(envelope.result.error.as_deref(), Some("stopped"));
    let guard = coordinator.state.read().await;
    let result = guard.agents[&agent_id]
        .messages()
        .iter()
        .find(|message| message.role == MessageRole::Tool)
        .expect("the command has a result")
        .clone();
    assert!(
        result.content.text.contains("Command stopped by owner"),
        "{}",
        result.content.text
    );
    assert_eq!(model.requests().len(), 1);
    drop(guard);
    std::fs::remove_dir_all(workspace).ok();
}
