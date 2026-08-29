use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::Duration;

use anima_daemon::app as daemon_app;
use axum::body::{to_bytes, Body};
use axum::http::{HeaderMap, Request, StatusCode};
use axum::response::Response;
use axum::Router;
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tower::util::ServiceExt;

const APPROVED_ORIGIN: &str = "http://localhost:4200";
const SECRET_SENTINEL: &str = "123456:secret-sentinel-never-leak";
const REPLACEMENT_SECRET_SENTINEL: &str = "654321:replacement-secret-never-leak";

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

async fn app_with_owner_env(
    host: &str,
    allowed_origins: Option<&str>,
    admin_token: Option<&str>,
) -> Router {
    let _guard = env_lock().lock().await;
    let previous_host = std::env::var_os("ANIMAOS_RS_HOST");
    let previous_origins = std::env::var_os("ANIMA_ALLOWED_UI_ORIGINS");
    let previous_token = std::env::var_os("ANIMA_LOCAL_ADMIN_TOKEN");

    std::env::set_var("ANIMAOS_RS_HOST", host);
    match allowed_origins {
        Some(value) => std::env::set_var("ANIMA_ALLOWED_UI_ORIGINS", value),
        None => std::env::remove_var("ANIMA_ALLOWED_UI_ORIGINS"),
    }
    match admin_token {
        Some(value) => std::env::set_var("ANIMA_LOCAL_ADMIN_TOKEN", value),
        None => std::env::remove_var("ANIMA_LOCAL_ADMIN_TOKEN"),
    }

    let app = daemon_app();

    restore_env("ANIMAOS_RS_HOST", previous_host);
    restore_env("ANIMA_ALLOWED_UI_ORIGINS", previous_origins);
    restore_env("ANIMA_LOCAL_ADMIN_TOKEN", previous_token);
    app
}

fn restore_env(name: &str, previous: Option<std::ffi::OsString>) {
    match previous {
        Some(value) => std::env::set_var(name, value),
        None => std::env::remove_var(name),
    }
}

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, HeaderMap, Value, String) {
    let response: Response = app.clone().oneshot(request).await.expect("app responds");
    let status = response.status();
    let headers = response.headers().clone();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body reads");
    let text = String::from_utf8(body.to_vec()).expect("response body is utf-8");
    let json = serde_json::from_str(&text).unwrap_or(Value::Null);
    (status, headers, json, text)
}

fn request(method: &str, uri: &str, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    builder
        .body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))
        .expect("request builds")
}

fn owner_request(method: &str, uri: &str, body: Option<Value>) -> Request<Body> {
    let mut request = request(method, uri, body);
    request
        .headers_mut()
        .insert("origin", APPROVED_ORIGIN.parse().unwrap());
    request
        .headers_mut()
        .insert("host", "127.0.0.1:8080".parse().unwrap());
    request
}

