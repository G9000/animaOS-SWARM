use super::*;

use anima_core::primitives::now_millis;
use serde_json::{json, Value};

use crate::history::conformance::FlakyHistoryStore;
use crate::history::{HistoryService, UsagePageQuery};
use crate::routes::usage::{
    csv_cell, export_csv, CSV_HEADER, USAGE_CURSOR_INVALID, USAGE_EXPORT_TOO_LARGE,
    USAGE_GROUP_INVALID, USAGE_LIMIT_INVALID, USAGE_RANGE_INVALID, USAGE_TASK_FAILED,
    USAGE_TZ_INVALID,
};
use crate::runs::{RunRecord, RunSource, RunStart, RunStatus, RunStepUsage};
use crate::sessions::{SessionKind, SessionOrigin, SessionRecord, TitleSource};
use crate::usage::pricing::{
    MAX_PRICE_MICROS_PER_MTOK, MAX_PRICING_MODEL_CHARS, MAX_PRICING_OVERRIDES,
    MAX_PRICING_PROVIDER_CHARS, PRICING_DUPLICATE, PRICING_ENTRY_INVALID, PRICING_TOO_MANY,
};
use crate::usage::{
    PricingSource, UsageRecord, UsageSource, DEFAULT_RECORDS_LIMIT, DEFAULT_USAGE_RANGE_DAYS,
    HISTORY_USAGE_BATCH, MAX_CSV_ROWS, MAX_RECORDS_LIMIT, MAX_SESSION_GROUPS, MAX_SUMMARY_ROWS,
    MAX_USAGE_RANGE_DAYS, USAGE_QUEUE_MAX, USAGE_SCAN_PAGE,
};

const OWNER_ORIGIN: &str = "http://localhost:4200";
const HOUR: u64 = 3_600_000;
const DAY: u64 = 24 * HOUR;

fn request(method: &str, uri: &str, origin: &str, body: Option<Value>) -> Request<Body> {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "127.0.0.1:8080")
        .header("origin", origin);
    match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

