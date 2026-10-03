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

fn proposal(name: &str) -> crate::skills::Proposal {
    crate::skills::Proposal {
        by: crate::skills::ProposedBy {
            agent_id: "agent-1".into(),
            session_id: "chat:1".into(),
            run_id: "run_1".into(),
        },
        name: name.into(),
        description: format!("About {name}"),
        body: format!("Do {name}."),
        slug: None,
    }
}

fn import_request(origin: &str, boundary: &str, body: Vec<u8>) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/api/skills/import")
        .header("host", "127.0.0.1:8080")
        .header("origin", origin)
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap()
}

#[tokio::test]
async fn drafts_are_listed_approved_and_rejected_through_the_routes() {
    use crate::skills::{SKILL_DRAFT_DECIDED, SKILL_HASH_REQUIRED};

    let (state, root) = daemon("routes-drafts");
    let app = router(state.clone(), DaemonConfig::default());
    let proposed = crate::skills::test_support::service(&state)
        .propose(proposal("Notes"))
        .await
        .unwrap();
    write_skill(&root, "found", &skill_text("found"));

    let pending = send(&app, "GET", "/api/skill-drafts", None).await;
    assert_eq!(pending.headers()["cache-control"], "no-store");
    let pending = json_body(pending).await;
    let drafts = pending["drafts"].as_array().unwrap();
    assert_eq!(drafts.len(), 2);
    let agent = drafts
        .iter()
        .find(|draft| draft["id"] == proposed.id.as_str())
        .unwrap();
    assert_eq!(agent["source"], "agent");
    assert_eq!(agent["status"], "pending");
    assert_eq!(agent["proposedBy"]["runId"], "run_1");
    assert_eq!(agent["stale"], false);
    assert_eq!(agent["body"], "Do Notes.");
    let file = drafts
        .iter()
        .find(|draft| draft["id"] == "file:found")
        .unwrap();
    let reviewed = skill_hash(skill_text("found").as_bytes());
    assert_eq!(file["fileHash"], reviewed.as_str());

    let approved = send(
        &app,
        "POST",
        &format!("/api/skill-drafts/{}/approve", proposed.id),
        None,
    )
    .await;
    assert_eq!(approved.status(), StatusCode::OK);
    assert_eq!(approved.headers()["cache-control"], "no-store");
    let approved = json_body(approved).await;
    assert_eq!(approved["skill"]["slug"], "notes");
    assert_eq!(approved["skill"]["status"], "active");
    assert_eq!(approved["draft"]["status"], "approved");

    for body in [json!({}), json!({"hash": "not-a-hash"})] {
        let hashless = send(
            &app,
            "POST",
            "/api/skill-drafts/file%3Afound/approve",
            Some(body),
        )
        .await;
        assert_eq!(hashless.status(), StatusCode::BAD_REQUEST);
        assert_eq!(json_body(hashless).await["error"], SKILL_HASH_REQUIRED);
    }
    let file_approved = send(
        &app,
        "POST",
        "/api/skill-drafts/file%3Afound/approve",
        Some(json!({"hash": reviewed})),
    )
    .await;
    assert_eq!(file_approved.status(), StatusCode::OK);
    assert_eq!(json_body(file_approved).await["skill"]["slug"], "found");

    let decided = send(
        &app,
        "POST",
        &format!("/api/skill-drafts/{}/reject", proposed.id),
        None,
    )
    .await;
    assert_eq!(decided.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(decided).await["error"], SKILL_DRAFT_DECIDED);

    let other = crate::skills::test_support::service(&state)
        .propose(proposal("Other"))
        .await
        .unwrap();
    let rejected = send(
        &app,
        "POST",
        &format!("/api/skill-drafts/{}/reject", other.id),
        None,
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::OK);
    assert_eq!(json_body(rejected).await["draft"]["status"], "rejected");

    let history =
        json_body(send(&app, "GET", "/api/skill-drafts?status=decided", None).await).await;
    assert_eq!(history["drafts"].as_array().unwrap().len(), 2);
    let invalid = send(&app, "GET", "/api/skill-drafts?status=bogus", None).await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    assert_eq!(invalid.headers()["cache-control"], "no-store");
    assert_eq!(
        json_body(invalid).await["error"],
        crate::routes::approvals::STATUS_INVALID
    );
    let malformed = send(&app, "GET", "/api/skill-drafts?status=%ZZ", None).await;
    assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);
    assert_eq!(malformed.headers()["cache-control"], "no-store");
    assert_eq!(json_body(malformed).await["error"], "malformed query");
    assert_eq!(
        send(&app, "POST", "/api/skill-drafts/skd_missing/reject", None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn the_draft_routes_require_the_owner_and_a_workspace() {
    let (state, _) = daemon("routes-drafts-auth");
    let app = router(state, DaemonConfig::default());
    for (method, uri) in [
        ("GET", "/api/skill-drafts"),
        ("POST", "/api/skill-drafts/skd_x/approve"),
        ("POST", "/api/skill-drafts/skd_x/reject"),
    ] {
        let refused = app
            .clone()
            .oneshot(request(method, uri, "https://untrusted.example", None))
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::FORBIDDEN, "{method} {uri}");
        assert_eq!(refused.headers()["cache-control"], "no-store");
    }
    let bare = router(
        Arc::new(RwLock::new(DaemonState::new())),
        DaemonConfig::default(),
    );
    for (method, uri) in [
        ("GET", "/api/skill-drafts"),
        ("GET", "/api/skill-drafts?status=decided"),
        ("POST", "/api/skill-drafts/skd_x/approve"),
        ("POST", "/api/skill-drafts/skd_x/reject"),
    ] {
        let response = send(&bare, method, uri, None).await;
        assert_eq!(response.status(), StatusCode::CONFLICT, "{method} {uri}");
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(json_body(response).await["error"], SKILLS_NEED_WORKSPACE);
    }
    let import = bare
        .clone()
        .oneshot(import_request(
            OWNER_ORIGIN,
            "b",
            crate::routes::multipart::encode_form(
                "b",
                &[("file", Some("SKILL.md"), skill_text("Imported").as_bytes())],
            ),
        ))
        .await
        .unwrap();
    assert_eq!(import.status(), StatusCode::CONFLICT);
    assert_eq!(import.headers()["cache-control"], "no-store");
    assert_eq!(json_body(import).await["error"], SKILLS_NEED_WORKSPACE);
    let malformed = send(
        &app,
        "POST",
        "/api/skill-drafts/skd_x/approve",
        Some(json!({"unknown": 1})),
    )
    .await;
    assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);
    assert_eq!(malformed.headers()["cache-control"], "no-store");
}

