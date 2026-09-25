use anima_core::TokenUsage;
use serde_json::json;

use super::events::{
    preview, run_status_event, snapshot_json, LiveEvent, LiveEventBody, SnapshotRun,
};
use super::fanout::{LiveDelivery, LiveHub};
use super::registry::LiveToolView;
use super::{
    DEFAULT_SESSION_EVENT_BUFFER, EVENT_KEEP_ALIVE_SECS, MAX_EVENT_SUBSCRIBERS_PER_AGENT,
    MAX_PREVIEW_BYTES, MAX_SNAPSHOT_TEXT_BYTES,
};
use crate::runs::{RunRecord, RunSource, RunStart, RunStatus, MAX_RUN_STEPS};

fn record(agent_id: &str) -> RunRecord {
    RunRecord::running(
        RunStart {
            agent_id: agent_id.into(),
            session_id: "chat:a".into(),
            source: RunSource::Web,
            source_ref: None,
            idempotency_key: None,
            text: "hello".into(),
            model: "gpt-5.4".into(),
            provider: Some("openai".into()),
            parent_run_id: None,
        },
        10,
    )
}

fn tool(call: &str) -> LiveToolView {
    LiveToolView {
        step_id: "run_1:1".into(),
        tool_call_id: call.into(),
        name: "search".into(),
        arguments_preview: "{}".into(),
        arguments_truncated: false,
        status: "running".into(),
        duration_ms: None,
        result_preview: None,
        truncated: false,
    }
}

#[test]
fn previews_are_cut_on_a_char_boundary_and_flagged() {
    assert_eq!(preview("short"), ("short".to_string(), false));
    let long = format!("{}é", "a".repeat(MAX_PREVIEW_BYTES - 1));
    let (cut, truncated) = preview(&long);
    assert!(truncated);
    assert_eq!(
        cut,
        "a".repeat(MAX_PREVIEW_BYTES - 1),
        "the split character is dropped whole"
    );
}

#[test]
fn events_serialize_their_envelope_and_fields() {
    let delta = LiveEvent::new(
        "agent-1",
        LiveEventBody::StepDelta {
            step_id: "run_1:1".into(),
            offset: 4,
            text: "lo".into(),
        },
    )
    .session("chat:a")
    .run("run_1");
    let value = delta.to_json(7);
    assert_eq!(value["type"], "step.delta");
    assert_eq!(value["agentId"], "agent-1");
    assert_eq!(value["sessionId"], "chat:a");
    assert_eq!(value["runId"], "run_1");
    assert_eq!(value["seq"], 7);
    assert!(value["at"].as_u64().is_some());
    assert_eq!(value["stepId"], "run_1:1");
    assert_eq!(value["offset"], 4);
    assert_eq!(value["text"], "lo");

    let finished = LiveEvent::new(
        "agent-1",
        LiveEventBody::ToolFinished {
            step_id: "run_1:1".into(),
            tool_call_id: "call-1".into(),
            name: "search".into(),
            status: "error",
            duration_ms: 12,
            result_preview: "boom".into(),
            truncated: true,
            recovered: false,
        },
    )
    .to_json(2);
    assert_eq!(finished["type"], "tool.finished");
    assert_eq!(finished["toolCallId"], "call-1");
    assert_eq!(finished["status"], "error");
    assert_eq!(finished["durationMs"], 12);
    assert_eq!(finished["resultPreview"], "boom");
    assert_eq!(finished["truncated"], true);
    assert!(finished.get("sessionId").is_none());

    let mut run = record("agent-1");
    let started = run_status_event(&run).to_json(3);
    assert_eq!(started["type"], "run.started");
    assert_eq!(started["runId"], run.id.as_str());
    assert_eq!(started["sessionId"], "chat:a");
    assert_eq!(started["run"]["status"], "running");
    assert_eq!(started["run"]["source"], "web");
    run.finish(RunStatus::Cancelled, None, 20);
    assert_eq!(run_status_event(&run).body.type_name(), "run.cancelled");
    run.status = RunStatus::Queued;
    assert_eq!(run_status_event(&run).body.type_name(), "run.queued");
    for (status, name) in [
        (RunStatus::Completed, "run.completed"),
        (RunStatus::Failed, "run.failed"),
        (RunStatus::Interrupted, "run.interrupted"),
        (RunStatus::AwaitingApproval, "run.awaiting_approval"),
    ] {
        run.status = status;
        assert_eq!(run_status_event(&run).body.type_name(), name);
    }
}

