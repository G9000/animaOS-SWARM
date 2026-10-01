//! Approvals (spec §7.3): the pending and decided lists, the owner's
//! decision, and each agent's policy and rules. Every route requires the
//! local owner and answers `Cache-Control: no-store`.

use std::collections::HashMap;

use anima_core::primitives::now_millis;
use axum::extract::{Request, State};
use axum::http::{StatusCode, Uri};
use axum::response::Response;

use super::contracts::{ApprovalResponse, ApprovalsEnvelope, ErrorBody};
use super::http::{json_response, request_query};
use super::jobs::{authorize, no_store};
use super::sessions::rejected;
use super::{ApiError, AppState};
use crate::approvals::{
    ApprovalRequest, DECIDED_APPROVAL_WINDOW_MS, DEFAULT_APPROVAL_PAGE, MAX_APPROVAL_PAGE,
};
use crate::history::ApprovalPageQuery;

const STATUS_INVALID: &str = "status must be pending or decided";
const CURSOR_INVALID: &str = "cursor is not valid";
const LIMIT_INVALID: &str = "limit must be between 1 and 100";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListStatus {
    Pending,
    Decided,
}

struct ListQuery {
    status: ListStatus,
    agent_id: Option<String>,
    before: Option<(u64, String)>,
    limit: usize,
}

/// `<createdAtMs>:<id>`: where the next page of decided approvals starts.
fn cursor_of(approval: &ApprovalRequest) -> String {
    format!("{}:{}", approval.created_at_ms, approval.id)
}

fn parse_cursor(cursor: &str) -> Option<(u64, String)> {
    let (at_ms, id) = cursor.split_once(':')?;
    let at_ms = at_ms.parse().ok()?;
    (!id.is_empty()).then(|| (at_ms, id.to_string()))
}

fn list_query(uri: &Uri) -> Result<ListQuery, ApiError> {
    let params =
        request_query(uri).map_err(|()| ApiError::bad_request_static("malformed query"))?;
    let status = match params.get("status").map(String::as_str) {
        None | Some("") | Some("pending") => ListStatus::Pending,
        Some("decided") => ListStatus::Decided,
        Some(_) => return Err(ApiError::bad_request_static(STATUS_INVALID)),
    };
    let before = match params.get("cursor").map(String::as_str) {
        None | Some("") => None,
        Some(cursor) => {
            Some(parse_cursor(cursor).ok_or_else(|| ApiError::bad_request_static(CURSOR_INVALID))?)
        }
    };
    let limit = match params.get("limit").map(String::as_str) {
        None | Some("") => DEFAULT_APPROVAL_PAGE,
        Some(value) => value
            .parse::<usize>()
            .ok()
            .filter(|limit| (1..=MAX_APPROVAL_PAGE).contains(limit))
            .ok_or_else(|| ApiError::bad_request_static(LIMIT_INVALID))?,
    };
    Ok(ListQuery {
        status,
        agent_id: params.get("agentId").filter(|id| !id.is_empty()).cloned(),
        before,
        limit,
    })
}

fn newest_first(left: &ApprovalRequest, right: &ApprovalRequest) -> std::cmp::Ordering {
    (right.created_at_ms, &right.id).cmp(&(left.created_at_ms, &left.id))
}

#[utoipa::path(get, path = "/api/approvals", tag = "approvals",
    params(
        ("status" = Option<String>, Query, description = "`pending` (the default): waiting for the owner, oldest first. `decided`: created in the last 30 days and since decided, newest first"),
        ("agentId" = Option<String>, Query, description = "Only this agent's approvals"),
        ("cursor" = Option<String>, Query, description = "`decided` only: the previous page's `nextCursor`"),
        ("limit" = Option<usize>, Query, description = "`decided` only: 1–100, default 50")
    ),
    responses(
        (status = 200, description = "Approvals, and for `decided` the next page's cursor", body = ApprovalsEnvelope),
        (status = 400, description = "Invalid status, cursor, or limit", body = ErrorBody),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 503, description = "The history store cannot be read", body = ErrorBody)
    ))]
pub(super) async fn list_approvals(State(state): State<AppState>, request: Request) -> Response {
    if let Err(response) = authorize(&state, &request, true) {
        return response;
    }
    let query = match list_query(request.uri()) {
        Ok(query) => query,
        Err(error) => return rejected(error),
    };
    let for_agent = |approval: &ApprovalRequest| {
        query
            .agent_id
            .as_deref()
            .is_none_or(|agent_id| approval.agent_id == agent_id)
    };
    if query.status == ListStatus::Pending {
        let approvals = state
            .daemon
            .read()
            .await
            .approvals
            .pending()
            .into_iter()
            .filter(|approval| for_agent(*approval))
            .map(ApprovalResponse::from)
            .collect();
        return no_store(json_response(
            StatusCode::OK,
            &ApprovalsEnvelope {
                approvals,
                next_cursor: None,
            },
        ));
    }
    let since_ms = now_millis().saturating_sub(DECIDED_APPROVAL_WINDOW_MS);
    let (held, history) = {
        let guard = state.daemon.read().await;
        let held = guard
            .approvals
            .decided()
            .into_iter()
            .filter(|approval| {
                for_agent(*approval)
                    && approval.created_at_ms >= since_ms
                    && query.before.as_ref().is_none_or(|(at_ms, id)| {
                        (approval.created_at_ms, approval.id.as_str()) < (*at_ms, id.as_str())
                    })
            })
            .cloned()
            .collect::<Vec<_>>();
        (held, guard.history.clone())
    };
    // Read outside the state lock (M2 lock rule); one more than a page, so
    // the merge below knows whether another page follows.
    let stored = match history
        .store()
        .page_approvals(&ApprovalPageQuery {
            agent_id: query.agent_id.clone(),
            since_ms,
            before: query.before.clone(),
            limit: query.limit + 1,
        })
        .await
    {
        Ok(stored) => stored,
        Err(error) => return rejected(ApiError::service_unavailable(error.message())),
    };
    // Both may hold an approval just being mirrored: the control plane's
    // copy is the current one.
    let mut merged = stored
        .into_iter()
        .map(|approval| (approval.id.clone(), approval))
        .collect::<HashMap<_, _>>();
    for approval in held {
        merged.insert(approval.id.clone(), approval);
    }
    let mut approvals = merged.into_values().collect::<Vec<_>>();
    approvals.sort_by(newest_first);
    let next_cursor =
        (approvals.len() > query.limit).then(|| cursor_of(&approvals[query.limit - 1]));
    approvals.truncate(query.limit);
    no_store(json_response(
        StatusCode::OK,
        &ApprovalsEnvelope {
            approvals: approvals.iter().map(ApprovalResponse::from).collect(),
            next_cursor,
        },
    ))
}
