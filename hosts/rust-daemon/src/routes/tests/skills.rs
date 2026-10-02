use super::*;

use serde_json::{json, Value};

use crate::skills::test_support::{skill_text, temp_workspace, with_workspace, write_skill};
use crate::skills::{
    skill_hash, SKILLS_NEED_WORKSPACE, SKILL_BODY_TOO_LARGE, SKILL_FILE_UNREVIEWED,
    SKILL_HASH_MISMATCH, SKILL_HASH_REQUIRED, SKILL_NOT_CHANGED, SKILL_SLUG_INVALID,
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

/// A daemon whose workspace is a fresh temporary folder.
fn daemon(label: &str) -> (Arc<RwLock<DaemonState>>, std::path::PathBuf) {
    let root = temp_workspace(label);
    let state = with_workspace(DaemonState::new(), &root);
    (Arc::new(RwLock::new(state)), root)
}

fn notes() -> Value {
    json!({"name": "notes", "description": "About notes", "body": "Do notes."})
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

#[tokio::test]
async fn the_skill_routes_require_the_owner_and_a_workspace() {
    let (state, _) = daemon("routes-auth");
    let app = router(state, DaemonConfig::default());
    for (method, body) in [("GET", None), ("PUT", Some(notes()))] {
        let refused = app
            .clone()
            .oneshot(request(
                method,
                "/api/skills/notes",
                "https://untrusted.example",
                body,
            ))
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::FORBIDDEN, "{method}");
        assert_eq!(refused.headers()["cache-control"], "no-store");
    }

    let bare = router(
        Arc::new(RwLock::new(DaemonState::new())),
        DaemonConfig::default(),
    );
    for (method, uri, body) in [
        ("GET", "/api/skills", None),
        ("PUT", "/api/skills/notes", Some(notes())),
    ] {
        let response = send(&bare, method, uri, body).await;
        assert_eq!(response.status(), StatusCode::CONFLICT, "{method} {uri}");
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(json_body(response).await["error"], SKILLS_NEED_WORKSPACE);
    }
}

#[tokio::test]
async fn put_creates_then_replaces_a_skill_and_get_lists_it() {
    let (state, root) = daemon("routes-put");
    let app = router(state, DaemonConfig::default());

    let created = send(&app, "PUT", "/api/skills/notes", Some(notes())).await;
    assert_eq!(created.status(), StatusCode::CREATED);
    assert_eq!(created.headers()["cache-control"], "no-store");
    let created = json_body(created).await;
    assert_eq!(created["skill"]["slug"], "notes");
    assert_eq!(created["skill"]["status"], "active");
    assert_eq!(created["skill"]["enabled"], true);
    assert_eq!(
        created["skill"]["approvedHash"],
        skill_hash(skill_text("notes").as_bytes())
    );
    assert_eq!(
        std::fs::read_to_string(root.join("skills/notes/SKILL.md")).unwrap(),
        skill_text("notes")
    );

    let replaced = send(&app, "PUT", "/api/skills/notes", Some(notes())).await;
    assert_eq!(replaced.status(), StatusCode::OK);

    let listed = json_body(send(&app, "GET", "/api/skills", None).await).await;
    assert_eq!(listed["skills"].as_array().unwrap().len(), 1);
    let detail = send(&app, "GET", "/api/skills/notes", None).await;
    assert_eq!(detail.headers()["cache-control"], "no-store");
    let detail = json_body(detail).await;
    assert_eq!(detail["skill"]["slug"], "notes");
    assert_eq!(detail["file"]["body"], "Do notes.");
    assert_eq!(detail["file"]["problem"], Value::Null);

    for (uri, body, message) in [
        ("/api/skills/Bad_Slug", notes(), SKILL_SLUG_INVALID),
        ("/api/skills/-lead", notes(), SKILL_SLUG_INVALID),
        ("/api/skills/con", notes(), SKILL_SLUG_INVALID),
        ("/api/skills/nul", notes(), SKILL_SLUG_INVALID),
        (
            "/api/skills/big",
            json!({"name": "big", "description": "d", "body": "b".repeat(32 * 1024 + 1)}),
            SKILL_BODY_TOO_LARGE,
        ),
    ] {
        let response = send(&app, "PUT", uri, Some(body)).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{uri}");
        assert_eq!(json_body(response).await["error"], message);
    }
    let unknown_field = send(
        &app,
        "PUT",
        "/api/skills/x",
        Some(json!({"name": "x", "description": "d", "body": "b", "extra": 1})),
    )
    .await;
    assert_eq!(unknown_field.status(), StatusCode::BAD_REQUEST);
    for slug in ["ghost", "con"] {
        assert_eq!(
            send(&app, "GET", &format!("/api/skills/{slug}"), None)
                .await
                .status(),
            StatusCode::NOT_FOUND,
            "{slug}"
        );
    }
}

#[tokio::test]
async fn a_32_kib_body_survives_json_escaping() {
    let (state, _) = daemon("routes-escaped");
    let app = router(state, DaemonConfig::default());
    let body = format!("x{}", "\u{1}".repeat(32 * 1024 - 1));
    let response = send(
        &app,
        "PUT",
        "/api/skills/escaped",
        Some(json!({"name": "escaped", "description": "d", "body": body})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn putting_over_an_unreviewed_file_answers_409() {
    let (state, root) = daemon("routes-unreviewed");
    let app = router(state, DaemonConfig::default());
    let path = write_skill(&root, "found", &skill_text("found"));

    let response = send(
        &app,
        "PUT",
        "/api/skills/found",
        Some(json!({"name": "found", "description": "d", "body": "Overwritten."})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(json_body(response).await["error"], SKILL_FILE_UNREVIEWED);
    assert_eq!(std::fs::read_to_string(path).unwrap(), skill_text("found"));
    let listed = json_body(send(&app, "GET", "/api/skills", None).await).await;
    assert!(listed["skills"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn patch_turns_a_skill_off_and_delete_moves_it_to_the_trash() {
    let (state, root) = daemon("routes-patch");
    let app = router(state, DaemonConfig::default());
    send(&app, "PUT", "/api/skills/notes", Some(notes())).await;

    let patched = json_body(
        send(
            &app,
            "PATCH",
            "/api/skills/notes",
            Some(json!({"enabled": false})),
        )
        .await,
    )
    .await;
    assert_eq!(patched["skill"]["enabled"], false);
    assert_eq!(
        send(
            &app,
            "PATCH",
            "/api/skills/ghost",
            Some(json!({"enabled": true}))
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    let deleted = send(&app, "DELETE", "/api/skills/notes", None).await;
    assert_eq!(deleted.status(), StatusCode::OK);
    let deleted = json_body(deleted).await;
    assert_eq!(deleted["deleted"], true);
    let trash = deleted["trashPath"].as_str().unwrap().to_string();
    assert!(trash.starts_with(".anima-trash/skills/notes-"), "{trash}");
    assert!(root.join(&trash).join("SKILL.md").exists());
    assert_eq!(
        send(&app, "GET", "/api/skills/notes", None).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(&app, "DELETE", "/api/skills/notes", None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn an_edited_file_reads_changed_and_needs_the_reviewed_hash() {
    let (state, root) = daemon("routes-approve");
    let app = router(state, DaemonConfig::default());
    send(&app, "PUT", "/api/skills/notes", Some(notes())).await;
    write_skill(&root, "notes", &skill_text("notes, edited by hand"));

    let listed = json_body(send(&app, "GET", "/api/skills", None).await).await;
    assert_eq!(listed["skills"][0]["status"], "changed");
    let detail = json_body(send(&app, "GET", "/api/skills/notes", None).await).await;
    let reviewed = skill_hash(skill_text("notes, edited by hand").as_bytes());
    assert_eq!(detail["file"]["hash"], reviewed.as_str());
    assert_eq!(detail["skill"]["name"], "notes", "the approved name stays");

    for hash in ["", "not-a-hash"] {
        let malformed = send(
            &app,
            "POST",
            "/api/skills/notes/approve",
            Some(json!({"hash": hash})),
        )
        .await;
        assert_eq!(malformed.status(), StatusCode::BAD_REQUEST, "{hash:?}");
        assert_eq!(json_body(malformed).await["error"], SKILL_HASH_REQUIRED);
    }

    let stale = send(
        &app,
        "POST",
        "/api/skills/notes/approve",
        Some(json!({"hash": "0".repeat(64)})),
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(stale).await["error"], SKILL_HASH_MISMATCH);

    let approved = send(
        &app,
        "POST",
        "/api/skills/notes/approve",
        Some(json!({"hash": reviewed})),
    )
    .await;
    assert_eq!(approved.status(), StatusCode::OK);
    let approved = json_body(approved).await;
    assert_eq!(approved["skill"]["status"], "active");
    assert_eq!(approved["skill"]["name"], "notes, edited by hand");

    let again = send(
        &app,
        "POST",
        "/api/skills/notes/approve",
        Some(json!({"hash": reviewed})),
    )
    .await;
    assert_eq!(again.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(again).await["error"], SKILL_NOT_CHANGED);
}

#[tokio::test]
async fn a_failed_save_answers_503_and_keeps_the_old_skill() {
    use crate::control_plane_store::ControlPlaneStoreConfig;

    let (state, _) = daemon("routes-fail");
    let app = router(state.clone(), DaemonConfig::default());
    send(&app, "PUT", "/api/skills/notes", Some(notes())).await;
    let broken = temp_workspace("routes-fail-store");
    state
        .write()
        .await
        .set_control_plane_store(Some(ControlPlaneStoreConfig::Json(broken.clone())));

    let edited = json!({"name": "notes", "description": "About notes", "body": "Different."});
    for (method, uri, body) in [
        ("PUT", "/api/skills/notes", Some(edited)),
        (
            "PATCH",
            "/api/skills/notes",
            Some(json!({"enabled": false})),
        ),
        ("DELETE", "/api/skills/notes", None),
    ] {
        let response = send(&app, method, uri, body).await;
        assert_eq!(
            response.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "{method}"
        );
        assert_eq!(response.headers()["cache-control"], "no-store");
    }

    let listed = json_body(send(&app, "GET", "/api/skills", None).await).await;
    let skill = &listed["skills"][0];
    assert_eq!(skill["enabled"], true, "the switch did not move");
    assert_eq!(
        skill["approvedHash"],
        skill_hash(skill_text("notes").as_bytes()),
        "the approved content did not move"
    );
    assert_eq!(
        skill["status"], "changed",
        "the unsaved new file is not trusted"
    );
    let _ = std::fs::remove_dir_all(broken);
}