async fn create_agent(app: &Router, name: &str) -> String {
    let (status, _, body, _) = send(
        app,
        request(
            "POST",
            "/api/agents",
            Some(json!({"name": name, "model": "gpt-5.4"})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    body["agent"]["state"]["id"]
        .as_str()
        .expect("created agent id")
        .to_string()
}

async fn create_connector(app: &Router, agent_id: &str) -> (StatusCode, HeaderMap, Value, String) {
    send(
        app,
        owner_request(
            "POST",
            &format!("/api/agents/{agent_id}/connectors/telegram"),
            Some(json!({"botToken": SECRET_SENTINEL})),
        ),
    )
    .await
}

async fn get_connectors(app: &Router, agent_id: &str) -> (StatusCode, HeaderMap, Value, String) {
    send(
        app,
        request("GET", &format!("/api/agents/{agent_id}/connectors"), None),
    )
    .await
}

async fn wait_for_pending_chat(app: &Router, agent_id: &str) -> String {
    for _ in 0..100 {
        let (status, _, body, _) = get_connectors(app, agent_id).await;
        assert_eq!(status, StatusCode::OK);
        if let Some(chat_id) = body["connectors"][0]["pendingPairing"]["chat"]["id"].as_str() {
            return chat_id.to_string();
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("deterministic Telegram poller should expose a pairing candidate");
}

fn assert_no_store(headers: &HeaderMap) {
    assert_eq!(
        headers
            .get("cache-control")
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
}

fn assert_secret_absent(texts: impl IntoIterator<Item = String>) {
    for text in texts {
        assert!(!text.contains(SECRET_SENTINEL), "secret leaked in {text}");
        assert!(
            !text.contains(REPLACEMENT_SECRET_SENTINEL),
            "replacement secret leaked in {text}"
        );
    }
}

#[tokio::test]
async fn connector_routes_cover_create_list_replace_pair_restart_and_delete() {
    let app = app_with_owner_env("127.0.0.1", None, None).await;
    let agent_id = create_agent(&app, "operator").await;
    let (create_status, create_headers, created, create_text) =
        create_connector(&app, &agent_id).await;

    assert_eq!(create_status, StatusCode::CREATED);
    assert_no_store(&create_headers);
    assert_eq!(created["connector"]["agentId"], agent_id);
    assert_eq!(created["connector"]["type"], "telegram");
    assert_eq!(created["connector"]["status"], "pairing");
    assert!(created["connector"].get("botToken").is_none());
    let connector_id = created["connector"]["id"]
        .as_str()
        .expect("connector id")
        .to_string();

    let (duplicate_status, _, duplicate, _) = create_connector(&app, &agent_id).await;
    assert_eq!(duplicate_status, StatusCode::CONFLICT);
    assert_eq!(duplicate["code"], "connector_already_exists");

    let (list_status, list_headers, listed, list_text) = get_connectors(&app, &agent_id).await;
    assert_eq!(list_status, StatusCode::OK);
    assert_no_store(&list_headers);
    assert_eq!(listed["connectors"].as_array().unwrap().len(), 1);
    assert_eq!(listed["connectors"][0]["id"], connector_id);

    let (replace_status, replace_headers, replaced, replace_text) = send(
        &app,
        owner_request(
            "PUT",
            &format!("/api/agents/{agent_id}/connectors/{connector_id}/credential"),
            Some(json!({"botToken": REPLACEMENT_SECRET_SENTINEL})),
        ),
    )
    .await;
    assert_eq!(replace_status, StatusCode::OK);
    assert_no_store(&replace_headers);
    assert_eq!(replaced["connector"]["id"], connector_id);

    let pending_chat = wait_for_pending_chat(&app, &agent_id).await;
    let (wrong_status, _, wrong, _) = send(
        &app,
        owner_request(
            "POST",
            &format!(
                "/api/agents/{agent_id}/connectors/{connector_id}/pairings/wrong-chat/approve"
            ),
            None,
        ),
    )
    .await;
    assert_eq!(wrong_status, StatusCode::CONFLICT);
    assert_eq!(wrong["code"], "connector_pairing_not_found");

    let (approve_status, approve_headers, approved, approve_text) = send(
        &app,
        owner_request(
            "POST",
            &format!(
                "/api/agents/{agent_id}/connectors/{connector_id}/pairings/{pending_chat}/approve"
            ),
            None,
        ),
    )
    .await;
    assert_eq!(approve_status, StatusCode::OK);
    assert_no_store(&approve_headers);
    assert_eq!(approved["connector"]["approvedChat"]["id"], pending_chat);
    assert!(approved["connector"]["pendingPairing"].is_null());

    let (restart_status, restart_headers, restarted, restart_text) = send(
        &app,
        owner_request(
            "POST",
            &format!("/api/agents/{agent_id}/connectors/{connector_id}/restart"),
            None,
        ),
    )
    .await;
    assert_eq!(restart_status, StatusCode::OK);
    assert_no_store(&restart_headers);
    assert_eq!(restarted["connector"]["id"], connector_id);

    let (delete_status, delete_headers, deleted, delete_text) = send(
        &app,
        owner_request(
            "DELETE",
            &format!("/api/agents/{agent_id}/connectors/{connector_id}"),
            None,
        ),
    )
    .await;
    assert_eq!(delete_status, StatusCode::OK);
    assert_no_store(&delete_headers);
    assert_eq!(deleted, json!({"deleted": true}));
    let (_, _, listed_after_delete, after_delete_text) = get_connectors(&app, &agent_id).await;
    assert_eq!(listed_after_delete, json!({"connectors": []}));

    assert_secret_absent([
        create_text,
        list_text,
        replace_text,
        approve_text,
        restart_text,
        delete_text,
        after_delete_text,
    ]);
}

#[tokio::test]
async fn connector_routes_enforce_agent_ownership_and_not_found_without_secret_leaks() {
    let app = app_with_owner_env("127.0.0.1", None, None).await;
    let owner_id = create_agent(&app, "owner").await;
    let other_id = create_agent(&app, "other").await;
    let (status, _, created, create_text) = create_connector(&app, &owner_id).await;
    assert_eq!(status, StatusCode::CREATED);
    let connector_id = created["connector"]["id"].as_str().unwrap();

    let mut responses = BTreeMap::new();
    for (name, method, suffix, body) in [
        (
            "replace",
            "PUT",
            "/credential",
            Some(json!({"botToken": SECRET_SENTINEL})),
        ),
        ("pairing", "POST", "/pairings/foreign-chat/approve", None),
        ("restart", "POST", "/restart", None),
        ("delete", "DELETE", "", None),
        ("messages", "GET", "/messages", None),
        ("send", "POST", "/messages", Some(json!({"text": "hello"}))),
    ] {
        let uri = format!("/api/agents/{other_id}/connectors/{connector_id}{suffix}");
        let request = if method == "GET" {
            request(method, &uri, body)
        } else {
            owner_request(method, &uri, body)
        };
        let (status, _, response, text) = send(&app, request).await;
        responses.insert(name, (status, response, text));
    }

    for (status, response, _) in responses.values() {
        assert_eq!(*status, StatusCode::NOT_FOUND);
        assert_eq!(response["code"], "not_found");
    }
    let (missing_status, _, missing, missing_text) = get_connectors(&app, "agent-missing").await;
    assert_eq!(missing_status, StatusCode::NOT_FOUND);
    assert_eq!(missing["code"], "not_found");
    let (missing_create_status, _, missing_create, missing_create_text) =
        create_connector(&app, "agent-missing").await;
    assert_eq!(missing_create_status, StatusCode::NOT_FOUND);
    assert_eq!(missing_create["code"], "not_found");
    assert_secret_absent(
        std::iter::once(create_text)
            .chain(responses.into_values().map(|(_, _, text)| text))
            .chain([missing_text, missing_create_text]),
    );
}

#[tokio::test]
async fn connector_thread_is_dedicated_bounded_and_stably_paginated() {
    let app = app_with_owner_env("127.0.0.1", None, None).await;
    let agent_id = create_agent(&app, "threader").await;
    let (_, _, created, _) = create_connector(&app, &agent_id).await;
    let connector_id = created["connector"]["id"].as_str().unwrap();

    let (unpaired_status, _, unpaired, _) = send(
        &app,
        owner_request(
            "POST",
            &format!("/api/agents/{agent_id}/connectors/{connector_id}/messages"),
            Some(json!({"text": "not paired"})),
        ),
    )
    .await;
    assert_eq!(unpaired_status, StatusCode::OK);
    assert_eq!(unpaired["deliveryQueued"], false);
    assert_eq!(unpaired["result"]["status"], "success");
    assert!(unpaired["messages"]
        .as_array()
        .unwrap()
        .iter()
        .all(|message| { message["roomId"] == created["connector"]["roomId"] }));

    let chat_id = wait_for_pending_chat(&app, &agent_id).await;
    let _ = send(
        &app,
        owner_request(
            "POST",
            &format!("/api/agents/{agent_id}/connectors/{connector_id}/pairings/{chat_id}/approve"),
            None,
        ),
    )
    .await;

    let (_, _, ordinary_run, _) = send(
        &app,
        request(
            "POST",
            &format!("/api/agents/{agent_id}/run"),
            Some(json!({"text": "ordinary web room"})),
        ),
    )
    .await;
    let ordinary_room = ordinary_run["agent"]["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()["roomId"]
        .as_str()
        .unwrap()
        .to_string();

    for text in ["first telegram", "second telegram", "third telegram"] {
        let (status, headers, sent, raw) = send(
            &app,
            owner_request(
                "POST",
                &format!("/api/agents/{agent_id}/connectors/{connector_id}/messages"),
                Some(json!({"text": text})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{raw}");
        assert_no_store(&headers);
        assert_eq!(sent["deliveryQueued"], true);
        assert_eq!(sent["result"]["status"], "success");
        assert!(sent["messages"].as_array().unwrap().iter().all(|message| {
            message["roomId"] != ordinary_room
                && message["roomId"] == created["connector"]["roomId"]
        }));
    }

    let uri = format!("/api/agents/{agent_id}/connectors/{connector_id}/messages?limit=2");
    let (first_status, first_headers, first_page, first_text) =
        send(&app, request("GET", &uri, None)).await;
    assert_eq!(first_status, StatusCode::OK);
    assert_no_store(&first_headers);
    assert_eq!(first_page["messages"].as_array().unwrap().len(), 2);
    let before = first_page["nextBefore"].as_str().expect("next cursor");
    let uri = format!(
        "/api/agents/{agent_id}/connectors/{connector_id}/messages?limit=2&before={before}"
    );
    let (second_status, _, second_page, second_text) = send(&app, request("GET", &uri, None)).await;
    assert_eq!(second_status, StatusCode::OK);
    assert_eq!(second_page["messages"].as_array().unwrap().len(), 2);
    assert!(first_page["messages"]
        .as_array()
        .unwrap()
        .iter()
        .all(|left| {
            second_page["messages"]
                .as_array()
                .unwrap()
                .iter()
                .all(|right| left["id"] != right["id"])
        }));
    assert!(first_text.contains("third telegram"));
    assert!(!first_text.contains("ordinary web room"));
    assert!(!second_text.contains("ordinary web room"));

    for invalid in [
        "limit=0",
        "limit=101",
        "limit=nope",
        "before=missing-message",
    ] {
        let (status, _, body, _) = send(
            &app,
            request(
                "GET",
                &format!("/api/agents/{agent_id}/connectors/{connector_id}/messages?{invalid}"),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "invalid_request");
    }
}

#[tokio::test]
async fn connector_inputs_are_bounded_before_mutation() {
    let app = app_with_owner_env("127.0.0.1", None, None).await;
    let agent_id = create_agent(&app, "bounded").await;
    let (missing_status, _, missing_body, _) = send(
        &app,
        owner_request(
            "POST",
            &format!("/api/agents/{agent_id}/connectors/telegram"),
            Some(json!({})),
        ),
    )
    .await;
    assert_eq!(missing_status, StatusCode::BAD_REQUEST);
    assert_eq!(missing_body["code"], "invalid_request");

    for token in ["".to_string(), "x".repeat(257), "x".repeat(4097)] {
        let (status, _, body, _) = send(
            &app,
            owner_request(
                "POST",
                &format!("/api/agents/{agent_id}/connectors/telegram"),
                Some(json!({"botToken": token})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["code"], "connector_token_invalid");
    }
    let (_, _, listed, _) = get_connectors(&app, &agent_id).await;
    assert_eq!(listed, json!({"connectors": []}));

    let (_, _, created, _) = create_connector(&app, &agent_id).await;
    let connector_id = created["connector"]["id"].as_str().unwrap();
    let chat_id = wait_for_pending_chat(&app, &agent_id).await;
    let _ = send(
        &app,
        owner_request(
            "POST",
            &format!("/api/agents/{agent_id}/connectors/{connector_id}/pairings/{chat_id}/approve"),
            None,
        ),
    )
    .await;
    for text in ["".to_string(), "x".repeat(4097), "x".repeat(16_385)] {
        let (status, _, body, _) = send(
            &app,
            owner_request(
                "POST",
                &format!("/api/agents/{agent_id}/connectors/{connector_id}/messages"),
                Some(json!({"text": text})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "invalid_request");
    }
    let (_, _, messages, _) = send(
        &app,
        request(
            "GET",
            &format!("/api/agents/{agent_id}/connectors/{connector_id}/messages"),
            None,
        ),
    )
    .await;
    assert_eq!(messages["messages"], json!([]));
}

#[tokio::test]
async fn local_owner_guard_fails_before_connector_side_effects() {
    for (name, host, origins, admin, mutate) in [
        ("remote bind", "0.0.0.0", None, None, "origin"),
        ("unapproved origin", "127.0.0.1", None, None, "evil-origin"),
        ("forwarded", "127.0.0.1", None, None, "forwarded"),
        ("originless", "127.0.0.1", None, None, "originless"),
        (
            "wrong bearer",
            "127.0.0.1",
            None,
            Some("right-token"),
            "wrong-token",
        ),
    ] {
        let app = app_with_owner_env(host, origins, admin).await;
        let agent_id = create_agent(&app, name).await;
        let mut blocked = owner_request(
            "POST",
            &format!("/api/agents/{agent_id}/connectors/telegram"),
            Some(json!({"botToken": SECRET_SENTINEL})),
        );
        match mutate {
            "evil-origin" => {
                blocked.headers_mut().insert(
                    "origin",
                    "http://localhost:4200.evil.example".parse().unwrap(),
                );
            }
            "forwarded" => {
                blocked
                    .headers_mut()
                    .insert("forwarded", "for=127.0.0.1".parse().unwrap());
            }
            "originless" => {
                blocked.headers_mut().remove("origin");
            }
            "wrong-token" => {
                blocked.headers_mut().remove("origin");
                blocked
                    .headers_mut()
                    .insert("authorization", "Bearer wrong-token".parse().unwrap());
            }
            _ => {}
        }
        let (status, headers, body, text) = send(&app, blocked).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{name}: {text}");
        assert_no_store(&headers);
        assert_eq!(body["code"], "local_owner_required");
        assert_secret_absent([text]);
        let (list_status, _, listed, _) = get_connectors(&app, &agent_id).await;
        assert_eq!(list_status, StatusCode::OK);
        assert_eq!(listed, json!({"connectors": []}), "{name}");
    }

    let forwarded_app = app_with_owner_env("127.0.0.1", None, None).await;
    let forwarded_agent_id = create_agent(&forwarded_app, "forwarding-headers").await;
    for header in [
        "forwarded",
        "via",
        "x-forwarded-for",
        "x-forwarded-host",
        "x-forwarded-proto",
        "x-forwarded-port",
        "x-real-ip",
    ] {
        let mut blocked = owner_request(
            "POST",
            &format!("/api/agents/{forwarded_agent_id}/connectors/telegram"),
            Some(json!({"botToken": SECRET_SENTINEL})),
        );
        blocked
            .headers_mut()
            .insert(header, "malicious-forward".parse().unwrap());
        let (status, _, body, _) = send(&forwarded_app, blocked).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{header}");
        assert_eq!(body["code"], "local_owner_required");
    }
    let (_, _, forwarded_list, _) = get_connectors(&forwarded_app, &forwarded_agent_id).await;
    assert_eq!(forwarded_list, json!({"connectors": []}));

    let app = app_with_owner_env(
        "127.0.0.1",
        Some("https://custom-ui.example"),
        Some("right-token"),
    )
    .await;
    let agent_id = create_agent(&app, "allowed").await;
    let mut originless = request(
        "POST",
        &format!("/api/agents/{agent_id}/connectors/telegram"),
        Some(json!({"botToken": SECRET_SENTINEL})),
    );
    originless
        .headers_mut()
        .insert("host", "localhost:8080".parse().unwrap());
    originless
        .headers_mut()
        .insert("authorization", "Bearer right-token".parse().unwrap());
    let (token_status, _, _, _) = send(&app, originless).await;
    assert_eq!(token_status, StatusCode::CREATED);

    let other_agent_id = create_agent(&app, "custom-origin").await;
    let mut custom_origin = owner_request(
        "POST",
        &format!("/api/agents/{other_agent_id}/connectors/telegram"),
        Some(json!({"botToken": "custom-origin-token"})),
    );
    custom_origin
        .headers_mut()
        .insert("origin", "https://custom-ui.example".parse().unwrap());
    let (custom_status, _, _, _) = send(&app, custom_origin).await;
    assert_eq!(custom_status, StatusCode::CREATED);

    let default_agent_id = create_agent(&app, "default-origin-remains").await;
    let (default_status, _, _, _) = create_connector(&app, &default_agent_id).await;
    assert_eq!(default_status, StatusCode::CREATED);
}

#[tokio::test]
async fn openapi_registers_connector_paths_tags_and_camel_case_schemas() {
    let app = app_with_owner_env("127.0.0.1", None, None).await;
    let (status, _, document, text) = send(&app, request("GET", "/openapi.json", None)).await;
    assert_eq!(status, StatusCode::OK);
    for path in [
        "/api/agents/{agent_id}/connectors",
        "/api/agents/{agent_id}/connectors/telegram",
        "/api/agents/{agent_id}/connectors/{connector_id}/credential",
        "/api/agents/{agent_id}/connectors/{connector_id}/pairings/{chat_id}/approve",
        "/api/agents/{agent_id}/connectors/{connector_id}/restart",
        "/api/agents/{agent_id}/connectors/{connector_id}",
        "/api/agents/{agent_id}/connectors/{connector_id}/messages",
    ] {
        assert!(document["paths"].get(path).is_some(), "missing {path}");
    }
    assert!(text.contains("\"name\":\"connectors\""));
    assert!(text.contains("\"name\":\"connector-thread\""));
    for property in ["botToken", "pendingPairing", "nextBefore", "deliveryQueued"] {
        assert!(
            text.contains(property),
            "missing camelCase property {property}"
        );
    }
    for forbidden in [
        "bot_token",
        "pending_pairing",
        "next_before",
        "delivery_queued",
    ] {
        assert!(!text.contains(forbidden), "snake_case leaked: {forbidden}");
    }
    assert_eq!(
        document["components"]["schemas"]["TelegramCredentialRequest"]["required"],
        json!(["botToken"])
    );
    assert_eq!(
        document["components"]["schemas"]["ConnectorMessageRequest"]["required"],
        json!(["text"])
    );
}
