use super::*;

use serde_json::{json, Value};

use crate::agent_runs::test_support::{companion_config, ScriptedModel, Step};
use crate::history::conformance::{fire_record, FlakyHistoryStore};
use crate::history::HistoryService;
use crate::schedules::{
    AutomationInput, ScheduleLastFired, ScheduleTarget, ScheduleTrigger, HEARTBEAT_NEEDS_TIME_ZONE,
    MAX_AUTOMATIONS_PER_AGENT, PROMPT_AND_TRIGGER_REQUIRED, TOO_MANY_AUTOMATIONS,
};

const OWNER_ORIGIN: &str = "http://localhost:4200";

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

fn daemon_with(daemon: DaemonState) -> (Arc<RwLock<DaemonState>>, String) {
    let mut daemon = daemon;
    let agent = daemon
        .create_agent(companion_config("companion"))
        .unwrap()
        .state
        .id;
    (Arc::new(RwLock::new(daemon)), agent)
}

fn hourly() -> Value {
    json!({
        "prompt": "Check status",
        "trigger": {"type": "interval", "intervalMs": 3_600_000},
        "target": {"type": "workspace"}
    })
}

#[tokio::test]
async fn the_automation_routes_require_the_owner() {
    let (state, agent) = daemon_with(DaemonState::new());
    let app = router(state, DaemonConfig::default());
    for (method, uri, body) in [
        ("GET", format!("/api/agents/{agent}/schedules"), None),
        (
            "GET",
            format!("/api/agents/{agent}/schedules/s1/history"),
            None,
        ),
        (
            "POST",
            format!("/api/agents/{agent}/schedules/s1/run"),
            None,
        ),
        (
            "POST",
            "/api/schedules/preview".to_string(),
            Some(json!({"trigger": {"type": "interval", "intervalMs": 60_000}})),
        ),
    ] {
        let refused = app
            .clone()
            .oneshot(request(method, &uri, "https://untrusted.example", body))
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::FORBIDDEN, "{method} {uri}");
        assert_eq!(refused.headers()["cache-control"], "no-store");
    }
}

#[tokio::test]
async fn a_cron_automation_with_a_name_and_active_hours_lists_its_new_fields() {
    let (state, agent) = daemon_with(DaemonState::new());
    let app = router(state, DaemonConfig::default());
    let base = format!("/api/agents/{agent}/schedules");
    let created = send(
        &app,
        "POST",
        &base,
        Some(json!({
            "name": "Weekday brief",
            "prompt": "Summarize my day",
            "trigger": {"type": "cron", "expression": "0 9 * * 1-5", "timeZone": "Europe/London"},
            "activeHours": {"start": "08:00", "end": "18:00", "days": [5, 1, 2, 3, 4], "timeZone": "Europe/London"}
        })),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let schedule = json_body(created).await["schedule"].clone();
    assert_eq!(schedule["name"], "Weekday brief");
    assert_eq!(schedule["trigger"]["type"], "cron");
    assert_eq!(
        schedule["target"]["type"], "workspace",
        "the default target"
    );
    assert_eq!(schedule["activeHours"]["days"], json!([1, 2, 3, 4, 5]));
    assert_eq!(schedule["createdBy"], json!({"kind": "owner"}));
    assert_eq!(schedule["preset"], Value::Null);
    assert_eq!(
        schedule["counters"],
        json!({"runs": 0, "failures": 0, "consecutiveFailures": 0})
    );
    assert_eq!(schedule["running"], false);

    let listed = send(&app, "GET", &base, None).await;
    assert_eq!(listed.status(), StatusCode::OK);
    assert_eq!(listed.headers()["cache-control"], "no-store");
    assert_eq!(
        json_body(listed).await["schedules"][0]["id"],
        schedule["id"]
    );

    let bad = send(
        &app,
        "POST",
        &base,
        Some(json!({
            "prompt": "x",
            "trigger": {"type": "cron", "expression": "61 * * * *", "timeZone": "UTC"}
        })),
    )
    .await;
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        json_body(bad).await["error"],
        "minute: must be from 0 to 59"
    );
}