async fn text_body(response: axum::response::Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

async fn send(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> axum::response::Response {
    app.clone()
        .oneshot(request(method, uri, OWNER_ORIGIN, body))
        .await
        .unwrap()
}

fn row(id: &str, agent: &str, session: Option<&str>, at_ms: u64) -> UsageRecord {
    UsageRecord {
        id: id.into(),
        agent_id: agent.into(),
        session_id: session.map(str::to_string),
        run_id: None,
        source: UsageSource::Chat,
        provider: "anthropic".into(),
        model: "claude-fable-5-1".into(),
        prompt_tokens: 10,
        completion_tokens: 5,
        cached_prompt_tokens: 0,
        reasoning_tokens: 0,
        total_tokens: 15,
        cost_micros: Some(100),
        pricing_source: PricingSource::Table,
        duration_ms: 7,
        created_at_ms: at_ms,
    }
}

async fn seed(state: &Arc<RwLock<DaemonState>>, rows: Vec<UsageRecord>) {
    let store = state.read().await.history.store();
    store.upsert_usage(&rows).await.unwrap();
}

fn empty_daemon() -> Arc<RwLock<DaemonState>> {
    Arc::new(RwLock::new(DaemonState::new()))
}

fn override_body(provider: &str, model: &str) -> Value {
    json!({
        "provider": provider, "model": model,
        "inputMicrosPerMtok": 1_000_000, "outputMicrosPerMtok": 2_000_000
    })
}

async fn error_of(response: axum::response::Response, status: StatusCode) -> String {
    assert_eq!(response.status(), status);
    assert_eq!(response.headers()["cache-control"], "no-store");
    json_body(response).await["error"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn the_usage_routes_refuse_a_non_owner() {
    let state = empty_daemon();
    let app = router(state.clone(), DaemonConfig::default());
    for (method, uri, body) in [
        ("GET", "/api/usage/summary", None),
        ("GET", "/api/usage/records", None),
        ("GET", "/api/usage/export.csv", None),
        ("GET", "/api/usage/pricing", None),
        (
            "PUT",
            "/api/usage/pricing",
            Some(json!({"overrides": [override_body("anthropic", "claude")]})),
        ),
    ] {
        let refused = app
            .clone()
            .oneshot(request(method, uri, "https://untrusted.example", body))
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::FORBIDDEN, "{method} {uri}");
        assert_eq!(refused.headers()["cache-control"], "no-store");
    }
    assert!(state.read().await.pricing_overrides.is_empty());
}

#[tokio::test]
async fn summary_totals_and_groups() {
    let state = empty_daemon();
    let mut first = row("u1", "agent-a", Some("s1"), DAY + HOUR);
    first.prompt_tokens = 100;
    first.completion_tokens = 50;
    first.total_tokens = 150;
    first.cost_micros = Some(1_000);
    let mut second = row("u2", "agent-a", Some("s1"), DAY + 23 * HOUR);
    second.provider = "openai".into();
    second.model = "gpt-x".into();
    second.cost_micros = None;
    second.pricing_source = PricingSource::Unknown;
    let mut third = row("u3", "agent-b", Some("s2"), 2 * DAY + HOUR);
    third.source = UsageSource::Telegram;
    third.prompt_tokens = 200;
    third.completion_tokens = 100;
    third.total_tokens = 300;
    third.cost_micros = Some(3_000);
    let mut fourth = row("u4", "agent-c", None, 3 * DAY);
    fourth.source = UsageSource::Title;
    fourth.provider = "chatgpt".into();
    fourth.model = "gpt-5".into();
    fourth.total_tokens = 2;
    fourth.cost_micros = None;
    fourth.pricing_source = PricingSource::Subscription;
    seed(&state, vec![first, second, third, fourth]).await;
    let app = router(state, DaemonConfig::default());
    let base = format!("/api/usage/summary?from=0&to={}", 30 * DAY);

    let plain = send(&app, "GET", &base, None).await;
    assert_eq!(plain.headers()["cache-control"], "no-store");
    let plain = json_body(plain).await;
    assert_eq!(plain["groups"], json!([]));
    assert_eq!(plain["groupBy"], Value::Null);
    assert_eq!(plain["truncated"], false);
    assert_eq!(plain["tzOffsetMinutes"], 0);
    let totals = &plain["totals"];
    assert_eq!(totals["calls"], 4);
    assert_eq!(totals["totalTokens"], 150 + 15 + 300 + 2);
    assert_eq!(totals["costMicros"], 4_000);
    assert_eq!(totals["unpricedCalls"], 1);
    assert_eq!(totals["subscriptionCalls"], 1);

    let keys = |body: &Value| -> Vec<(String, u64)> {
        body["groups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|group| {
                (
                    group["key"].as_str().unwrap().to_string(),
                    group["totals"]["calls"].as_u64().unwrap(),
                )
            })
            .collect()
    };
    let by_model = json_body(send(&app, "GET", &format!("{base}&groupBy=model"), None).await).await;
    assert_eq!(by_model["groupBy"], "model");
    assert_eq!(
        keys(&by_model),
        [
            ("anthropic/claude-fable-5-1".to_string(), 2),
            ("openai/gpt-x".to_string(), 1),
            ("chatgpt/gpt-5".to_string(), 1),
        ]
    );
    let by_source =
        json_body(send(&app, "GET", &format!("{base}&groupBy=source"), None).await).await;
    assert_eq!(
        keys(&by_source),
        [
            ("telegram".to_string(), 1),
            ("chat".to_string(), 2),
            ("title".to_string(), 1),
        ]
    );
    let by_day = json_body(send(&app, "GET", &format!("{base}&groupBy=day"), None).await).await;
    assert_eq!(
        keys(&by_day),
        [
            ("1970-01-02".to_string(), 2),
            ("1970-01-03".to_string(), 1),
            ("1970-01-04".to_string(), 1),
        ]
    );
    let shifted = json_body(
        send(
            &app,
            "GET",
            &format!("{base}&groupBy=day&tzOffsetMinutes=480"),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(shifted["tzOffsetMinutes"], 480);
    assert_eq!(
        keys(&shifted),
        [
            ("1970-01-02".to_string(), 1),
            ("1970-01-03".to_string(), 2),
            ("1970-01-04".to_string(), 1),
        ],
        "the 23:00 UTC call is the next day at +08:00"
    );

    let agent = json_body(send(&app, "GET", &format!("{base}&agentId=agent-a"), None).await).await;
    assert_eq!(agent["totals"]["calls"], 2);
    let session = json_body(send(&app, "GET", &format!("{base}&sessionId=s2"), None).await).await;
    assert_eq!(session["totals"]["calls"], 1);
    assert_eq!(session["totals"]["totalTokens"], 300);
}

#[tokio::test]
async fn summary_validates_its_query() {
    let app = router(empty_daemon(), DaemonConfig::default());
    for (query, message) in [
        ("from=100&to=50", USAGE_RANGE_INVALID),
        ("from=5&to=5", USAGE_RANGE_INVALID),
        ("from=0&to=31622400001", USAGE_RANGE_INVALID),
        ("from=abc", USAGE_RANGE_INVALID),
        ("to=-1", USAGE_RANGE_INVALID),
        ("groupBy=year", USAGE_GROUP_INVALID),
        ("tzOffsetMinutes=841", USAGE_TZ_INVALID),
        ("tzOffsetMinutes=-841", USAGE_TZ_INVALID),
        ("tzOffsetMinutes=abc", USAGE_TZ_INVALID),
    ] {
        let response = send(&app, "GET", &format!("/api/usage/summary?{query}"), None).await;
        assert_eq!(
            error_of(response, StatusCode::BAD_REQUEST).await,
            message,
            "{query}"
        );
    }
    for query in [
        "from=0&to=31622400000&groupBy=session&tzOffsetMinutes=840",
        "tzOffsetMinutes=-840",
    ] {
        let response = send(&app, "GET", &format!("/api/usage/summary?{query}"), None).await;
        assert_eq!(response.status(), StatusCode::OK, "{query}");
    }
}

#[tokio::test]
async fn summary_defaults_to_the_last_thirty_days() {
    let state = empty_daemon();
    let app = router(Arc::clone(&state), DaemonConfig::default());
    let window = json_body(send(&app, "GET", "/api/usage/summary", None).await).await;
    let (from, to) = (
        window["from"].as_u64().unwrap(),
        window["to"].as_u64().unwrap(),
    );
    assert_eq!(to - from, 30 * DAY);
    seed(
        &state,
        vec![
            row("inside", "agent-a", None, from + DAY),
            row("outside", "agent-a", None, from - DAY),
        ],
    )
    .await;

    let summary = json_body(send(&app, "GET", "/api/usage/summary", None).await).await;
    assert_eq!(
        summary["totals"]["calls"], 1,
        "31-day-old rows are excluded"
    );
}

#[tokio::test]
async fn records_page_newest_first_with_a_cursor() {
    let state = empty_daemon();
    seed(
        &state,
        (1..=5)
            .map(|n| row(&format!("u{n}"), "agent-a", None, n * 1_000))
            .collect(),
    )
    .await;
    let app = router(state, DaemonConfig::default());
    let base = "/api/usage/records?from=0&to=100000&limit=2";

    let first = send(&app, "GET", base, None).await;
    assert_eq!(first.headers()["cache-control"], "no-store");
    let first = json_body(first).await;
    let ids = |body: &Value| -> Vec<String> {
        body["records"]
            .as_array()
            .unwrap()
            .iter()
            .map(|record| record["id"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(ids(&first), ["u5", "u4"]);
    assert_eq!(first["nextCursor"], "4000:u4");
    assert_eq!(first["records"][0]["costMicros"], 100);
    assert_eq!(first["records"][0]["pricingSource"], "table");
    assert_eq!(first["records"][0]["source"], "chat");

    let second = json_body(send(&app, "GET", &format!("{base}&cursor=4000:u4"), None).await).await;
    assert_eq!(ids(&second), ["u3", "u2"]);
    assert_eq!(second["nextCursor"], "2000:u2");
    let third = json_body(send(&app, "GET", &format!("{base}&cursor=2000:u2"), None).await).await;
    assert_eq!(ids(&third), ["u1"]);
    assert_eq!(third["nextCursor"], Value::Null);

    let filtered = json_body(
        send(
            &app,
            "GET",
            "/api/usage/records?from=0&to=100000&agentId=nobody",
            None,
        )
        .await,
    )
    .await;
    assert_eq!(filtered["records"], json!([]));
}

#[tokio::test]
async fn records_validate_limit_and_cursor() {
    let app = router(empty_daemon(), DaemonConfig::default());
    for (query, message) in [
        ("limit=0", USAGE_LIMIT_INVALID),
        ("limit=201", USAGE_LIMIT_INVALID),
        ("limit=abc", USAGE_LIMIT_INVALID),
        ("cursor=nope", USAGE_CURSOR_INVALID),
        ("cursor=x:id", USAGE_CURSOR_INVALID),
        ("cursor=5:", USAGE_CURSOR_INVALID),
        ("from=9&to=1", USAGE_RANGE_INVALID),
    ] {
        let response = send(&app, "GET", &format!("/api/usage/records?{query}"), None).await;
        assert_eq!(
            error_of(response, StatusCode::BAD_REQUEST).await,
            message,
            "{query}"
        );
    }
    let response = send(&app, "GET", "/api/usage/records?limit=200", None).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn csv_has_the_header_rows_and_safe_cells() {
    let state = empty_daemon();
    let mut newer = row("u2", "agent-a", None, 2_000);
    newer.model = "m,\"x\"".into();
    newer.provider = "chatgpt".into();
    newer.cost_micros = None;
    newer.pricing_source = PricingSource::Subscription;
    newer.prompt_tokens = 1;
    newer.completion_tokens = 1;
    newer.total_tokens = 2;
    newer.duration_ms = 0;
    let mut older = row("u1", "agent-a", Some("s1"), 1_000);
    older.model = "=cmd|calc".into();
    older.cost_micros = Some(1_500_000);
    seed(&state, vec![newer, older]).await;
    let app = router(state, DaemonConfig::default());

    let response = send(
        &app,
        "GET",
        &format!("/api/usage/export.csv?from=0&to={}", 2 * DAY),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert_eq!(
        response.headers()["content-type"],
        "text/csv; charset=utf-8"
    );
    assert_eq!(
        response.headers()["content-disposition"],
        "attachment; filename=\"anima-usage-1970-01-01-1970-01-02.csv\""
    );
    let csv = text_body(response).await;
    let lines = csv.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0], CSV_HEADER);
    assert_eq!(
        lines[1],
        "u1,1970-01-01T00:00:01.000Z,agent-a,s1,,chat,anthropic,'=cmd|calc,10,5,0,0,15,1.500000,table,7",
        "oldest first, with the formula neutralized"
    );
    assert_eq!(
        lines[2],
        "u2,1970-01-01T00:00:02.000Z,agent-a,,,chat,chatgpt,\"m,\"\"x\"\"\",1,1,0,0,2,,subscription,0",
        "a comma and quotes are quoted, a null cost is blank"
    );
}

#[test]
fn csv_cells_neutralize_formulas_and_quote_specials() {
    assert_eq!(csv_cell("plain"), "plain");
    assert_eq!(csv_cell(""), "");
    for formula in ["=1+1", "+1", "-1", "@SUM(A1)", "\tcell"] {
        assert_eq!(csv_cell(formula), format!("'{formula}"));
    }
    assert_eq!(csv_cell("\rcell"), "\"'\rcell\"", "a CR start is both");
    assert_eq!(csv_cell("a,b"), "\"a,b\"");
    assert_eq!(csv_cell("say \"hi\""), "\"say \"\"hi\"\"\"");
    assert_eq!(csv_cell("two\nlines"), "\"two\nlines\"");
    assert_eq!(csv_cell("=a,\"b\""), "\"'=a,\"\"b\"\"\"");
    assert_eq!(csv_cell("mid=dle"), "mid=dle");
}

#[tokio::test]
async fn csv_for_an_empty_range_is_only_the_header() {
    let app = router(empty_daemon(), DaemonConfig::default());
    let response = send(
        &app,
        "GET",
        &format!("/api/usage/export.csv?from=0&to={DAY}"),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(text_body(response).await, format!("{CSV_HEADER}\n"));
}

#[tokio::test]
async fn csv_refuses_more_than_the_row_cap() {
    let state = empty_daemon();
    seed(
        &state,
        (1..=3)
            .map(|n| row(&format!("u{n}"), "agent-a", None, n * 1_000))
            .collect(),
    )
    .await;
    let store = state.read().await.history.store();
    assert!(export_csv(&*store, 0, DAY, None, 2)
        .await
        .unwrap()
        .is_none());
    let csv = export_csv(&*store, 0, DAY, None, 3).await.unwrap().unwrap();
    assert_eq!(csv.lines().count(), 4);
    let app = router(state, DaemonConfig::default());
    let bad = send(&app, "GET", "/api/usage/export.csv?from=9&to=1", None).await;
    assert_eq!(
        error_of(bad, StatusCode::BAD_REQUEST).await,
        USAGE_RANGE_INVALID
    );
}

#[tokio::test]
async fn get_pricing_starts_empty_and_reports_the_table_date() {
    let app = router(empty_daemon(), DaemonConfig::default());
    let response = send(&app, "GET", "/api/usage/pricing", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let body = json_body(response).await;
    assert_eq!(body["overrides"], json!([]));
    assert_eq!(body["tableDate"], anima_model_adapters::PRICING_TABLE_DATE);
}

/// A finished run of `agent_id` with one 10 + 2 token step.
fn finished_run(agent_id: &str, run_id: &str) -> RunRecord {
    let mut run = RunRecord::running(
        RunStart {
            agent_id: agent_id.into(),
            session_id: "chat:one".into(),
            source: RunSource::Web,
            source_ref: None,
            idempotency_key: None,
            text: "hi".into(),
            model: "claude-fable-5-1".into(),
            provider: Some("anthropic".into()),
            parent_run_id: None,
        },
        1_000,
    );
    run.id = run_id.into();
    let usage = TokenUsage {
        prompt_tokens: 10,
        completion_tokens: 2,
        total_tokens: 12,
        ..TokenUsage::default()
    };
    run.steps = vec![RunStepUsage {
        step_id: format!("{run_id}:1"),
        usage: usage.clone(),
        at_ms: 1_001,
        duration_ms: 5,
    }];
    run.usage = usage;
    run.finish(RunStatus::Completed, None, 2_000);
    run
}

async fn usage_rows(state: &Arc<RwLock<DaemonState>>) -> Vec<UsageRecord> {
    let store = state.read().await.history.store();
    store
        .page_usage(&UsagePageQuery {
            from_ms: 0,
            to_ms: u64::MAX,
            agent_id: None,
            session_id: None,
            before: None,
            limit: 100,
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn put_pricing_replaces_the_list_and_prices_later_calls() {
    let mut daemon = DaemonState::new();
    let agent = daemon
        .create_agent(test_config("historian"))
        .unwrap()
        .state
        .id;
    let state = Arc::new(RwLock::new(daemon));
    let captured = Arc::new(StdMutex::new(None));
    let app = super::super::router_with_runs(Arc::clone(&state), DaemonConfig::default(), {
        let captured = Arc::clone(&captured);
        move |runs| {
            *captured.lock().unwrap() = Some(runs.clone());
            runs
        }
    });
    let coordinator = captured.lock().unwrap().take().unwrap();
    let transactions = coordinator.control_plane_transactions();
    let history = state.read().await.history.clone();

    state
        .write()
        .await
        .runs
        .insert(finished_run(&agent, "run_before"));
    history
        .flush_once(&state, &transactions, now_millis())
        .await
        .unwrap();

    let put = send(
        &app,
        "PUT",
        "/api/usage/pricing",
        Some(json!({"overrides": [override_body("  Anthropic ", " CLAUDE-FABLE ")]})),
    )
    .await;
    assert_eq!(put.status(), StatusCode::OK);
    assert_eq!(put.headers()["cache-control"], "no-store");
    let put = json_body(put).await;
    assert_eq!(put["overrides"][0]["provider"], "anthropic");
    assert_eq!(put["overrides"][0]["model"], "claude-fable");
    assert_eq!(put["overrides"][0]["cachedInputMicrosPerMtok"], Value::Null);
    assert_eq!(put["tableDate"], anima_model_adapters::PRICING_TABLE_DATE);
    let read = json_body(send(&app, "GET", "/api/usage/pricing", None).await).await;
    assert_eq!(read["overrides"], put["overrides"]);

    state
        .write()
        .await
        .runs
        .insert(finished_run(&agent, "run_after"));
    history
        .flush_once(&state, &transactions, now_millis())
        .await
        .unwrap();
    let rows = usage_rows(&state).await;
    let find = |id: &str| rows.iter().find(|row| row.id == id).unwrap();
    assert_eq!(find("run_after:1").pricing_source, PricingSource::Override);
    assert_eq!(find("run_after:1").cost_micros, Some(14));
    assert_eq!(
        find("run_before:1").pricing_source,
        PricingSource::Table,
        "an earlier row keeps its price"
    );

    let cleared = send(
        &app,
        "PUT",
        "/api/usage/pricing",
        Some(json!({"overrides": []})),
    )
    .await;
    assert_eq!(json_body(cleared).await["overrides"], json!([]));
    assert_eq!(
        usage_rows(&state)
            .await
            .iter()
            .find(|row| row.id == "run_after:1")
            .unwrap()
            .pricing_source,
        PricingSource::Override,
        "clearing the list does not reprice stored rows"
    );
}

#[tokio::test]
async fn put_pricing_validates() {
    let state = empty_daemon();
    let app = router(Arc::clone(&state), DaemonConfig::default());
    let put = |body: Value| send(&app, "PUT", "/api/usage/pricing", Some(body));

    let many = (0..=MAX_PRICING_OVERRIDES)
        .map(|n| override_body("anthropic", &format!("m{n}")))
        .collect::<Vec<_>>();
    assert_eq!(
        error_of(
            put(json!({"overrides": many})).await,
            StatusCode::BAD_REQUEST
        )
        .await,
        PRICING_TOO_MANY
    );
    let mut too_pricey = override_body("anthropic", "claude");
    too_pricey["inputMicrosPerMtok"] = json!(MAX_PRICE_MICROS_PER_MTOK + 1);
    for invalid in [
        override_body("", "claude"),
        override_body("anthropic", "  "),
        override_body(&"p".repeat(MAX_PRICING_PROVIDER_CHARS + 1), "claude"),
        override_body("anthropic", &"m".repeat(MAX_PRICING_MODEL_CHARS + 1)),
        too_pricey,
    ] {
        assert_eq!(
            error_of(
                put(json!({"overrides": [invalid]})).await,
                StatusCode::BAD_REQUEST
            )
            .await,
            PRICING_ENTRY_INVALID
        );
    }
    assert_eq!(
        error_of(
            put(json!({"overrides": [
                override_body("google", "gemini-x"),
                override_body("Gemini", "GEMINI-X"),
            ]}))
            .await,
            StatusCode::BAD_REQUEST
        )
        .await,
        PRICING_DUPLICATE,
        "two names of one provider are a duplicate"
    );

    let mut unknown_entry_field = override_body("anthropic", "claude");
    unknown_entry_field["extra"] = json!(1);
    for malformed in [
        json!({"overrides": [], "extra": 1}),
        json!({"overrides": [unknown_entry_field]}),
        json!({"overrides": "none"}),
        json!({}),
    ] {
        assert_eq!(
            put(malformed).await.status(),
            StatusCode::BAD_REQUEST,
            "malformed bodies are refused"
        );
    }
    assert!(state.read().await.pricing_overrides.is_empty());
}

/// A path a JSON control-plane save cannot write to: a directory where the
/// store expects a file.
fn invalid_snapshot_directory() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "anima-usage-route-invalid-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[tokio::test]
async fn a_failed_save_restores_the_pricing_and_answers_503() {
    use crate::control_plane_store::ControlPlaneStoreConfig;

    let state = empty_daemon();
    let app = router(Arc::clone(&state), DaemonConfig::default());
    let kept = json!({"overrides": [override_body("anthropic", "claude")]});
    let saved = send(&app, "PUT", "/api/usage/pricing", Some(kept)).await;
    assert_eq!(saved.status(), StatusCode::OK);
    let before = json_body(send(&app, "GET", "/api/usage/pricing", None).await).await;

    let invalid_path = invalid_snapshot_directory();
    state
        .write()
        .await
        .set_control_plane_store(Some(ControlPlaneStoreConfig::Json(invalid_path.clone())));
    let refused = send(
        &app,
        "PUT",
        "/api/usage/pricing",
        Some(json!({"overrides": [override_body("openai", "gpt")]})),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(refused.headers()["cache-control"], "no-store");

    let after = json_body(send(&app, "GET", "/api/usage/pricing", None).await).await;
    assert_eq!(after, before, "the old list stays");
    let _ = std::fs::remove_dir_all(invalid_path);
}

#[tokio::test]
async fn a_dropped_pricing_put_still_finishes() {
    let state = empty_daemon();
    let captured = Arc::new(StdMutex::new(None));
    let app = super::super::router_with_runs(Arc::clone(&state), DaemonConfig::default(), {
        let captured = Arc::clone(&captured);
        move |runs| {
            *captured.lock().unwrap() = Some(runs.clone());
            runs
        }
    });
    let coordinator = captured.lock().unwrap().take().unwrap();

    let held = coordinator.control_plane_transaction().await;
    {
        let mut put = Box::pin(app.clone().oneshot(request(
            "PUT",
            "/api/usage/pricing",
            OWNER_ORIGIN,
            Some(json!({"overrides": [override_body("anthropic", "claude")]})),
        )));
        assert!(
            futures::poll!(put.as_mut()).is_pending(),
            "the PUT must wait for the control-plane transaction"
        );
        // The request is dropped here, before its change could run.
    }
    // Let the spawned change run up to the transaction by yielding, not
    // sleeping.
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    assert!(state.read().await.pricing_overrides.is_empty());
    drop(held);

    // tokio's mutex is FIFO: this waits behind the spawned change.
    let _after = coordinator.control_plane_transaction().await;
    let list = state.read().await.pricing_overrides.clone();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].model, "claude");
}

/// A daemon whose agent has the chat `chat:plans`.
fn session_daemon() -> (Arc<RwLock<DaemonState>>, String) {
    let mut daemon = DaemonState::new();
    let agent = daemon
        .create_agent(test_config("companion"))
        .unwrap()
        .state
        .id;
    daemon.sessions.insert(SessionRecord::new(
        &agent,
        "chat:plans",
        SessionKind::Chat,
        SessionOrigin::Web,
        "Plans".into(),
        TitleSource::Owner,
        1,
    ));
    (Arc::new(RwLock::new(daemon)), agent)
}

#[tokio::test]
async fn session_get_includes_usage_totals_and_the_list_does_not() {
    let (state, agent) = session_daemon();
    seed(
        &state,
        vec![
            row("u1", &agent, Some("chat:plans"), 1_000),
            row("u2", &agent, Some("chat:plans"), 2_000),
            row("u3", &agent, Some("chat:other"), 3_000),
        ],
    )
    .await;
    let app = router(state, DaemonConfig::default());

    let detail = send(
        &app,
        "GET",
        &format!("/api/agents/{agent}/sessions/chat%3Aplans"),
        None,
    )
    .await;
    assert_eq!(detail.status(), StatusCode::OK);
    let detail = json_body(detail).await;
    let usage = &detail["session"]["usage"];
    assert_eq!(usage["calls"], 2);
    assert_eq!(usage["totalTokens"], 30);
    assert_eq!(usage["costMicros"], 200);

    for uri in [
        format!("/api/agents/{agent}/sessions"),
        format!("/api/agents/{agent}/sessions?view=summary"),
    ] {
        let listed = json_body(send(&app, "GET", &uri, None).await).await;
        let sessions = listed["sessions"].as_array().unwrap();
        assert!(!sessions.is_empty(), "{uri}");
        assert!(sessions.iter().all(|s| s.get("usage").is_none()), "{uri}");
    }
}

#[tokio::test]
async fn session_usage_is_null_when_the_store_fails() {
    let (state, agent) = session_daemon();
    let store = Arc::new(FlakyHistoryStore::new());
    state
        .write()
        .await
        .set_history(HistoryService::new(store.clone()));
    store.set_failing(true);
    let app = router(state, DaemonConfig::default());

    let detail = send(
        &app,
        "GET",
        &format!("/api/agents/{agent}/sessions/chat%3Aplans"),
        None,
    )
    .await;
    assert_eq!(detail.status(), StatusCode::OK, "the GET does not fail");
    let detail = json_body(detail).await;
    assert_eq!(detail["session"]["usage"], Value::Null);
    assert!(detail["session"].as_object().unwrap().contains_key("usage"));
}

#[test]
fn the_usage_routes_are_in_the_openapi_document() {
    use utoipa::OpenApi;

    let document = crate::routes::ApiDoc::openapi();
    let json = serde_json::to_value(&document).unwrap();
    for (path, methods) in [
        ("/api/usage/summary", vec!["get"]),
        ("/api/usage/records", vec!["get"]),
        ("/api/usage/export.csv", vec!["get"]),
        ("/api/usage/pricing", vec!["get", "put"]),
    ] {
        for method in methods {
            assert_eq!(
                json["paths"][path][method]["tags"],
                json!(["usage"]),
                "{method} {path}"
            );
        }
    }
    assert!(json["tags"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tag| tag["name"] == "usage"));
    assert!(
        json["components"]["schemas"]["SessionResponse"]["properties"]
            .get("usage")
            .is_some()
    );
}

#[test]
fn constants() {
    assert_eq!(
        USAGE_RANGE_INVALID,
        "from and to must be epoch milliseconds, from before to, at most 366 days apart"
    );
    assert_eq!(
        USAGE_GROUP_INVALID,
        "groupBy must be one of day, model, source, session"
    );
    assert_eq!(USAGE_LIMIT_INVALID, "limit must be from 1 to 200");
    assert_eq!(USAGE_CURSOR_INVALID, "cursor is not valid");
    assert_eq!(USAGE_TZ_INVALID, "tzOffsetMinutes must be from -840 to 840");
    assert_eq!(
        USAGE_EXPORT_TOO_LARGE,
        "That range has too many calls to export; choose a shorter range"
    );
    assert_eq!(
        USAGE_TASK_FAILED,
        "The usage change did not finish; check Usage and try again"
    );
    assert_eq!(
        CSV_HEADER,
        "id,createdAt,agentId,sessionId,runId,source,provider,model,promptTokens,completionTokens,cachedPromptTokens,reasoningTokens,totalTokens,costUsd,pricingSource,durationMs"
    );
    assert_eq!(DEFAULT_USAGE_RANGE_DAYS, 30);
    assert_eq!(MAX_USAGE_RANGE_DAYS, 366);
    assert_eq!(DEFAULT_RECORDS_LIMIT, 50);
    assert_eq!(MAX_RECORDS_LIMIT, 200);
    assert_eq!(USAGE_SCAN_PAGE, 2_000);
    assert_eq!(MAX_SUMMARY_ROWS, 200_000);
    assert_eq!(MAX_SESSION_GROUPS, 20);
    assert_eq!(MAX_CSV_ROWS, 100_000);
    assert_eq!(USAGE_QUEUE_MAX, 10_000);
    assert_eq!(HISTORY_USAGE_BATCH, 500);
    assert_eq!(MAX_PRICING_OVERRIDES, 100);
    assert_eq!(MAX_PRICE_MICROS_PER_MTOK, 1_000_000_000_000);
    assert_eq!(MAX_PRICING_PROVIDER_CHARS, 64);
    assert_eq!(MAX_PRICING_MODEL_CHARS, 128);
}
