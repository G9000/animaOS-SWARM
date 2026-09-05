use crate::connectors::mail::{MailConnector, MailDraft, MailMessage};
use utoipa::ToSchema;

#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct MailStatusResponse {
    configured: bool,
    connector: Option<MailConnector>,
}
#[derive(serde::Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct MailConnectResponse {
    connector: MailConnector,
    consent_url: String,
}
#[derive(serde::Serialize, ToSchema)]
#[allow(dead_code)]
struct MailMessagesResponse {
    messages: Vec<MailMessage>,
}
#[derive(serde::Serialize, ToSchema)]
#[allow(dead_code)]
struct MailDraftsResponse {
    drafts: Vec<MailDraft>,
}
#[derive(serde::Serialize, ToSchema)]
#[allow(dead_code)]
struct MailDraftResponse {
    draft: MailDraft,
}
use super::{
    http::{json_response, request_query},
    AppState,
};
use crate::connectors::mail::{DraftInput, MailError, Provider};
use axum::{
    extract::{Path, Request, State},
    http::{header, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
};
use serde_json::json;
fn response(status: StatusCode, value: serde_json::Value) -> Response {
    let mut r = json_response(status, &value);
    r.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}
fn error(e: MailError) -> Response {
    response(
        match e {
            MailError::Invalid => StatusCode::BAD_REQUEST,
            MailError::NotFound => StatusCode::NOT_FOUND,
            MailError::Conflict | MailError::Unauthorized => StatusCode::CONFLICT,
            _ => StatusCode::SERVICE_UNAVAILABLE,
        },
        json!({"error":e.to_string()}),
    )
}
fn guard(s: &AppState, r: &Request) -> Result<(), Response> {
    let result = if r.method() == axum::http::Method::GET {
        s.local_owner.authorize_read(r.headers())
    } else {
        s.local_owner.authorize(r.headers())
    };
    result.map_err(|_| {
        response(
            StatusCode::FORBIDDEN,
            json!({"error":"Local owner authorization is required"}),
        )
    })
}
#[utoipa::path(
 get, path = "/api/agents/{agent_id}/connectors/mail/{provider}", tag = "connectors", 
 params(("agent_id" = String, Path, description = "Agent identifier"),("provider" = String, Path, description = "gmail or outlook")),
 responses((status = 200, description = "Mail operation result", body = MailStatusResponse),
 (status = 400, description = "Invalid mail request", body = super::contracts::ErrorBody),
 (status = 403, description = "Local owner authorization required", body = super::contracts::ErrorBody),
 (status = 404, description = "Agent, connector or draft not found", body = super::contracts::ErrorBody),
 (status = 409, description = "Operation unavailable or reauthorization required", body = super::contracts::ErrorBody),
 (status = 503, description = "Provider, credentials or persistence unavailable", body = super::contracts::ErrorBody))
)]
pub(super) async fn status(
    State(s): State<AppState>,
    Path((agent, provider)): Path<(String, String)>,
    r: Request,
) -> Response {
    if let Err(e) = guard(&s, &r) {
        return e;
    }
    let p = match Provider::parse(&provider) {
        Ok(p) => p,
        Err(e) => return error(e),
    };
    match s.mail.connector_for_agent(&agent, p).await {
        Ok(c) => response(
            StatusCode::OK,
            json!({"configured":s.mail.configured(p).await.unwrap_or(false),"connector":c}),
        ),
        Err(e) => error(e),
    }
}
#[utoipa::path(
 post, path = "/api/agents/{agent_id}/connectors/mail/{provider}", tag = "connectors", 
 params(("agent_id" = String, Path, description = "Agent identifier"),("provider" = String, Path, description = "gmail or outlook")),
 responses((status = 201, description = "Mail operation result", body = MailConnectResponse),
 (status = 400, description = "Invalid mail request", body = super::contracts::ErrorBody),
 (status = 403, description = "Local owner authorization required", body = super::contracts::ErrorBody),
 (status = 404, description = "Agent, connector or draft not found", body = super::contracts::ErrorBody),
 (status = 409, description = "Operation unavailable or reauthorization required", body = super::contracts::ErrorBody),
 (status = 503, description = "Provider, credentials or persistence unavailable", body = super::contracts::ErrorBody))
)]
pub(super) async fn connect(
    State(s): State<AppState>,
    Path((agent, provider)): Path<(String, String)>,
    r: Request,
) -> Response {
    if let Err(e) = guard(&s, &r) {
        return e;
    }
    let p = match Provider::parse(&provider) {
        Ok(p) => p,
        Err(e) => return error(e),
    };
    match s.mail.begin_connect(&agent, p).await {
        Ok((c, url)) => response(StatusCode::CREATED, json!({"connector":c,"consentUrl":url})),
        Err(e) => error(e),
    }
}
#[utoipa::path(
 delete, path = "/api/agents/{agent_id}/connectors/mail/{provider}/{connector_id}", tag = "connectors", 
 params(("agent_id" = String, Path, description = "Agent identifier"),("provider" = String, Path, description = "gmail or outlook"),("connector_id" = String, Path, description = "Mail connection identifier")),
 responses((status = 200, description = "Mail operation result", body = super::contracts::DeleteResponse),
 (status = 400, description = "Invalid mail request", body = super::contracts::ErrorBody),
 (status = 403, description = "Local owner authorization required", body = super::contracts::ErrorBody),
 (status = 404, description = "Agent, connector or draft not found", body = super::contracts::ErrorBody),
 (status = 409, description = "Operation unavailable or reauthorization required", body = super::contracts::ErrorBody),
 (status = 503, description = "Provider, credentials or persistence unavailable", body = super::contracts::ErrorBody))
)]
pub(super) async fn disconnect(
    State(s): State<AppState>,
    Path((agent, provider, id)): Path<(String, String, String)>,
    r: Request,
) -> Response {
    if let Err(e) = guard(&s, &r) {
        return e;
    }
    let p = match Provider::parse(&provider) {
        Ok(p) => p,
        Err(e) => return error(e),
    };
    match s.mail.disconnect(&agent, &id, p).await {
        Ok(()) => response(StatusCode::OK, json!({"deleted":true})),
        Err(e) => error(e),
    }
}
#[utoipa::path(
 get, path = "/api/agents/{agent_id}/connectors/mail/{provider}/{connector_id}/messages", tag = "connectors", 
 params(("agent_id" = String, Path, description = "Agent identifier"),("provider" = String, Path, description = "gmail or outlook"),("connector_id" = String, Path, description = "Mail connection identifier"),("refresh" = Option<bool>, Query, description = "Refresh provider inbox before returning")),
 responses((status = 200, description = "Mail operation result", body = MailMessagesResponse),
 (status = 400, description = "Invalid mail request", body = super::contracts::ErrorBody),
 (status = 403, description = "Local owner authorization required", body = super::contracts::ErrorBody),
 (status = 404, description = "Agent, connector or draft not found", body = super::contracts::ErrorBody),
 (status = 409, description = "Operation unavailable or reauthorization required", body = super::contracts::ErrorBody),
 (status = 503, description = "Provider, credentials or persistence unavailable", body = super::contracts::ErrorBody))
)]
pub(super) async fn messages(
    State(s): State<AppState>,
    Path((agent, provider, id)): Path<(String, String, String)>,
    r: Request,
) -> Response {
    if let Err(e) = guard(&s, &r) {
        return e;
    }
    let p = match Provider::parse(&provider) {
        Ok(p) => p,
        Err(e) => return error(e),
    };
    let refresh = match request_query(r.uri()) {
        Ok(q) => q.get("refresh").is_some_and(|s| s == "true"),
        Err(_) => return error(MailError::Invalid),
    };
    match s.mail.messages(&agent, &id, p, refresh).await {
        Ok(m) => response(StatusCode::OK, json!({"messages":m})),
        Err(e) => error(e),
    }
}
#[utoipa::path(
 get, path = "/api/agents/{agent_id}/connectors/mail/{provider}/{connector_id}/drafts", tag = "connectors", 
 params(("agent_id" = String, Path, description = "Agent identifier"),("provider" = String, Path, description = "gmail or outlook"),("connector_id" = String, Path, description = "Mail connection identifier")),
 responses((status = 200, description = "Mail operation result", body = MailDraftsResponse),
 (status = 400, description = "Invalid mail request", body = super::contracts::ErrorBody),
 (status = 403, description = "Local owner authorization required", body = super::contracts::ErrorBody),
 (status = 404, description = "Agent, connector or draft not found", body = super::contracts::ErrorBody),
 (status = 409, description = "Operation unavailable or reauthorization required", body = super::contracts::ErrorBody),
 (status = 503, description = "Provider, credentials or persistence unavailable", body = super::contracts::ErrorBody))
)]
pub(super) async fn drafts(
    State(s): State<AppState>,
    Path((agent, provider, id)): Path<(String, String, String)>,
    r: Request,
) -> Response {
    if let Err(e) = guard(&s, &r) {
        return e;
    }
    let p = match Provider::parse(&provider) {
        Ok(p) => p,
        Err(e) => return error(e),
    };
    match s.mail.drafts(&agent, &id, p).await {
        Ok(d) => response(StatusCode::OK, json!({"drafts":d})),
        Err(e) => error(e),
    }
}
#[utoipa::path(
 post, path = "/api/agents/{agent_id}/connectors/mail/{provider}/{connector_id}/drafts", tag = "connectors", request_body = DraftInput,
 params(("agent_id" = String, Path, description = "Agent identifier"),("provider" = String, Path, description = "gmail or outlook"),("connector_id" = String, Path, description = "Mail connection identifier")),
 responses((status = 201, description = "Mail operation result", body = MailDraftResponse),
 (status = 400, description = "Invalid mail request", body = super::contracts::ErrorBody),
 (status = 403, description = "Local owner authorization required", body = super::contracts::ErrorBody),
 (status = 404, description = "Agent, connector or draft not found", body = super::contracts::ErrorBody),
 (status = 409, description = "Operation unavailable or reauthorization required", body = super::contracts::ErrorBody),
 (status = 503, description = "Provider, credentials or persistence unavailable", body = super::contracts::ErrorBody))
)]
pub(super) async fn create_draft(
    State(s): State<AppState>,
    Path((agent, provider, id)): Path<(String, String, String)>,
    r: Request,
) -> Response {
    if let Err(e) = guard(&s, &r) {
        return e;
    }
    let p = match Provider::parse(&provider) {
        Ok(p) => p,
        Err(e) => return error(e),
    };
    let bytes = match axum::body::to_bytes(r.into_body(), 110_000).await {
        Ok(b) => b,
        Err(_) => return error(MailError::Invalid),
    };
    let input = match serde_json::from_slice::<DraftInput>(&bytes) {
        Ok(v) => v,
        Err(_) => return error(MailError::Invalid),
    };
    match s.mail.create_draft(&agent, &id, p, input).await {
        Ok(d) => response(StatusCode::CREATED, json!({"draft":d})),
        Err(e) => error(e),
    }
}
#[utoipa::path(
 post, path = "/api/agents/{agent_id}/connectors/mail/{provider}/{connector_id}/drafts/{draft_id}/approve", tag = "connectors", 
 params(("agent_id" = String, Path, description = "Agent identifier"),("provider" = String, Path, description = "gmail or outlook"),("connector_id" = String, Path, description = "Mail connection identifier"),("draft_id" = String, Path, description = "Immutable local draft identifier")),
 responses((status = 200, description = "Mail operation result", body = MailDraftResponse),
 (status = 400, description = "Invalid mail request", body = super::contracts::ErrorBody),
 (status = 403, description = "Local owner authorization required", body = super::contracts::ErrorBody),
 (status = 404, description = "Agent, connector or draft not found", body = super::contracts::ErrorBody),
 (status = 409, description = "Operation unavailable or reauthorization required", body = super::contracts::ErrorBody),
 (status = 503, description = "Provider, credentials or persistence unavailable", body = super::contracts::ErrorBody))
)]
pub(super) async fn approve(
    State(s): State<AppState>,
    Path((agent, provider, id, draft)): Path<(String, String, String, String)>,
    r: Request,
) -> Response {
    resolve(s, agent, provider, id, draft, r, true).await
}
#[utoipa::path(
 post, path = "/api/agents/{agent_id}/connectors/mail/{provider}/{connector_id}/drafts/{draft_id}/reject", tag = "connectors", 
 params(("agent_id" = String, Path, description = "Agent identifier"),("provider" = String, Path, description = "gmail or outlook"),("connector_id" = String, Path, description = "Mail connection identifier"),("draft_id" = String, Path, description = "Immutable local draft identifier")),
 responses((status = 200, description = "Mail operation result", body = MailDraftResponse),
 (status = 400, description = "Invalid mail request", body = super::contracts::ErrorBody),
 (status = 403, description = "Local owner authorization required", body = super::contracts::ErrorBody),
 (status = 404, description = "Agent, connector or draft not found", body = super::contracts::ErrorBody),
 (status = 409, description = "Operation unavailable or reauthorization required", body = super::contracts::ErrorBody),
 (status = 503, description = "Provider, credentials or persistence unavailable", body = super::contracts::ErrorBody))
)]
pub(super) async fn reject(
    State(s): State<AppState>,
    Path((agent, provider, id, draft)): Path<(String, String, String, String)>,
    r: Request,
) -> Response {
    resolve(s, agent, provider, id, draft, r, false).await
}
async fn resolve(
    s: AppState,
    agent: String,
    provider: String,
    id: String,
    draft: String,
    r: Request,
    approve: bool,
) -> Response {
    if let Err(e) = guard(&s, &r) {
        return e;
    }
    let p = match Provider::parse(&provider) {
        Ok(p) => p,
        Err(e) => return error(e),
    };
    match s.mail.resolve(&agent, &id, p, &draft, approve).await {
        Ok(d) => response(StatusCode::OK, json!({"draft":d})),
        Err(e) => error(e),
    }
}
#[utoipa::path(get, path = "/api/connectors/mail/{provider}/callback", tag = "connectors",
 params(("provider" = String, Path, description = "gmail or outlook")),
 responses((status = 200, description = "OAuth connection completed"), (status = 400, description = "Invalid, expired or unsuccessful OAuth callback")))]
pub(super) async fn callback(
    State(s): State<AppState>,
    Path(provider): Path<String>,
    uri: axum::http::Uri,
) -> Response {
    let p = match Provider::parse(&provider) {
        Ok(p) => p,
        Err(e) => return error(e),
    };
    let query = match request_query(&uri) {
        Ok(q) => q,
        Err(_) => return error(MailError::Invalid),
    };
    let code = query.get("code").map(String::as_str).unwrap_or("");
    let nonce = query.get("state").map(String::as_str).unwrap_or("");
    let result = s.mail.complete_connect(p, nonce, code).await;
    let (status, message) = match result {
        Ok(_) => (
            StatusCode::OK,
            "Mail connected. Close this tab and return to Connectors.",
        ),
        Err(_) => (
            StatusCode::BAD_REQUEST,
            "Authorization could not be completed. Reconnect from the Connectors tab.",
        ),
    };
    let mut r = (
        status,
        Html(format!(
            "<!doctype html><html><body><p>{message}</p></body></html>"
        )),
    )
        .into_response();
    r.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    r
}
