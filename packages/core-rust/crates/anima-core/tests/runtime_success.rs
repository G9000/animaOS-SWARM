mod support;

use std::sync::{Arc, Mutex};

use anima_core::{
    AgentRuntime, AgentState, Content, DataValue, EngineEvent, Message, StepStatus, TaskResult,
    TaskStatus, ToolCall,
};

use self::support::{
    config, retry_metadata, ClockProvider, RecordingDatabase, RecordingEvaluator,
    ScriptedModelAdapter, UnknownToolModelAdapter,
};

#[tokio::test]
async fn public_runtime_boundary_runs_with_provider_evaluator_and_persistence() {
    let database = Arc::new(RecordingDatabase::default());
    let evaluator_calls = Arc::new(Mutex::new(Vec::new()));
    let emitted_events = Arc::new(Mutex::new(Vec::<String>::new()));

    let mut runtime = AgentRuntime::new(config(), Arc::new(ScriptedModelAdapter));
    runtime.set_database(database.clone());
    runtime.register_provider(Arc::new(ClockProvider));
    runtime.register_evaluator(Arc::new(RecordingEvaluator {
        calls: evaluator_calls.clone(),
    }));
    runtime.set_event_listener(Arc::new({
        let emitted_events = emitted_events.clone();
        move |event: EngineEvent| {
            emitted_events
                .lock()
                .expect("event recorder should not be poisoned")
                .push(event.event_type.as_str().to_string());
        }
    }));
    runtime.init();

    let result = runtime
        .run_with_tools(
            Content {
                text: "Are we ready for first release?".into(),
                ..Content::default()
            },
            |state: AgentState, message: Message, tool_call: ToolCall| async move {
                assert_eq!(state.name, "release-agent");
                assert_eq!(message.content.text, "Are we ready for first release?");
                assert_eq!(tool_call.name, "memory_search");

                TaskResult::success(
                    Content {
                        text: "memory says: ship a scoped v0.x".into(),
                        ..Content::default()
                    },
                    1,
                )
            },
        )
        .await;

    assert_eq!(result.status, TaskStatus::Success);
    assert_eq!(
        result.data.as_ref().map(|content| content.text.as_str()),
        Some("release-agent finalized with memory says: ship a scoped v0.x")
    );

    let snapshot = runtime.snapshot();
    assert_eq!(snapshot.state.status.as_str(), "completed");
    assert_eq!(snapshot.message_count, 4);
    assert_eq!(snapshot.event_count, runtime.events().len());
    assert!(snapshot.state.token_usage.total_tokens >= 17);

    let evaluator_calls = evaluator_calls
        .lock()
        .expect("evaluator call recorder should not be poisoned");
    assert_eq!(
        evaluator_calls.as_slice(),
        ["release-agent finalized with memory says: ship a scoped v0.x"]
    );

    let statuses = database.statuses();
    assert_eq!(statuses, [StepStatus::Done]);

    let emitted_events = emitted_events
        .lock()
        .expect("event recorder should not be poisoned");
    assert!(emitted_events.iter().any(|event| event == "agent:spawned"));
    assert!(emitted_events.iter().any(|event| event == "tool:before"));
    assert!(emitted_events.iter().any(|event| event == "tool:after"));
    assert!(emitted_events.iter().any(|event| event == "task:completed"));
}

#[tokio::test]
async fn public_runtime_boundary_surfaces_unknown_tool_results_via_run() {
    let mut runtime = AgentRuntime::new(config(), Arc::new(UnknownToolModelAdapter));
    runtime.init();

    let result = runtime
        .run(Content {
            text: "Use whatever tool you need".into(),
            ..Content::default()
        })
        .await;

    assert_eq!(result.status, TaskStatus::Success);
    let output = result
        .data
        .as_ref()
        .map(|content| content.text.as_str())
        .expect("assistant output should exist");
    assert!(output.contains("Unknown tool: missing_tool"));

    let tool_message = runtime
        .messages()
        .iter()
        .find(|message| message.role == anima_core::MessageRole::Tool)
        .expect("tool message should be recorded");
    assert!(tool_message
        .content
        .text
        .contains("Unknown tool: missing_tool"));
    assert_eq!(runtime.snapshot().state.status.as_str(), "completed");
}

#[tokio::test]
async fn public_runtime_boundary_recovers_retry_runs_from_persistence() {
    let database = Arc::new(RecordingDatabase::default());
    let mut runtime = AgentRuntime::new(config(), Arc::new(ScriptedModelAdapter));
    runtime.set_database(database.clone());
    runtime.register_provider(Arc::new(ClockProvider));
    runtime.init();

    let first = runtime
        .run_with_tools(
            Content {
                text: "Retry the same release question".into(),
                metadata: Some(retry_metadata("release-retry")),
                ..Content::default()
            },
            |_, _, tool_call| async move {
                assert_eq!(tool_call.name, "memory_search");
                TaskResult::success(
                    Content {
                        text: "memory says: recovered answer".into(),
                        ..Content::default()
                    },
                    1,
                )
            },
        )
        .await;

    let second = runtime
        .run_with_tools(
            Content {
                text: "Retry the same release question".into(),
                metadata: Some(retry_metadata("release-retry")),
                ..Content::default()
            },
            |_, _, _| async move {
                panic!("replayed retry should reuse the persisted tool result")
            },
        )
        .await;

    assert_eq!(first.status, TaskStatus::Success);
    assert_eq!(second.status, TaskStatus::Success);
    assert_eq!(database.statuses(), [StepStatus::Done]);
    assert_eq!(
        second.data.as_ref().map(|content| content.text.as_str()),
        Some("release-agent finalized with memory says: recovered answer")
    );

    let recovered_tool_message = runtime
        .messages()
        .iter()
        .rev()
        .find(|message| message.role == anima_core::MessageRole::Tool)
        .expect("a tool message should exist after the retry run");
    assert_eq!(
        recovered_tool_message
            .content
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("recoveredFromPersistence")),
        Some(&DataValue::Bool(true))
    );
}
