//! Stopped Telegram turns, suppressed replies, and ledger-backed owner-send
//! replays (spec §4.6; M1 F16).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::sync::{RwLock, Semaphore};

use super::*;
use crate::agent_runs::test_support::{calculate_call, companion_config, ScriptedModel, Step};
use crate::connectors::credentials::InMemoryCredentialStore;
use crate::connectors::{TelegramChatKind, TelegramChatMetadata, TelegramSenderMetadata};
use crate::runs::RunStatus;

const CONNECTOR: &str = "telegram-stop";
const ROOM: &str = "telegram-room-stop";

fn connector(agent_id: &str) -> TelegramConnectorRecord {
    TelegramConnectorRecord {
        id: CONNECTOR.into(),
        agent_id: agent_id.into(),
        room_id: ROOM.into(),
        bot: TelegramBotIdentity {
            id: "stop-bot".into(),
            username: Some("stop_bot".into()),
            display_name: None,
        },
        approved_chat: Some(chat()),
        pending_pairing: None,
        next_update_id: 0,
        enabled: true,
        deleted_at_ms: None,
        created_at_ms: 1,
        updated_at_ms: 1,
    }
}

fn chat() -> TelegramChatMetadata {
    TelegramChatMetadata {
        id: "stop-chat".into(),
        kind: TelegramChatKind::Private,
        title: None,
        username: None,
    }
}

fn inbound(agent_id: &str, update_id: i64, text: &str) -> TelegramInboundRecord {
    TelegramInboundRecord {
        connector_id: CONNECTOR.into(),
        update_id,
        agent_id: agent_id.into(),
        room_id: ROOM.into(),
        normalized_text: text.into(),
        sender: TelegramSenderMetadata {
            id: "sender-1".into(),
            username: None,
            display_name: None,
        },
        chat: chat(),
        received_at_ms: 1,
        processing_state: InboundProcessingState::Received,
        run_idempotency_key: format!("telegram:{CONNECTOR}:{update_id}"),
    }
}

/// Counts deliveries.
#[derive(Default)]
struct CountingSends(AtomicUsize);

#[async_trait]
impl TelegramTransport for CountingSends {
    async fn get_me(
        &self,
        _token: &TelegramBotToken,
    ) -> Result<TelegramBotIdentity, TelegramTransportError> {
        Ok(TelegramBotIdentity {
            id: "stop-bot".into(),
            username: Some("stop_bot".into()),
            display_name: None,
        })
    }

    async fn get_updates(
        &self,
        _token: &TelegramBotToken,
        offset: i64,
    ) -> Result<TelegramUpdateBatch, TelegramTransportError> {
        Ok(TelegramUpdateBatch {
            updates: Vec::new(),
            next_update_id: offset,
        })
    }

    async fn send_message(
        &self,
        _token: &TelegramBotToken,
        _chat_id: &str,
        _text: &str,
    ) -> Result<Vec<TelegramSentMessage>, TelegramTransportError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(Vec::new())
    }
}

async fn fixture(
    model: Arc<dyn anima_core::ModelAdapter>,
) -> (
    SharedDaemonState,
    AgentRunCoordinator,
    ConnectorManager,
    Arc<CountingSends>,
    String,
) {
    let mut daemon = DaemonState::with_model_adapter(model);
    let agent = daemon
        .create_agent(companion_config("telegram"))
        .unwrap()
        .state
        .id;
    daemon
        .connectors
        .insert(CONNECTOR.into(), connector(&agent));
    let state = Arc::new(RwLock::new(daemon));
    let runs = AgentRunCoordinator::new(Arc::clone(&state), Arc::new(Semaphore::new(4)));
    let credentials = Arc::new(InMemoryCredentialStore::default());
    credentials
        .put(CONNECTOR, TelegramBotToken::parse("42:stop-tests").unwrap())
        .await
        .unwrap();
    let transport = Arc::new(CountingSends::default());
    let manager = ConnectorManager::new(
        Arc::clone(&state),
        runs.clone(),
        credentials,
        transport.clone(),
    );
    (state, runs, manager, transport, agent)
}

