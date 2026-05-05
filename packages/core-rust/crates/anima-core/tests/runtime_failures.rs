mod support;

use std::sync::{Arc, Mutex};

use anima_core::{AgentRuntime, Content, EngineEvent, TaskStatus};

use self::support::{
    config, FailingEvaluator, FailingModelAdapter, FailingProvider, FinalAnswerModelAdapter,
};

#[tokio::test]
async fn public_runtime_boundary_marks_failed_when_evaluator_errors() {
    let emitted_events = Arc::new(Mutex::new(Vec::<String>::new()));
    let mut runtime = AgentRuntime::new(config(), Arc::new(FinalAnswerModelAdapter));
    runtime.register_evaluator(Arc::new(FailingEvaluator));
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
        .run(Content {
            text: "Give me the release verdict".into(),
            ..Content::default()
        })
        .await;

    assert_eq!(result.status, TaskStatus::Error);
    assert_eq!(result.error.as_deref(), Some("evaluator exploded"));
    assert_eq!(runtime.snapshot().state.status.as_str(), "failed");

    let emitted_events = emitted_events
        .lock()
        .expect("event recorder should not be poisoned");
    assert!(emitted_events.iter().any(|event| event == "agent:failed"));
    assert!(emitted_events.iter().any(|event| event == "task:failed"));
}

#[tokio::test]
async fn public_runtime_boundary_marks_failed_when_provider_context_building_fails() {
    let emitted_events = Arc::new(Mutex::new(Vec::<String>::new()));
    let mut runtime = AgentRuntime::new(config(), Arc::new(FinalAnswerModelAdapter));
    runtime.register_provider(Arc::new(FailingProvider));
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
        .run(Content {
            text: "Build provider context first".into(),
            ..Content::default()
        })
        .await;

    assert_eq!(result.status, TaskStatus::Error);
    assert_eq!(
        result.error.as_deref(),
        Some("provider context unavailable")
    );

    let snapshot = runtime.snapshot();
    assert_eq!(snapshot.state.status.as_str(), "failed");
    assert_eq!(snapshot.message_count, 1);
    assert_eq!(
        snapshot.messages[0].content.text,
        "Build provider context first"
    );

    let emitted_events = emitted_events
        .lock()
        .expect("event recorder should not be poisoned");
    assert!(emitted_events.iter().any(|event| event == "agent:started"));
    assert!(emitted_events.iter().any(|event| event == "task:started"));
    assert!(emitted_events.iter().any(|event| event == "agent:failed"));
    assert!(emitted_events.iter().any(|event| event == "task:failed"));
}

#[tokio::test]
async fn public_runtime_boundary_marks_failed_when_model_generation_fails() {
    let emitted_events = Arc::new(Mutex::new(Vec::<String>::new()));
    let mut runtime = AgentRuntime::new(config(), Arc::new(FailingModelAdapter));
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
        .run(Content {
            text: "Talk to the model".into(),
            ..Content::default()
        })
        .await;

    assert_eq!(result.status, TaskStatus::Error);
    assert_eq!(result.error.as_deref(), Some("model backend offline"));

    let snapshot = runtime.snapshot();
    assert_eq!(snapshot.state.status.as_str(), "failed");
    assert_eq!(snapshot.message_count, 1);
    assert_eq!(snapshot.messages[0].content.text, "Talk to the model");

    let emitted_events = emitted_events
        .lock()
        .expect("event recorder should not be poisoned");
    assert!(emitted_events.iter().any(|event| event == "agent:started"));
    assert!(emitted_events.iter().any(|event| event == "task:started"));
    assert!(emitted_events.iter().any(|event| event == "agent:failed"));
    assert!(emitted_events.iter().any(|event| event == "task:failed"));
}
