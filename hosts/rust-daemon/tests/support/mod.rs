use anima_daemon::app as daemon_app;
use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use axum::Router;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tower::util::ServiceExt;

fn workspace_root_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

pub(crate) fn test_app() -> Router {
    daemon_app()
}

pub(crate) struct WorkspaceRootGuard {
    previous: Option<OsString>,
    path: PathBuf,
    _lock: MutexGuard<'static, ()>,
}

impl WorkspaceRootGuard {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for WorkspaceRootGuard {
    fn drop(&mut self) {
        if let Some(previous) = &self.previous {
            std::env::set_var("ANIMAOS_WORKSPACE_ROOT", previous);
        } else {
            std::env::remove_var("ANIMAOS_WORKSPACE_ROOT");
        }
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub(crate) fn use_temp_workspace_root(prefix: &str) -> WorkspaceRootGuard {
    let lock = workspace_root_lock()
        .lock()
        .expect("workspace root lock should not be poisoned");
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should move forward")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("anima-daemon-{prefix}-{unique}"));
    fs::create_dir_all(&path).expect("workspace dir should be created");

    let previous = std::env::var_os("ANIMAOS_WORKSPACE_ROOT");
    std::env::set_var("ANIMAOS_WORKSPACE_ROOT", &path);

    WorkspaceRootGuard {
        previous,
        path,
        _lock: lock,
    }
}

pub(crate) async fn send_request(app: &Router, request: Request<Body>) -> (StatusCode, String) {
    let response = app.clone().oneshot(request).await.expect("app responds");
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body reads");
    (
        status,
        std::str::from_utf8(&body)
            .expect("body is utf-8")
            .to_string(),
    )
}

pub(crate) async fn send_json_request(
    app: &Router,
    method: &str,
    uri: &str,
    body: &str,
) -> (StatusCode, String) {
    send_request(
        app,
        Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_owned()))
            .expect("request builds"),
    )
    .await
}

pub(crate) async fn send_empty_request(
    app: &Router,
    method: &str,
    uri: &str,
) -> (StatusCode, String) {
    send_request(
        app,
        Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .expect("request builds"),
    )
    .await
}

pub(crate) fn extract_json_string_field(response: &str, field: &str) -> String {
    let needle = format!("\"{field}\":\"");
    let start = response
        .find(&needle)
        .map(|index| index + needle.len())
        .expect("field should exist");
    let rest = &response[start..];
    let end = rest.find('"').expect("field should terminate");
    rest[..end].to_string()
}

#[allow(dead_code)]
pub(crate) fn extract_json_u64_field(response: &str, field: &str) -> u64 {
    let needle = format!("\"{field}\":");
    let start = response
        .find(&needle)
        .map(|index| index + needle.len())
        .expect("field should exist");
    let rest = &response[start..];
    let end = rest
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end]
        .parse::<u64>()
        .expect("field should be an unsigned integer")
}

#[allow(dead_code)]
pub(crate) fn extract_sse_event_data<'a>(stream: &'a str, event_name: &str) -> Option<&'a str> {
    let marker = format!("event: {event_name}\n");
    let start = stream.find(&marker)? + marker.len();
    let rest = &stream[start..];
    let data_marker = "data: ";
    let data_start = rest.find(data_marker)? + data_marker.len();
    let data = &rest[data_start..];
    let end = data.find("\n\n").unwrap_or(data.len());
    Some(&data[..end])
}
