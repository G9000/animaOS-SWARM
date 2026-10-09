//! The queue cap counts only steers the model has not read (M8 Tasks 5–8
//! review): a steer already in the run's transcript waits for nothing.

use super::steer_tests::{accept, message, wait_for_key};
use super::test_support::{add_chat, calculate_call, coordinator_with, Gate, ScriptedModel, Step};
use super::{AcceptedRun, SessionRunMode};
use crate::runs::RunStatus;

/// Eight steers the model read during a long run no longer wait, so a ninth
/// steer and a new message are both accepted.
#[tokio::test]
async fn steers_the_model_read_do_not_fill_the_queue() {
    let gate = Gate::new();
    let model = ScriptedModel::gated(
        vec![
            Step::Tools(vec![calculate_call("call-1", "1+1")]),
            Step::Tools(vec![calculate_call("call-2", "2+2")]),
            Step::Text(vec!["Done"]),
            Step::Text(vec!["Next"]),
        ],
        gate.clone(),
    );
    let (coordinator, agent_id) = coordinator_with(model.clone()).await;
    add_chat(&coordinator, &agent_id, "chat:s").await;
    let AcceptedRun::Created(first) = accept(
        &coordinator,
        message(&agent_id, "key-0", "hello", SessionRunMode::Queue),
    )
    .await
    else {
        panic!("the first message is queued");
    };
    gate.entered().await;
    for n in 1..=8 {
        let key = format!("steer-{n}");
        let AcceptedRun::Steered(_) = accept(
            &coordinator,
            message(&agent_id, &key, &key, SessionRunMode::Steer),
        )
        .await
        else {
            panic!("{key} joins the run");
        };
    }
    // The second model call reads all eight.
    gate.release();
    gate.entered().await;
    assert_eq!(model.requests().len(), 2);
    {
        let guard = coordinator.state.read().await;
        assert_eq!(
            guard.runs.get(&first.id).unwrap().pending_steers.len(),
            8,
            "kept until the run's commit"
        );
        assert_eq!(super::queue::waiting_count(&guard, &agent_id, None), 0);
    }

    let AcceptedRun::Steered(joined) = accept(
        &coordinator,
        message(&agent_id, "steer-9", "steer-9", SessionRunMode::Steer),
    )
    .await
    else {
        panic!("a ninth steer joins the run");
    };
    assert_eq!(joined.id, first.id);
    let AcceptedRun::Created(next) = accept(
        &coordinator,
        message(&agent_id, "key-1", "and then", SessionRunMode::Queue),
    )
    .await
    else {
        panic!("a new message is queued");
    };
    assert_eq!(
        super::queue::waiting_count(&*coordinator.state.read().await, &agent_id, None),
        2
    );

    gate.release.add_permits(20);
    wait_for_key(&coordinator, &agent_id, "key-0", RunStatus::Completed).await;
    wait_for_key(&coordinator, &agent_id, "key-1", RunStatus::Completed).await;
    let guard = coordinator.state.read().await;
    assert!(guard.runs.get(&first.id).unwrap().pending_steers.is_empty());
    assert_eq!(
        guard.runs.get(&next.id).unwrap().status,
        RunStatus::Completed
    );
    assert!(
        guard
            .runs
            .find_by_idempotency_key(&agent_id, "steer-9", 0)
            .is_none(),
        "the ninth steer joined the run's transcript"
    );
}

/// Three steers the run read and one it did not, with five messages
/// queued: the read ones take no slot, so the unread one is queued.
#[tokio::test]
async fn read_steers_take_no_open_slot_when_the_run_ends() {
    let gate = Gate::new();
    let model = ScriptedModel::gated(
        vec![
            Step::Tools(vec![calculate_call("call-1", "1+1")]),
            Step::Text(vec!["Done"]),
        ],
        gate.clone(),
    );
    let (coordinator, agent_id) = coordinator_with(model).await;
    add_chat(&coordinator, &agent_id, "chat:s").await;
    accept(
        &coordinator,
        message(&agent_id, "key-0", "hello", SessionRunMode::Queue),
    )
    .await;
    gate.entered().await;
    for n in 1..=5 {
        let key = format!("queued-{n}");
        let AcceptedRun::Created(_) = accept(
            &coordinator,
            message(&agent_id, &key, &key, SessionRunMode::Queue),
        )
        .await
        else {
            panic!("{key} is queued");
        };
    }
    for key in ["steer-1", "steer-2", "steer-3"] {
        let AcceptedRun::Steered(_) = accept(
            &coordinator,
            message(&agent_id, key, key, SessionRunMode::Steer),
        )
        .await
        else {
            panic!("{key} joins the run");
        };
    }
    // The last model call reads those three; a fourth arrives during it.
    gate.release();
    gate.entered().await;
    let AcceptedRun::Steered(_) = accept(
        &coordinator,
        message(&agent_id, "steer-4", "steer-4", SessionRunMode::Steer),
    )
    .await
    else {
        panic!("steer-4 joins the run");
    };

    gate.release.add_permits(20);
    wait_for_key(&coordinator, &agent_id, "key-0", RunStatus::Completed).await;
    let unread = wait_for_key(&coordinator, &agent_id, "steer-4", RunStatus::Completed).await;
    assert!(unread.error.is_none(), "queued, not offered again");
    assert_eq!(unread.input.text, "steer-4");
    wait_for_key(&coordinator, &agent_id, "queued-5", RunStatus::Completed).await;
}