/// The running run whose streamed text reads `text`, once one does.
async fn streaming_run(state: &SharedDaemonState, text: &str) -> String {
    for _ in 0..500 {
        {
            let guard = state.read().await;
            let found = guard
                .runs
                .active_records()
                .into_iter()
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

#[test]
fn stopped_and_suppressed_are_settled_states_with_their_own_names() {
    assert!(InboundProcessingState::Stopped.is_terminal());
    assert!(InboundProcessingState::Processed.is_terminal());
    assert!(!InboundProcessingState::Processing.is_terminal());
    assert!(OutboundDeliveryState::Suppressed.is_settled());
    assert!(OutboundDeliveryState::Delivered.is_settled());
    assert!(!OutboundDeliveryState::Suppressed.awaits_delivery());
    assert!(OutboundDeliveryState::Failed.awaits_delivery());
    assert_eq!(
        serde_json::to_value(InboundProcessingState::Stopped).unwrap(),
        "stopped"
    );
    assert_eq!(
        serde_json::to_value(OutboundDeliveryState::Suppressed).unwrap(),
        "suppressed"
    );
}

#[tokio::test]
async fn a_stopped_telegram_turn_is_saved_as_stopped_and_never_runs_again() {
    let (state, runs, manager, transport, agent) =
        fixture(ScriptedModel::new(vec![Step::Hold(vec!["Let me"])])).await;
    state
        .write()
        .await
        .inbound
        .insert((CONNECTOR.into(), 7), inbound(&agent, 7, "remind me"));
    let processing = {
        let manager = manager.clone();
        tokio::spawn(async move { manager.process_pending_once(CONNECTOR.into()).await })
    };
    let run_id = streaming_run(&state, "Let me").await;

    let stopping = runs.stop_run(&agent, &run_id).await.unwrap();
    assert!(stopping.stop.is_some());
    assert_eq!(
        state.read().await.inbound[&(CONNECTOR.to_string(), 7)].processing_state,
        InboundProcessingState::Stopped,
        "saved before the run was signalled"
    );

    assert!(processing.await.unwrap().unwrap());
    {
        let guard = state.read().await;
        assert_eq!(
            guard.runs.get(&run_id).unwrap().status,
            RunStatus::Cancelled
        );
        assert_eq!(
            guard.inbound[&(CONNECTOR.to_string(), 7)].processing_state,
            InboundProcessingState::Stopped
        );
        assert!(
            guard
                .outbound
                .values()
                .all(|record| record.connector_id != CONNECTOR),
            "a stopped turn has no reply"
        );
    }
    assert!(
        !manager
            .process_pending_once(CONNECTOR.into())
            .await
            .unwrap(),
        "a stopped turn is never run again"
    );
    assert_eq!(transport.0.load(Ordering::SeqCst), 0);
    manager.shutdown().await;
}

#[tokio::test]
async fn stopping_after_the_reply_committed_suppresses_its_delivery() {
    let (state, runs, manager, transport, agent) =
        fixture(ScriptedModel::new(vec![Step::Text(vec!["On it"])])).await;
    let (_, queued) = manager
        .send_from_owner(
            agent.clone(),
            CONNECTOR.into(),
            "remind me".into(),
            "owner-key".into(),
        )
        .await
        .unwrap();
    assert!(queued);
    let run = state
        .read()
        .await
        .runs
        .find_by_idempotency_key(&agent, "owner-key", 0)
        .cloned()
        .unwrap();
    assert_eq!(run.status, RunStatus::Completed);

    let stopped = runs.stop_run(&agent, &run.id).await.unwrap();

    assert_eq!(
        stopped.status,
        RunStatus::Completed,
        "a finished run stays as it was"
    );
    let delivery = state
        .read()
        .await
        .outbound
        .values()
        .find(|record| record.connector_id == CONNECTOR)
        .map(|record| record.delivery_state.clone());
    assert_eq!(delivery, Some(OutboundDeliveryState::Suppressed));
    assert!(
        !manager
            .deliver_pending_once(CONNECTOR.into())
            .await
            .unwrap(),
        "nothing is left to deliver"
    );
    assert_eq!(transport.0.load(Ordering::SeqCst), 0);
    manager.shutdown().await;
}

#[tokio::test]
async fn a_replayed_owner_send_answers_with_the_runs_own_reply() {
    let (_state, _runs, manager, _transport, agent) = fixture(ScriptedModel::new(vec![
        Step::Tools(vec![calculate_call("call-1", "1+1")]),
        Step::Text(vec!["It is 2"]),
    ]))
    .await;
    let send = || {
        manager.send_from_owner(
            agent.clone(),
            CONNECTOR.into(),
            "what is 1+1?".into(),
            "owner-key".into(),
        )
    };

    let (first, _) = send().await.unwrap();
    let (replayed, queued) = send().await.unwrap();

    let text = |envelope: &AgentRunEnvelope| {
        envelope
            .result
            .data
            .as_ref()
            .map(|content| content.text.clone())
    };
    assert_eq!(text(&first).as_deref(), Some("It is 2"));
    assert_eq!(
        text(&replayed).as_deref(),
        Some("It is 2"),
        "the final reply, not the tool-call message"
    );
    assert!(queued);
    manager.shutdown().await;
}

/// Final fix wave S2-D (review B, Minor 4): a replayed owner send answers
/// from its run's ledger record, so a stopped run is not a success, even
/// though its partial text is the first assistant message after the turn.
#[tokio::test]
async fn a_replayed_owner_send_of_a_stopped_run_is_not_a_success() {
    let model = ScriptedModel::new(vec![Step::Hold(vec!["Let me"])]);
    let (state, runs, manager, _transport, agent) = fixture(model.clone()).await;
    let send = || {
        let manager = manager.clone();
        let agent = agent.clone();
        async move {
            manager
                .send_from_owner(
                    agent,
                    CONNECTOR.into(),
                    "remind me".into(),
                    "owner-key".into(),
                )
                .await
        }
    };
    let first = tokio::spawn(send());
    let run_id = streaming_run(&state, "Let me").await;
    runs.stop_run(&agent, &run_id).await.unwrap();
    let (first, _) = first.await.unwrap().unwrap();
    assert_eq!(first.result.status, "error");

    let (replayed, queued) = send().await.unwrap();

    assert_eq!(
        replayed.result.status, "error",
        "a stopped run is not a success"
    );
    assert_eq!(replayed.result.error.as_deref(), Some("Stopped by owner"));
    assert!(replayed.result.data.is_none());
    assert!(!queued);
    assert_eq!(model.requests().len(), 1, "nothing ran again");
    manager.shutdown().await;
}

/// Final fix wave S2-D: a replayed owner send of a failed run answers with
/// its error instead of running the turn again.
#[tokio::test]
async fn a_replayed_owner_send_of_a_failed_run_answers_its_error() {
    let model = ScriptedModel::new(vec![Step::Fail("provider unavailable")]);
    let (_state, _runs, manager, _transport, agent) = fixture(model.clone()).await;
    let send = || {
        manager.send_from_owner(
            agent.clone(),
            CONNECTOR.into(),
            "remind me".into(),
            "owner-key".into(),
        )
    };
    let (first, _) = send().await.unwrap();
    assert_eq!(first.result.status, "error");

    let (replayed, queued) = send().await.unwrap();

    assert_eq!(replayed.result.status, "error");
    assert!(replayed.result.data.is_none());
    assert!(!queued);
    assert_eq!(model.requests().len(), 1, "the turn did not run again");
    manager.shutdown().await;
}

/// Final fix wave S2-D: a replayed owner send whose turn and reply left the
/// hot tail reads the reply from the history store instead of running the
/// turn again.
#[tokio::test]
async fn a_replayed_owner_send_whose_reply_left_the_hot_tail_reads_it_from_history() {
    let model = ScriptedModel::new(vec![Step::Text(vec!["It is 2"])]);
    let (state, _runs, manager, _transport, agent) = fixture(model.clone()).await;
    let send = || {
        manager.send_from_owner(
            agent.clone(),
            CONNECTOR.into(),
            "what is 1+1?".into(),
            "owner-key".into(),
        )
    };
    send().await.unwrap();
    let store = {
        let mut guard = state.write().await;
        let reply_id = guard
            .runs
            .find_by_idempotency_key(&agent, "owner-key", 0)
            .and_then(|record| record.reply_message_id.clone())
            .expect("the run saved its reply id");
        let runtime = guard.agents.get_mut(&agent).unwrap();
        let reply = runtime
            .messages()
            .iter()
            .find(|message| message.id == reply_id)
            .cloned()
            .unwrap();
        // Pruned: the store holds it, the hot tail no longer does.
        runtime.retain_messages(|message| message.room_id != ROOM);
        let store = guard.history.store();
        store
            .upsert_messages(&[crate::history::HistoryMessage {
                agent_id: agent.clone(),
                session_id: crate::sessions::session_id_for_room(ROOM),
                hidden: false,
                message: reply,
            }])
            .await
            .unwrap();
        store
    };
    drop(store);

    let (replayed, queued) = send().await.unwrap();

    assert_eq!(replayed.result.status, "success");
    assert_eq!(
        replayed
            .result
            .data
            .as_ref()
            .map(|content| content.text.as_str()),
        Some("It is 2")
    );
    assert!(queued, "its reply is still queued for delivery");
    assert_eq!(model.requests().len(), 1, "the turn did not run again");
    manager.shutdown().await;
}
