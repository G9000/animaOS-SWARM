use super::*;

use std::collections::BTreeMap;

use anima_core::{DataValue, MessageRole, ToolCall};
use serde_json::Value;

use crate::agent_runs::test_support::companion_config;
use crate::approvals::{ApprovalRequest, PendingApprovalStart};
use crate::history::conformance::{history_message, FlakyHistoryStore};
use crate::history::HistoryService;
use crate::live::{LiveEvent, LiveEventBody, LiveHub};
use crate::logs::{LogBuffer, LogLevel, LOG_BUFFER_LINES};
use crate::routes::status::{collect, MAX_STATUS_CONNECTORS, MAX_STATUS_ERROR_CHARS};
use crate::runs::{RunRecord, RunSource, RunStart};
use crate::schedules::{
    AutomationCounters, AutomationCreator, ScheduleTarget, ScheduleTrigger, ScheduledPromptRecord,
};

const OWNER_ORIGIN: &str = "http://localhost:4200";

fn get(uri: &str, origin: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", "127.0.0.1:8080");
    if let Some(origin) = origin {
        builder = builder.header("origin", origin);
    }
    builder.body(Body::empty()).unwrap()
}

async fn json_of(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

async fn text_of(response: axum::response::Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

fn daemon() -> (DaemonState, String) {
    let mut daemon = DaemonState::new();
    let agent = daemon
        .create_agent(companion_config("companion"))
        .unwrap()
        .state
        .id;
    (daemon, agent)
}

fn app_for(state: &Arc<RwLock<DaemonState>>, buffer: &Arc<LogBuffer>) -> axum::Router {
    crate::routes::router_with_logs(
        Arc::clone(state),
        DaemonConfig::default(),
        Arc::clone(buffer),
    )
}

async fn status_of(app: &axum::Router) -> Value {
    let response = app
        .clone()
        .oneshot(get("/api/status", Some(OWNER_ORIGIN)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    json_of(response).await
}

async fn metrics_of(app: &axum::Router) -> String {
    let response = app
        .clone()
        .oneshot(get("/metrics", Some(OWNER_ORIGIN)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    text_of(response).await
}

fn run_start(agent_id: &str) -> RunStart {
    RunStart {
        agent_id: agent_id.into(),
        session_id: "chat:plans".into(),
        source: RunSource::Web,
        source_ref: None,
        idempotency_key: None,
        text: "a private prompt".into(),
        model: "gpt-5.4".into(),
        provider: None,
        parent_run_id: None,
    }
}

fn schedule(
    id: &str,
    agent_id: &str,
    enabled: bool,
    counters: AutomationCounters,
) -> ScheduledPromptRecord {
    ScheduledPromptRecord {
        id: id.into(),
        import_idempotency_key: None,
        agent_id: agent_id.into(),
        prompt: "a private automation prompt".into(),
        trigger: ScheduleTrigger::Interval {
            interval_ms: 60_000,
        },
        enabled,
        target: ScheduleTarget::Workspace,
        next_due_at_ms: 10,
        last_fired: None,
        last_safe_outcome: None,
        created_at_ms: 1,
        updated_at_ms: 1,
        name: id.into(),
        active_hours: None,
        created_by: AutomationCreator::Owner,
        preset: None,
        counters,
    }
}

#[tokio::test]
async fn status_requires_the_owner() {
    let (daemon, _) = daemon();
    let app = app_for(&Arc::new(RwLock::new(daemon)), &LogBuffer::new());
    let response = app
        .oneshot(get("/api/status", Some("https://untrusted.example")))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(response.headers()["cache-control"], "no-store");
}

#[tokio::test]
async fn status_reports_version_uptime_and_readiness() {
    let (daemon, _) = daemon();
    let state = Arc::new(RwLock::new(daemon));
    let runs = AgentRunCoordinator::new(Arc::clone(&state), Arc::new(Semaphore::new(4)));
    let manager = ConnectorManager::new(
        Arc::clone(&state),
        runs,
        Arc::new(InMemoryCredentialStore::default()),
        Arc::new(CountingTelegramTransport::default()),
    );
    let snapshot = collect(
        &state,
        &DaemonConfig::default(),
        &LogBuffer::new(),
        &manager,
        1_000_000,
        1_090_999,
    )
    .await;
    let status = &snapshot.response;
    assert_eq!(status.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(
        (status.started_at_ms, status.now_ms, status.uptime_seconds),
        (1_000_000, 1_090_999, 90)
    );
    assert_eq!(status.readiness.status, "ready");
    assert!(status.readiness.issues.is_empty());
    assert_eq!(status.storage.persistence_mode, "memory");
    assert_eq!(status.storage.control_plane, "memory");
    assert_eq!(status.storage.control_plane_durability, "ephemeral");
    assert!(status.storage.history.ephemeral);
    assert!(status.storage.history.healthy);

    let before_start = collect(
        &state,
        &DaemonConfig::default(),
        &LogBuffer::new(),
        &manager,
        2_000_000,
        1_000,
    )
    .await;
    assert_eq!(before_start.response.uptime_seconds, 0, "never negative");
}

#[tokio::test]
async fn status_lists_providers_without_keys() {
    let (daemon, _) = daemon();
    let app = app_for(&Arc::new(RwLock::new(daemon)), &LogBuffer::new());
    let body = status_of(&app).await;
    let providers = body["providers"].as_array().unwrap();
    assert!(providers.iter().any(|provider| provider["id"] == "chatgpt"));
    for provider in providers {
        let mut keys = provider
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        keys.sort();
        assert_eq!(keys, ["configured", "id", "label"]);
    }
    let text = body.to_string();
    assert!(!text.contains("apiKey"), "{text}");
    assert!(!text.contains("api_key"), "{text}");
}

#[tokio::test]
async fn status_lists_connectors_without_credentials() {
    let (mut daemon, agent) = daemon();
    daemon.connectors.insert(
        "telegram-a".into(),
        TelegramConnectorRecord {
            id: "telegram-a".into(),
            agent_id: agent.clone(),
            room_id: "telegram-room-a".into(),
            bot: TelegramBotIdentity {
                id: "7".into(),
                username: Some("status_bot".into()),
                display_name: None,
            },
            approved_chat: None,
            pending_pairing: Some(TelegramPendingPairing {
                chat: TelegramChatMetadata {
                    id: "chat-1".into(),
                    kind: TelegramChatKind::Private,
                    title: None,
                    username: Some("someone".into()),
                },
                requested_at_ms: 1,
            }),
            next_update_id: 0,
            enabled: true,
            deleted_at_ms: None,
            created_at_ms: 1,
            updated_at_ms: 1,
        },
    );
    let state = Arc::new(RwLock::new(daemon));
    let credentials = Arc::new(InMemoryCredentialStore::default());
    credentials
        .put(
            "telegram-a",
            TelegramBotToken::parse("42:SECRET-TOKEN-VALUE").unwrap(),
        )
        .await
        .unwrap();
    let limiter = Arc::new(Semaphore::new(4));
    let runs = AgentRunCoordinator::new(Arc::clone(&state), Arc::clone(&limiter));
    let manager = ConnectorManager::new(
        Arc::clone(&state),
        runs.clone(),
        credentials,
        Arc::new(CountingTelegramTransport::default()),
    );
    let app = router_with_services(
        Arc::clone(&state),
        DaemonConfig::default(),
        limiter,
        runs,
        manager,
        true,
    );
    let body = status_of(&app).await;
    assert_eq!(
        body["connectors"],
        serde_json::json!([{
            "id": "telegram-a",
            "agentId": agent,
            "type": "telegram",
            "status": "pairing",
            "enabled": true
        }])
    );
    let text = body.to_string();
    for secret in ["SECRET-TOKEN-VALUE", "42:", "status_bot", "chat-1"] {
        assert!(!text.contains(secret), "{secret} leaked: {text}");
    }
    assert_eq!(MAX_STATUS_CONNECTORS, 50);
}

#[tokio::test]
async fn status_counts_pending_approvals_runs_and_subscribers() {
    let (mut daemon, agent) = daemon();
    daemon
        .runs
        .insert(RunRecord::running(run_start(&agent), 10));
    daemon.runs.insert(RunRecord::queued(run_start(&agent), 11));
    let call = ToolCall {
        id: "call-1".into(),
        name: "memory_add".into(),
        args: BTreeMap::from([("content".to_string(), DataValue::String("the plan".into()))]),
    };
    daemon.approvals.insert(ApprovalRequest::pending(
        PendingApprovalStart {
            agent_id: &agent,
            session_id: "chat:plans",
            run_id: "run_x",
            call: &call,
            timeout_ms: 60_000,
        },
        20,
    ));
    let hub = daemon.live.clone();
    let _stream = hub.subscribe(&agent).unwrap();
    let state = Arc::new(RwLock::new(daemon));
    let app = app_for(&state, &LogBuffer::new());

    let body = status_of(&app).await;
    assert_eq!(body["approvals"]["pending"], 1);
    assert_eq!(body["runs"]["running"], 1);
    assert_eq!(body["runs"]["queued"], 1);
    assert_eq!(body["runs"]["byStatus"]["running"], 1);
    assert_eq!(body["runs"]["byStatus"]["queued"], 1);
    assert_eq!(body["runs"]["byStatus"]["completed"], 0);
    assert_eq!(body["events"]["subscribers"], 1);
    assert_eq!(body["events"]["laggedEvents"], 0);
}

#[tokio::test]
async fn status_reports_history_health_and_redacts_the_last_error() {
    let store = Arc::new(FlakyHistoryStore::new());
    store.set_failure_text(&format!(
        "connection to postgres failed with key sk-proj-AbCdEfGh12345678 {}",
        "x".repeat(2 * MAX_STATUS_ERROR_CHARS)
    ));
    let history = HistoryService::new(store.clone());
    let (mut daemon, agent) = daemon();
    daemon.set_history(Arc::clone(&history));
    let state = Arc::new(RwLock::new(daemon));
    let app = app_for(&state, &LogBuffer::new());

    let healthy = status_of(&app).await;
    assert_eq!(healthy["storage"]["history"]["healthy"], true);
    assert_eq!(healthy["storage"]["history"]["lastError"], Value::Null);
    assert_eq!(healthy["storage"]["history"]["store"], "flaky");

    history.enqueue_committed(
        &agent,
        "chat:one",
        &[history_message("msg-1", &agent, "chat:one", MessageRole::User, "hi", 1).message],
    );
    store.set_failing(true);
    let transactions = tokio::sync::Mutex::new(());
    assert!(history
        .flush_once(&state, &transactions, 5_000)
        .await
        .is_err());

    let failing = status_of(&app).await;
    let history_status = &failing["storage"]["history"];
    assert_eq!(history_status["healthy"], false);
    assert_eq!(history_status["failingSinceMs"], 5_000);
    assert_eq!(history_status["flushErrors"], 1);
    assert_eq!(history_status["pendingFlush"], 1);
    let last_error = history_status["lastError"].as_str().unwrap();
    assert!(last_error.contains("[redacted]"), "{last_error}");
    assert!(!last_error.contains("sk-proj"), "{last_error}");
    assert!(last_error.chars().count() <= MAX_STATUS_ERROR_CHARS);
}

#[tokio::test]
async fn status_counts_automations_and_failures() {
    let (mut daemon, agent) = daemon();
    for (id, enabled, failures, consecutive) in
        [("a", true, 3, 2), ("b", true, 1, 0), ("c", false, 4, 1)]
    {
        daemon.schedules.insert(
            id.into(),
            schedule(
                id,
                &agent,
                enabled,
                AutomationCounters {
                    runs: 10,
                    failures,
                    consecutive_failures: consecutive,
                },
            ),
        );
    }
    let app = app_for(&Arc::new(RwLock::new(daemon)), &LogBuffer::new());
    let body = status_of(&app).await;
    assert_eq!(
        body["automations"],
        serde_json::json!({ "total": 3, "enabled": 2, "failing": 2, "failuresTotal": 8 })
    );
}

#[tokio::test]
async fn status_reports_the_log_buffer() {
    let (daemon, _) = daemon();
    let buffer = LogBuffer::new();
    buffer.push(1, LogLevel::Info, "t", "one");
    buffer.push(2, LogLevel::Info, "t", "two");
    let app = app_for(&Arc::new(RwLock::new(daemon)), &buffer);
    let body = status_of(&app).await;
    assert_eq!(
        body["logs"],
        serde_json::json!({ "buffered": 2, "newestSeq": 2 })
    );
}

#[test]
fn status_is_in_the_openapi_document() {
    use utoipa::OpenApi;

    let document = crate::routes::ApiDoc::openapi();
    let operation = document.paths.paths["/api/status"]
        .get
        .as_ref()
        .expect("/api/status is documented");
    assert_eq!(operation.tags.as_deref(), Some(&["status".to_string()][..]));
    assert!(document
        .tags
        .unwrap_or_default()
        .iter()
        .any(|tag| tag.name == "status"));
}

#[tokio::test]
async fn limits_match_the_constants() {
    let (daemon, _) = daemon();
    let config = DaemonConfig::default();
    let app = app_for(&Arc::new(RwLock::new(daemon)), &LogBuffer::new());
    let body = status_of(&app).await;
    assert_eq!(
        body["limits"],
        serde_json::json!({
            "maxRequestBytes": config.max_request_bytes,
            "maxConcurrentRuns": config.max_concurrent_runs,
            "maxRunsPerAgent": config.max_runs_per_agent,
            "queuedRunsPerAgent": 8,
            "maxBackgroundProcesses": config.max_background_processes,
            "logBufferLines": 2000,
            "eventBuffer": 1024
        })
    );
    assert_eq!(LOG_BUFFER_LINES, 2_000);
}

#[tokio::test]
async fn build_revision_is_null_when_unset() {
    let (daemon, _) = daemon();
    let app = app_for(&Arc::new(RwLock::new(daemon)), &LogBuffer::new());
    let body = status_of(&app).await;
    match option_env!("ANIMAOS_BUILD_REVISION") {
        None => assert_eq!(body["buildRevision"], Value::Null),
        Some(revision) => assert_eq!(body["buildRevision"], revision),
    }
}

const NEW_METRICS: [(&str, &str); 14] = [
    ("anima_daemon_uptime_seconds", "gauge"),
    ("anima_daemon_runs_running", "gauge"),
    ("anima_daemon_runs_queued", "gauge"),
    ("anima_daemon_runs", "gauge"),
    ("anima_daemon_approvals_pending", "gauge"),
    ("anima_daemon_event_subscribers", "gauge"),
    ("anima_daemon_history_pending_flush", "gauge"),
    ("anima_daemon_history_usage_queued", "gauge"),
    ("anima_daemon_history_failing", "gauge"),
    ("anima_daemon_automations_failing", "gauge"),
    ("anima_daemon_log_lines_buffered", "gauge"),
    ("anima_daemon_event_lagged_total", "counter"),
    ("anima_daemon_history_flush_errors_total", "counter"),
    ("anima_daemon_messages_pruned_total", "counter"),
];

fn metric_value(text: &str, name: &str) -> u64 {
    text.lines()
        .find_map(|line| line.strip_prefix(&format!("{name} ")))
        .unwrap_or_else(|| panic!("{name} is missing:\n{text}"))
        .parse()
        .unwrap()
}

#[tokio::test]
async fn metrics_keep_the_existing_lines_and_add_the_new_ones() {
    let (daemon, _) = daemon();
    let app = app_for(&Arc::new(RwLock::new(daemon)), &LogBuffer::new());
    let text = metrics_of(&app).await;

    for existing in [
        "anima_daemon_ready 1",
        "anima_daemon_agents 1",
        "anima_daemon_background_process_manager_healthy 1",
        "anima_daemon_persistence_mode_info{mode=\"memory\"} 1",
        "anima_daemon_control_plane_durability_info{mode=\"ephemeral\"} 1",
    ] {
        assert!(
            text.lines().any(|line| line == existing),
            "{existing}\n{text}"
        );
    }
    for (name, kind) in NEW_METRICS {
        assert_eq!(
            text.lines()
                .filter(|line| line.starts_with(&format!("# HELP {name} ")))
                .count(),
            1,
            "{name} HELP"
        );
        assert!(
            text.lines()
                .any(|line| line == format!("# TYPE {name} {kind}")),
            "{name} TYPE"
        );
        if name != "anima_daemon_runs" {
            assert_eq!(
                text.lines()
                    .filter(|line| line.starts_with(&format!("{name} ")))
                    .count(),
                1,
                "{name} value"
            );
        }
    }
    let statuses = text
        .lines()
        .filter(|line| line.starts_with("anima_daemon_runs{status=\""))
        .collect::<Vec<_>>();
    assert_eq!(statuses.len(), 7, "{statuses:?}");
    assert!(statuses.contains(&"anima_daemon_runs{status=\"queued\"} 0"));
    assert!(statuses.contains(&"anima_daemon_runs{status=\"awaiting_approval\"} 0"));
}

#[tokio::test]
async fn metrics_counters_increase() {
    let store = Arc::new(FlakyHistoryStore::new());
    let history = HistoryService::new(store.clone());
    let (mut daemon, agent) = daemon();
    daemon.set_history(Arc::clone(&history));
    daemon.set_live_hub(LiveHub::new(2));
    let hub = daemon.live.clone();
    let state = Arc::new(RwLock::new(daemon));
    let app = app_for(&state, &LogBuffer::new());
    let before = metrics_of(&app).await;
    for (name, _) in NEW_METRICS.iter().filter(|(_, kind)| *kind == "counter") {
        assert_eq!(metric_value(&before, name), 0, "{name}");
    }

    // A failed flush.
    history.enqueue_committed(
        &agent,
        "chat:one",
        &[history_message("msg-1", &agent, "chat:one", MessageRole::User, "hi", 1).message],
    );
    store.set_failing(true);
    let transactions = tokio::sync::Mutex::new(());
    assert!(history
        .flush_once(&state, &transactions, 5_000)
        .await
        .is_err());
    // A prune.
    history.note_pruned(4);
    // A lagging subscriber: five events into a buffer of two.
    let mut stream = hub.subscribe(&agent).unwrap();
    for n in 0..5u64 {
        hub.publish(
            LiveEvent::new(
                &agent,
                LiveEventBody::StepDelta {
                    step_id: "run_1:1".into(),
                    offset: n,
                    text: "x".into(),
                },
            ),
            None,
        );
    }
    assert!(stream.next().await.is_some());

    let after = metrics_of(&app).await;
    assert_eq!(
        metric_value(&after, "anima_daemon_history_flush_errors_total"),
        1
    );
    assert_eq!(
        metric_value(&after, "anima_daemon_messages_pruned_total"),
        4
    );
    assert_eq!(metric_value(&after, "anima_daemon_event_lagged_total"), 3);
    assert_eq!(metric_value(&after, "anima_daemon_history_failing"), 1);
    assert_eq!(
        metric_value(&after, "anima_daemon_history_pending_flush"),
        1
    );
    assert_eq!(metric_value(&after, "anima_daemon_event_subscribers"), 1);
}

#[tokio::test]
async fn metrics_need_no_authorization_and_carry_counts_only() {
    let (mut daemon, agent) = daemon();
    daemon
        .runs
        .insert(RunRecord::running(run_start(&agent), 10));
    daemon.schedules.insert(
        "a".into(),
        schedule("a", &agent, true, AutomationCounters::default()),
    );
    let state = Arc::new(RwLock::new(daemon));
    let app = app_for(&state, &LogBuffer::new());

    for origin in [None, Some("https://untrusted.example"), Some(OWNER_ORIGIN)] {
        let response = app.clone().oneshot(get("/metrics", origin)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{origin:?}");
        let text = text_of(response).await;
        for private in [
            agent.as_str(),
            "companion",
            "a private prompt",
            "a private automation prompt",
            "gpt-5.4",
            "costMicros",
        ] {
            assert!(!text.contains(private), "{private} leaked:\n{text}");
        }
    }
    // The status aggregate, unlike the metrics, is for the owner only.
    let refused = app
        .oneshot(get("/api/status", Some("https://untrusted.example")))
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
}