#[test]
fn the_registry_tracks_a_steps_text_in_utf16_units_and_keeps_its_tail() {
    let hub = LiveHub::new(8);
    let runs = hub.runs();
    let control = runs.register("run_1");
    assert!(!control.cancel.is_cancelled());
    control.cancel.cancel();
    assert!(
        runs.register("run_1").cancel.is_cancelled(),
        "registering again returns the same control"
    );
    assert!(runs.control("run_1").unwrap().cancel.is_cancelled());
    assert!(runs.control("missing").is_none());
    runs.start_step("run_1", "run_1:1");
    assert_eq!(runs.append_text("run_1", "Hé"), Some(0));
    assert_eq!(runs.append_text("run_1", "😀"), Some(2));
    assert_eq!(
        runs.append_text("run_1", "!"),
        Some(4),
        "the emoji is two UTF-16 units"
    );
    assert_eq!(runs.append_text("missing", "x"), None);
    let view = runs.view("run_1").unwrap();
    assert_eq!(view.step_id.as_deref(), Some("run_1:1"));
    assert_eq!(view.text, "Hé😀!");
    assert_eq!(view.text_offset, 0);

    runs.start_step("run_1", "run_1:2");
    let long = "b".repeat(MAX_SNAPSHOT_TEXT_BYTES + 10);
    assert_eq!(runs.append_text("run_1", &long), Some(0));
    let view = runs.view("run_1").unwrap();
    assert_eq!(view.text.len(), MAX_SNAPSHOT_TEXT_BYTES);
    assert_eq!(view.text_offset, 10, "the dropped head is counted");
    assert_eq!(
        runs.append_text("run_1", "c"),
        Some(MAX_SNAPSHOT_TEXT_BYTES as u64 + 10)
    );

    runs.tool_started("run_1", tool("call-1"));
    runs.tool_finished("run_1", "call-1", "success", 5, "ok".into(), false);
    let card = runs.view("run_1").unwrap().tools[0].clone();
    assert_eq!(
        (
            card.status.as_str(),
            card.duration_ms,
            card.result_preview.as_deref()
        ),
        ("success", Some(5), Some("ok"))
    );
    runs.tool_started("run_1", tool("call-2"));
    assert_eq!(
        runs.tools_started("run_1"),
        ["search"],
        "names are distinct"
    );

    for n in 0..(MAX_RUN_STEPS + 3) {
        runs.record_step_usage("run_1", &format!("run_1:{n}"), TokenUsage::default());
    }
    assert_eq!(runs.steps("run_1").len(), MAX_RUN_STEPS);
    runs.note_steer_key("run_1", "key-1", "also this");
    assert_eq!(
        runs.steer_text("run_1", "key-1").as_deref(),
        Some("also this")
    );
    assert_eq!(runs.steer_text("run_1", "key-2"), None);
    assert!(runs.remove("run_1"));
    assert!(runs.view("run_1").is_none());
    assert!(!runs.remove("run_1"));
}

#[test]
fn a_tail_cut_inside_a_character_drops_it_whole_and_counts_its_utf16_units() {
    let hub = LiveHub::new(8);
    let runs = hub.runs();
    runs.register("run_1");
    runs.start_step("run_1", "run_1:1");
    let emoji = "😀".repeat(MAX_SNAPSHOT_TEXT_BYTES / 4);
    let units = MAX_SNAPSHOT_TEXT_BYTES as u64 / 2;
    assert_eq!(runs.append_text("run_1", &emoji), Some(0));
    assert_eq!(
        runs.append_text("run_1", "a"),
        Some(units),
        "each emoji is two units"
    );

    let view = runs.view("run_1").unwrap();
    assert_eq!(view.text_offset, 2, "the first emoji left whole");
    assert_eq!(view.text.len(), MAX_SNAPSHOT_TEXT_BYTES - 3);
    assert!(view.text.ends_with("😀a"));
    assert_eq!(runs.append_text("run_1", "b"), Some(units + 1));
}

#[tokio::test]
async fn events_reach_the_agent_and_its_parent_and_nobody_else() {
    let hub = LiveHub::new(8);
    let mut companion = hub.subscribe("companion").unwrap();
    let mut helper = hub.subscribe("helper").unwrap();
    let mut other = hub.subscribe("other").unwrap();

    hub.publish(
        LiveEvent::new("helper", LiveEventBody::SessionUpdated).session("room-1"),
        Some("companion"),
    );
    hub.publish(
        LiveEvent::new("nobody-watches", LiveEventBody::SessionDeleted),
        None,
    );

    for subscription in [&mut companion, &mut helper] {
        let Some(LiveDelivery::Event(event)) = subscription.next().await else {
            panic!("the event arrives");
        };
        assert_eq!(event.agent_id, "helper");
        assert_eq!(event.body, LiveEventBody::SessionUpdated);
    }
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), other.next())
            .await
            .is_err(),
        "an unrelated stream hears nothing"
    );
}

