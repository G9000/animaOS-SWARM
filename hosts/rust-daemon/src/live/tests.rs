use anima_core::TokenUsage;
use serde_json::json;

use super::events::{
    preview, run_status_event, snapshot_json, LiveEvent, LiveEventBody, SnapshotRun,
};
use super::fanout::{LiveDelivery, LiveHub};
use super::registry::LiveToolView;
use super::{
    DEFAULT_SESSION_EVENT_BUFFER, DELTA_FLUSH_BYTES, DELTA_FLUSH_MS, EVENT_KEEP_ALIVE_SECS,
    MAX_EVENT_SUBSCRIBERS_PER_AGENT, MAX_LIVE_TOOL_CARDS, MAX_PREVIEW_BYTES,
    MAX_SNAPSHOT_TEXT_BYTES,
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
fn a_run_keeps_only_its_newest_tool_cards() {
    let hub = LiveHub::new(8);
    let runs = hub.runs();
    runs.register("run_1");
    let started = MAX_LIVE_TOOL_CARDS + 3;
    for n in 0..started {
        runs.tool_started("run_1", tool(&format!("call-{n}")));
    }
    runs.tool_finished("run_1", "call-0", "success", 1, "gone".into(), false);
    runs.tool_finished(
        "run_1",
        &format!("call-{}", started - 1),
        "error",
        2,
        "boom".into(),
        false,
    );

    let tools = runs.view("run_1").unwrap().tools;
    assert_eq!(tools.len(), MAX_LIVE_TOOL_CARDS);
    assert_eq!(tools[0].tool_call_id, "call-3", "the oldest cards go first");
    assert!(
        tools.iter().all(|card| card.tool_call_id != "call-0"),
        "finishing a dropped card brings nothing back"
    );
    let newest = tools.last().unwrap();
    assert_eq!(newest.tool_call_id, format!("call-{}", started - 1));
    assert_eq!(newest.status, "error");
    assert_eq!(runs.tools_started("run_1"), ["search"]);
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
async fn the_live_run_limits_match_the_spec() {
    // Spec §4.1, §6, and §16. The other tests use the names; this pins the values.
    assert_eq!(MAX_EVENT_SUBSCRIBERS_PER_AGENT, 16);
    assert_eq!(MAX_PREVIEW_BYTES, 2 * 1024);
    assert_eq!(MAX_SNAPSHOT_TEXT_BYTES, 64 * 1024);
    assert_eq!(EVENT_KEEP_ALIVE_SECS, 15);
    assert_eq!(DEFAULT_SESSION_EVENT_BUFFER, 1_024);
    assert_eq!(MAX_RUN_STEPS, 50, "a run keeps the usage of 50 model calls");
    assert_eq!(DELTA_FLUSH_MS, 50);
    assert_eq!(DELTA_FLUSH_BYTES, 512);
    assert_eq!(MAX_LIVE_TOOL_CARDS, 50);
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

mod coalescing {
    use std::time::Duration;

    use anima_core::RunFrame;

    use super::record;
    use crate::agent_runs::test_support::next_event;
    use crate::live::fanout::{LiveDelivery, LiveHub};
    use crate::live::observer::{DeltaChunk, DeltaCoalescer, LiveRun};
    use crate::live::{LiveEventBody, DELTA_FLUSH_BYTES, DELTA_FLUSH_MS};
    use crate::runs::RunStatus;

    fn chunk(step_id: &str, offset: u64, text: &str) -> DeltaChunk {
        DeltaChunk {
            step_id: step_id.into(),
            offset,
            text: text.into(),
        }
    }

    #[test]
    fn small_quick_deltas_wait_and_join_into_one_chunk() {
        let mut coalescer = DeltaCoalescer::default();
        assert!(coalescer.push("run_1:1", 0, "Hel", 1_000).is_empty());
        assert!(coalescer.push("run_1:1", 3, "lo", 1_010).is_empty());
        assert!(!coalescer.is_empty());
        assert_eq!(coalescer.take(), Some(chunk("run_1:1", 0, "Hello")));
        assert!(coalescer.is_empty());
        assert_eq!(coalescer.take(), None);
    }

    #[test]
    fn a_full_or_old_buffer_is_due_at_once() {
        let mut coalescer = DeltaCoalescer::default();
        let big = "x".repeat(DELTA_FLUSH_BYTES);
        assert_eq!(
            coalescer.push("run_1:1", 0, &big, 1_000),
            [chunk("run_1:1", 0, &big)]
        );
        assert!(coalescer.push("run_1:1", 512, "a", 2_000).is_empty());
        assert_eq!(
            coalescer.push("run_1:1", 513, "b", 2_000 + DELTA_FLUSH_MS),
            [chunk("run_1:1", 512, "ab")]
        );
    }

    #[test]
    fn a_new_step_flushes_the_previous_steps_text_first() {
        let mut coalescer = DeltaCoalescer::default();
        assert!(coalescer.push("run_1:1", 0, "first", 1_000).is_empty());
        assert_eq!(
            coalescer.push("run_1:2", 0, "second", 1_001),
            [chunk("run_1:1", 0, "first")]
        );
        assert_eq!(coalescer.take(), Some(chunk("run_1:2", 0, "second")));
    }

    #[tokio::test]
    async fn a_quiet_stream_is_flushed_by_the_timer_and_other_events_follow_its_text() {
        let hub = LiveHub::new(16);
        let run = record("agent-1");
        let mut subscription = hub.subscribe("agent-1").unwrap();
        let live_run = LiveRun::register(hub.clone(), &run, None);
        let observer = live_run.observer();
        let step_id = format!("{}:1", run.id);

        observer.on_frame(RunFrame::StepStarted {
            step_id: step_id.clone(),
        });
        observer.on_frame(RunFrame::TextDelta {
            step_id: step_id.clone(),
            text: "Hi".into(),
        });
        let delivery = tokio::time::timeout(Duration::from_secs(1), subscription.next())
            .await
            .expect("the timer flushes within a second");
        let Some(LiveDelivery::Event(event)) = delivery else {
            panic!("a delta arrives");
        };
        assert_eq!(
            event.body,
            LiveEventBody::StepDelta {
                step_id: step_id.clone(),
                offset: 0,
                text: "Hi".into(),
            }
        );
        assert_eq!(event.run_id.as_deref(), Some(run.id.as_str()));

        observer.on_frame(RunFrame::TextDelta {
            step_id: step_id.clone(),
            text: " there".into(),
        });
        live_run.publish_record(&run);
        // Buffered text comes first.
        let delta = next_event(&mut subscription).await;
        assert!(matches!(
            &delta.body,
            LiveEventBody::StepDelta { offset: 2, text, .. } if text == " there"
        ));
        // Then the run event.
        let started = next_event(&mut subscription).await;
        assert_eq!(started.body.type_name(), "run.started");

        drop(observer);
        drop(live_run);
        assert!(
            hub.runs().view(&run.id).is_none(),
            "dropping the run forgets it"
        );
    }

    #[tokio::test]
    async fn a_run_that_ends_early_sends_its_buffered_text_before_its_result() {
        let hub = LiveHub::new(16);
        let mut run = record("agent-1");
        let mut subscription = hub.subscribe("agent-1").unwrap();
        let live_run = LiveRun::register(hub.clone(), &run, None);
        let observer = live_run.observer();
        let step_id = format!("{}:1", run.id);
        observer.on_frame(RunFrame::StepStarted {
            step_id: step_id.clone(),
        });
        observer.on_frame(RunFrame::TextDelta {
            step_id: step_id.clone(),
            text: "partial".into(),
        });

        // An aborted run task: its live link goes, then its guard fails it.
        drop(live_run);
        run.finish(RunStatus::Failed, None, 20);
        hub.publish(crate::live::run_status_event(&run), None);

        // The buffered text arrives...
        let first = next_event(&mut subscription).await;
        assert!(matches!(
            &first.body,
            LiveEventBody::StepDelta { text, .. } if text == "partial"
        ));
        // ...then the result.
        let second = next_event(&mut subscription).await;
        assert_eq!(second.body.type_name(), "run.failed");
        observer.on_frame(RunFrame::TextDelta {
            step_id,
            text: "late".into(),
        });
        assert!(
            tokio::time::timeout(
                Duration::from_millis(DELTA_FLUSH_MS * 3),
                subscription.next()
            )
            .await
            .is_err(),
            "a forgotten run publishes nothing more"
        );
    }

    #[tokio::test]
    async fn a_flushed_emoji_moves_the_next_deltas_offset_by_two_utf16_units() {
        let hub = LiveHub::new(16);
        let run = record("agent-1");
        let mut subscription = hub.subscribe("agent-1").unwrap();
        let live_run = LiveRun::register(hub.clone(), &run, None);
        let observer = live_run.observer();
        let step_id = format!("{}:1", run.id);
        observer.on_frame(RunFrame::StepStarted {
            step_id: step_id.clone(),
        });

        for text in ["😀", "a"] {
            observer.on_frame(RunFrame::TextDelta {
                step_id: step_id.clone(),
                text: text.into(),
            });
            live_run.flush();
        }

        for (offset, text) in [(0, "😀"), (2, "a")] {
            assert_eq!(
                next_event(&mut subscription).await.body,
                LiveEventBody::StepDelta {
                    step_id: step_id.clone(),
                    offset,
                    text: text.into(),
                }
            );
        }
        let view = hub.runs().view(&run.id).unwrap();
        assert_eq!((view.text.as_str(), view.text_offset), ("😀a", 0));
    }

    #[tokio::test]
    async fn a_run_announces_one_terminal_event_however_many_are_sent() {
        let hub = LiveHub::new(16);
        let mut run = record("agent-1");
        let mut subscription = hub.subscribe("agent-1").unwrap();
        let live_run = LiveRun::register(hub.clone(), &run, None);
        let end = live_run.end();
        live_run.publish_record(&run);
        assert!(!end.ended(), "run.started is not an end");

        run.finish(RunStatus::Failed, None, 20);
        live_run.publish_record(&run);
        assert!(end.ended());
        live_run.publish_record(&run);
        run.status = RunStatus::Completed;
        end.publish_record(&run);
        drop(live_run);
        end.publish_record(&run);

        let names: Vec<&str> = [
            next_event(&mut subscription).await,
            next_event(&mut subscription).await,
        ]
        .iter()
        .map(|event| event.body.type_name())
        .collect();
        assert_eq!(names, ["run.started", "run.failed"]);
        assert!(
            tokio::time::timeout(
                Duration::from_millis(DELTA_FLUSH_MS * 3),
                subscription.next()
            )
            .await
            .is_err(),
            "no second terminal event"
        );
    }
}