#[tokio::test]
async fn a_draft_for_a_skill_approved_since_reads_stale() {
    let (state, _) = daemon("routes-stale");
    let app = router(state.clone(), DaemonConfig::default());
    send(&app, "PUT", "/api/skills/notes", Some(notes())).await;
    crate::skills::test_support::service(&state)
        .propose(proposal("notes"))
        .await
        .unwrap();
    send(
        &app,
        "PUT",
        "/api/skills/notes",
        Some(json!({"name": "notes", "description": "About notes", "body": "Changed meanwhile."})),
    )
    .await;

    let drafts = json_body(send(&app, "GET", "/api/skill-drafts", None).await).await;
    let draft = &drafts["drafts"][0];
    assert_eq!(draft["stale"], true);
    assert_ne!(draft["baseHash"], draft["currentHash"]);
}

#[tokio::test]
async fn importing_a_skill_file_creates_a_draft() {
    use crate::routes::multipart::encode_form;
    use crate::skills::{IMPORT_NOT_MULTIPART, IMPORT_TOO_LARGE, SKILL_FILE_NO_FRONT_MATTER};

    let (state, _) = daemon("routes-import");
    let app = router(state, DaemonConfig::default());
    let text = skill_text("Imported");

    let refused = app
        .clone()
        .oneshot(import_request(
            "https://untrusted.example",
            "b",
            encode_form("b", &[("file", Some("SKILL.md"), text.as_bytes())]),
        ))
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert_eq!(refused.headers()["cache-control"], "no-store");

    let created = app
        .clone()
        .oneshot(import_request(
            OWNER_ORIGIN,
            "b",
            encode_form("b", &[("file", Some("SKILL.md"), text.as_bytes())]),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    assert_eq!(created.headers()["cache-control"], "no-store");
    let created = json_body(created).await;
    assert_eq!(created["draft"]["slug"], "imported");
    assert_eq!(created["draft"]["source"], "import");
    assert_eq!(created["draft"]["status"], "pending");

    let named = json_body(
        app.clone()
            .oneshot(import_request(
                OWNER_ORIGIN,
                "b",
                encode_form(
                    "b",
                    &[
                        ("file", Some("SKILL.md"), text.as_bytes()),
                        ("slug", None, &b"chosen"[..]),
                    ],
                ),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(named["draft"]["slug"], "chosen");

    let huge = format!(
        "---\nname: huge\ndescription: d\n---\n\n{}",
        "b".repeat(64 * 1024)
    );
    let way_over = vec![b'x'; 200 * 1024];
    for (body, message) in [
        (
            encode_form("b", &[("other", None, &b"x"[..])]),
            IMPORT_NOT_MULTIPART,
        ),
        (
            encode_form("b", &[("file", Some("SKILL.md"), huge.as_bytes())]),
            IMPORT_TOO_LARGE,
        ),
        (
            encode_form("b", &[("file", Some("SKILL.md"), &way_over[..])]),
            IMPORT_TOO_LARGE,
        ),
        (
            encode_form("b", &[("file", Some("SKILL.md"), &b"no front matter"[..])]),
            SKILL_FILE_NO_FRONT_MATTER,
        ),
        (b"--b\r\nnot a form".to_vec(), IMPORT_NOT_MULTIPART),
    ] {
        let response = app
            .clone()
            .oneshot(import_request(OWNER_ORIGIN, "b", body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{message}");
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(json_body(response).await["error"], message);
    }
    let bad_slug = app
        .clone()
        .oneshot(import_request(
            OWNER_ORIGIN,
            "b",
            encode_form(
                "b",
                &[
                    ("file", Some("SKILL.md"), text.as_bytes()),
                    ("slug", None, &[0xff, 0xfe][..]),
                ],
            ),
        ))
        .await
        .unwrap();
    assert_eq!(bad_slug.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_body(bad_slug).await["error"], SKILL_SLUG_INVALID);
    let json = send(
        &app,
        "POST",
        "/api/skills/import",
        Some(json!({"file": "x"})),
    )
    .await;
    assert_eq!(json.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_body(json).await["error"], IMPORT_NOT_MULTIPART);
    assert_eq!(
        send(&app, "GET", "/api/skills/import", None).await.status(),
        StatusCode::METHOD_NOT_ALLOWED,
        "import is a reserved slug"
    );
}