#[tokio::test]
async fn the_heartbeat_preset_takes_the_owners_overrides() {
    let (state, agent) = daemon_with(DaemonState::new());
    let app = router(state, DaemonConfig::default());
    let base = format!("/api/agents/{agent}/schedules");
    let created = send(
        &app,
        "POST",
        &base,
        Some(json!({"preset": "heartbeat", "timeZone": "UTC", "prompt": "Anything urgent?"})),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let schedule = json_body(created).await["schedule"].clone();
    assert_eq!(schedule["preset"], "heartbeat");
    assert_eq!(schedule["name"], "Heartbeat");
    assert_eq!(schedule["prompt"], "Anything urgent?");
    assert_eq!(schedule["trigger"]["intervalMs"], 1_800_000);
    assert_eq!(schedule["activeHours"]["start"], "08:00");
    assert_eq!(schedule["activeHours"]["end"], "22:00");

    for (body, problem) in [
        (json!({"preset": "heartbeat"}), HEARTBEAT_NEEDS_TIME_ZONE),
        (json!({"prompt": "Check"}), PROMPT_AND_TRIGGER_REQUIRED),
    ] {
        let refused = send(&app, "POST", &base, Some(body)).await;
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
        assert_eq!(json_body(refused).await["error"], problem);
    }
}

#[tokio::test]
async fn patch_renames_and_sets_then_clears_active_hours() {
    let (state, agent) = daemon_with(DaemonState::new());
    let app = router(state, DaemonConfig::default());
    let base = format!("/api/agents/{agent}/schedules");
    let id = json_body(send(&app, "POST", &base, Some(hourly())).await).await["schedule"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let item = format!("{base}/{id}");

    let set = json_body(
        send(
            &app,
            "PATCH",
            &item,
            Some(json!({
                "name": "Status",
                "activeHours": {"start": "09:00", "end": "17:00", "days": [1], "timeZone": "UTC"}
            })),
        )
        .await,
    )
    .await;
    assert_eq!(set["schedule"]["name"], "Status");
    assert_eq!(set["schedule"]["activeHours"]["start"], "09:00");

    let cleared =
        json_body(send(&app, "PATCH", &item, Some(json!({"activeHours": null}))).await).await;
    assert_eq!(cleared["schedule"]["activeHours"], Value::Null);
    assert_eq!(cleared["schedule"]["name"], "Status");
}

#[tokio::test]
async fn preview_lists_the_next_fires_or_the_triggers_problem() {
    let (state, _) = daemon_with(DaemonState::new());
    let app = router(state, DaemonConfig::default());
    let preview = |body: Value| {
        let app = app.clone();
        async move { send(&app, "POST", "/api/schedules/preview", Some(body)).await }
    };

    let hourly = preview(json!({"trigger": {"type": "interval", "intervalMs": 3_600_000}})).await;
    assert_eq!(hourly.status(), StatusCode::OK);
    assert_eq!(hourly.headers()["cache-control"], "no-store");
    let runs = json_body(hourly).await["nextRuns"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(runs.len(), 3);
    let first = runs[0].as_u64().unwrap();
    assert_eq!(runs[1].as_u64().unwrap(), first + 3_600_000);

    let once = preview(json!({"trigger": {"type": "once", "atMs": 4_102_444_800_000u64}})).await;
    assert_eq!(
        json_body(once).await["nextRuns"],
        json!([4_102_444_800_000u64])
    );

    let bad =
        preview(json!({"trigger": {"type": "cron", "expression": "* * *", "timeZone": "UTC"}}))
            .await;
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_body(bad).await["code"], "schedule_invalid");
}

#[tokio::test]
async fn run_now_answers_202_and_records_a_manual_fire() {
    let model = ScriptedModel::new(vec![Step::Text(vec!["CHECKIN_OK"])]);
    let (state, agent) = daemon_with(DaemonState::with_model_adapter(model));
    let app = router(state.clone(), DaemonConfig::default());
    let base = format!("/api/agents/{agent}/schedules");
    let id = json_body(send(&app, "POST", &base, Some(hourly())).await).await["schedule"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let accepted = send(&app, "POST", &format!("{base}/{id}/run"), None).await;
    assert_eq!(accepted.status(), StatusCode::ACCEPTED);
    assert!(json_body(accepted).await["schedule"]["lastFiredAtMs"].is_u64());
    crate::sessions::test_support::within("the manual run to finish", async {
        while state.read().await.schedules[&id]
            .last_safe_outcome
            .is_none()
        {
            tokio::task::yield_now().await;
        }
    })
    .await;

    let history = json_body(send(&app, "GET", &format!("{base}/{id}/history"), None).await).await;
    let runs = history["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["manual"], true);
    assert_eq!(runs[0]["outcome"], "silent");

    let missing = send(&app, "POST", &format!("{base}/missing/run"), None).await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn run_now_conflicts_while_an_occurrence_has_no_outcome() {
    let (state, agent) = daemon_with(DaemonState::new());
    let app = router(state.clone(), DaemonConfig::default());
    let base = format!("/api/agents/{agent}/schedules");
    let id = json_body(send(&app, "POST", &base, Some(hourly())).await).await["schedule"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    state
        .write()
        .await
        .schedules
        .get_mut(&id)
        .unwrap()
        .last_fired = Some(ScheduleLastFired {
        fired_at_ms: 1,
        run_idempotency_key: format!("schedule:{id}:1"),
        manual: false,
    });

    let refused = send(&app, "POST", &format!("{base}/{id}/run"), None).await;
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(refused).await["code"], "schedule_conflict");
    let listed = json_body(send(&app, "GET", &base, None).await).await;
    assert_eq!(listed["schedules"][0]["running"], true);
}

#[tokio::test]
async fn history_merges_pending_and_stored_fires_newest_first() {
    let (state, agent) = daemon_with(DaemonState::new());
    let app = router(state.clone(), DaemonConfig::default());
    let base = format!("/api/agents/{agent}/schedules");
    let id = json_body(send(&app, "POST", &base, Some(hourly())).await).await["schedule"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let store = state.read().await.history.store();
    store
        .upsert_schedule_runs(&[
            fire_record("old", &id, &agent, 10),
            fire_record("both", &id, &agent, 20),
        ])
        .await
        .unwrap();
    {
        let mut guard = state.write().await;
        guard
            .schedule_fires
            .record(fire_record("both", &id, &agent, 20));
        guard
            .schedule_fires
            .record(fire_record("new", &id, &agent, 30));
    }

    let page =
        json_body(send(&app, "GET", &format!("{base}/{id}/history?limit=2"), None).await).await;
    let ids = page["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| run["id"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["new", "both"]);

    for limit in ["0", "51", "x"] {
        let refused = send(
            &app,
            "GET",
            &format!("{base}/{id}/history?limit={limit}"),
            None,
        )
        .await;
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST, "{limit}");
    }
    let unknown = send(&app, "GET", &format!("{base}/missing/history"), None).await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_failing_history_store_answers_503() {
    let mut daemon = DaemonState::new();
    let store = Arc::new(FlakyHistoryStore::new());
    daemon.set_history(HistoryService::new(store.clone()));
    let (state, agent) = daemon_with(daemon);
    let app = router(state, DaemonConfig::default());
    let base = format!("/api/agents/{agent}/schedules");
    let id = json_body(send(&app, "POST", &base, Some(hourly())).await).await["schedule"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    store.set_failing(true);

    let failed = send(&app, "GET", &format!("{base}/{id}/history"), None).await;
    assert_eq!(failed.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(failed.headers()["cache-control"], "no-store");
    assert_eq!(
        json_body(failed).await["code"],
        "schedule_history_unavailable"
    );
}

#[tokio::test]
async fn the_twenty_first_automation_answers_409_and_a_failed_save_503() {
    use crate::control_plane_store::ControlPlaneStoreConfig;
    let (state, agent) = daemon_with(DaemonState::new());
    let app = router(state.clone(), DaemonConfig::default());
    let base = format!("/api/agents/{agent}/schedules");
    let service = crate::schedules::AutomationService::new(
        state.clone(),
        Arc::new(tokio::sync::Mutex::new(())),
    );
    for _ in 0..MAX_AUTOMATIONS_PER_AGENT {
        service
            .create(
                AutomationInput::owner(
                    agent.clone(),
                    "Check".into(),
                    ScheduleTrigger::Interval {
                        interval_ms: 3_600_000,
                    },
                    ScheduleTarget::Workspace,
                ),
                anima_core::primitives::now_millis(),
            )
            .await
            .unwrap();
    }
    let refused = send(&app, "POST", &base, Some(hourly())).await;
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(refused).await["error"], TOO_MANY_AUTOMATIONS);

    let id = state.read().await.schedules.keys().next().unwrap().clone();
    let invalid = std::env::temp_dir().join(format!(
        "anima-automation-route-invalid-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&invalid).unwrap();
    state
        .write()
        .await
        .set_control_plane_store(Some(ControlPlaneStoreConfig::Json(invalid.clone())));
    let failed = send(&app, "DELETE", &format!("{base}/{id}"), None).await;
    assert_eq!(failed.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        state.read().await.schedules.contains_key(&id),
        "the automation stays"
    );
    let _ = std::fs::remove_dir_all(invalid);
}