#[tokio::test]
async fn subscribers_are_capped_per_agent_and_released_on_drop() {
    let hub = LiveHub::new(8);
    let held: Vec<_> = (0..MAX_EVENT_SUBSCRIBERS_PER_AGENT)
        .map(|_| hub.subscribe("agent-1").unwrap())
        .collect();
    assert!(hub.subscribe("agent-1").is_err());
    assert!(hub.subscribe("agent-2").is_ok(), "the cap is per agent");
    drop(held);
    assert_eq!(hub.subscribers("agent-1"), 0);
    assert!(hub.subscribe("agent-1").is_ok());
}

#[tokio::test]
async fn a_lagging_subscription_reports_how_many_events_it_missed() {
    let hub = LiveHub::new(2);
    let mut subscription = hub.subscribe("agent-1").unwrap();
    for n in 0..5u64 {
        hub.publish(
            LiveEvent::new(
                "agent-1",
                LiveEventBody::StepDelta {
                    step_id: "run_1:1".into(),
                    offset: n,
                    text: "x".into(),
                },
            ),
            None,
        );
    }
    assert!(matches!(
        subscription.next().await,
        Some(LiveDelivery::Lagged(3))
    ));
    assert_eq!(hub.lagged_events(), 3);
    let Some(LiveDelivery::Event(event)) = subscription.next().await else {
        panic!("the stream continues after the gap");
    };
    assert!(matches!(
        event.body,
        LiveEventBody::StepDelta { offset: 3, .. }
    ));
}

#[tokio::test]
async fn the_stream_limits_match_the_spec() {
    // Spec §6 and §16. The other tests use the names; this pins the values.
    assert_eq!(MAX_EVENT_SUBSCRIBERS_PER_AGENT, 16);
    assert_eq!(MAX_PREVIEW_BYTES, 2 * 1024);
    assert_eq!(MAX_SNAPSHOT_TEXT_BYTES, 64 * 1024);
    assert_eq!(EVENT_KEEP_ALIVE_SECS, 15);
    assert_eq!(DEFAULT_SESSION_EVENT_BUFFER, 1_024);
    assert_eq!(
        crate::app::DaemonConfig::default().session_event_buffer,
        DEFAULT_SESSION_EVENT_BUFFER
    );

    // A new daemon state's hub keeps that many events per agent.
    let hub = crate::state::DaemonState::new().live;
    let mut subscription = hub.subscribe("agent-1").unwrap();
    for _ in 0..=DEFAULT_SESSION_EVENT_BUFFER {
        hub.publish(
            LiveEvent::new("agent-1", LiveEventBody::SessionUpdated),
            None,
        );
    }
    assert!(
        matches!(subscription.next().await, Some(LiveDelivery::Lagged(1))),
        "one event more than the buffer drops the oldest"
    );
}

#[tokio::test]
async fn closing_the_hub_ends_open_and_later_subscriptions() {
    let hub = LiveHub::new(8);
    let mut open = hub.subscribe("agent-1").unwrap();
    hub.publish(
        LiveEvent::new("agent-1", LiveEventBody::SessionUpdated),
        None,
    );

    hub.close();

    let within = std::time::Duration::from_secs(5);
    assert!(
        tokio::time::timeout(within, open.next())
            .await
            .expect("an open subscription answers at once")
            .is_none(),
        "an open stream ends, even with an event still buffered"
    );
    let mut late = hub.subscribe("agent-1").unwrap();
    assert!(
        tokio::time::timeout(within, late.next())
            .await
            .expect("a later subscription answers at once")
            .is_none(),
        "a stream opened after the close ends at once"
    );
}

#[test]
fn a_snapshot_lists_runs_with_their_live_state() {
    let run = record("agent-1");
    let hub = LiveHub::new(8);
    hub.runs().register(&run.id);
    hub.runs().start_step(&run.id, "run_x:1");
    hub.runs().append_text(&run.id, "Half");
    hub.runs().tool_started(&run.id, tool("call-9"));

    let value = snapshot_json(
        "agent-1",
        1,
        &[SnapshotRun {
            live: hub.runs().view(&run.id),
            record: run.clone(),
        }],
    );

    assert_eq!(value["type"], "stream.snapshot");
    assert_eq!(value["seq"], 1);
    assert_eq!(value["approvals"], json!([]));
    let entry = &value["runs"][0];
    assert_eq!(entry["run"]["id"], run.id.as_str());
    assert_eq!(entry["stepId"], "run_x:1");
    assert_eq!(entry["text"], "Half");
    assert_eq!(entry["textOffset"], 0);
    assert_eq!(entry["tools"][0]["toolCallId"], "call-9");
    assert_eq!(entry["tools"][0]["status"], "running");
}
