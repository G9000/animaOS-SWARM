//! Approvals (spec §7.3): the pending and decided lists, the owner's
//! decision, and each agent's policy and rules. Every route requires the
//! local owner and answers `Cache-Control: no-store`.

use std::collections::HashMap;

use anima_core::primitives::now_millis;
use axum::extract::{Path, Request, State};
use axum::http::{StatusCode, Uri};
use axum::response::Response;

use super::contracts::{
    ApprovalEnvelope, ApprovalPolicyEnvelope, ApprovalPolicyResponse, ApprovalResponse,
    ApprovalRuleEnvelope, ApprovalRuleResponse, ApprovalRulesEnvelope, ApprovalToolResponse,
    ApprovalsEnvelope, DeleteResponse, ErrorBody,
};
use super::http::{json_response, request_query};
use super::jobs::{authorize, body, no_store};
use super::sessions::rejected;
use super::{ApiError, AppState};
use crate::agent_runs::{config_helper_parent, is_helper_config};
use crate::approvals::{
    matcher_kinds, risk_class, validate_matcher, ApprovalDecisionKind, ApprovalMatcher,
    ApprovalPolicy, ApprovalRequest, ApprovalRule, MatcherKind, PolicyAction, RiskClass,
    DECIDED_APPROVAL_WINDOW_MS, DEFAULT_APPROVAL_PAGE, HELPERS_USE_COMPANION_APPROVALS,
    MAX_APPROVAL_PAGE, READ_TOOLS_NEED_NO_RULE, UNKNOWN_RULE_TOOL,
};
use crate::history::ApprovalPageQuery;
use crate::state::OwnerDecision;
use crate::tools::ToolRegistry;
use serde::Deserialize;
use utoipa::ToSchema;

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

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct MatcherRequest {
    kind: MatcherKind,
    /// Ignored for `any`.
    #[serde(default)]
    value: String,
}

impl From<MatcherRequest> for ApprovalMatcher {
    fn from(matcher: MatcherRequest) -> Self {
        Self {
            kind: matcher.kind,
            value: matcher.value,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DecisionRequest {
    decision: ApprovalDecisionKind,
    /// At most 1,000 characters; a denial's reaches the model.
    #[serde(default)]
    note: Option<String>,
    /// For `allow_session` and `allow_always`; the suggestion when absent.
    #[serde(default)]
    matcher: Option<MatcherRequest>,
    /// The approval's current revision.
    revision: u64,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct PolicyRequest {
    write: PolicyAction,
    exec: PolicyAction,
    network: PolicyAction,
    delegate: PolicyAction,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct RuleRequest {
    tool: String,
    matcher: MatcherRequest,
}

#[utoipa::path(post, path = "/api/approvals/{approval_id}/decision", tag = "approvals",
    params(("approval_id" = String, Path)),
    request_body = DecisionRequest,
    responses(
        (status = 200, description = "The approval as decided; the same decision again returns it unchanged", body = ApprovalEnvelope),
        (status = 400, description = "An invalid body, a note over 1,000 characters, or a matcher that does not fit the tool", body = ErrorBody),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "No such approval", body = ErrorBody),
        (status = 409, description = "Resolved another way already (another decision, the timeout, a Stop, or a restart), a stale revision, a full rule or allowance list, or a session that no longer exists", body = ErrorBody),
        (status = 503, description = "The decision could not be saved, or the history store cannot be read", body = ErrorBody)
    ))]
pub(super) async fn decide_approval(
    State(state): State<AppState>,
    Path(approval_id): Path<String>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, false) {
        return response;
    }
    let input: DecisionRequest = match body(&state, request).await {
        Ok(input) => input,
        Err(response) => return response,
    };
    let decision = OwnerDecision {
        kind: input.decision,
        note: input.note,
        matcher: input.matcher.map(ApprovalMatcher::from),
        revision: input.revision,
    };
    match state
        .agent_runs
        .decide_approval(&approval_id, decision)
        .await
    {
        Ok(approval) => no_store(json_response(
            StatusCode::OK,
            &ApprovalEnvelope {
                approval: ApprovalResponse::from(&approval),
            },
        )),
        Err(error) => rejected(error),
    }
}

#[utoipa::path(get, path = "/api/agents/{agent_id}/approval-policy", tag = "approvals",
    params(("agent_id" = String, Path)),
    responses(
        (status = 200, description = "The agent's policy (a helper's is its companion's)", body = ApprovalPolicyEnvelope),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "Agent not found", body = ErrorBody)
    ))]
pub(super) async fn get_approval_policy(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, true) {
        return response;
    }
    let guard = state.daemon.read().await;
    let Some(runtime) = guard.agents.get(&agent_id) else {
        return rejected(ApiError::not_found());
    };
    let owner = config_helper_parent(runtime.config())
        .unwrap_or(&agent_id)
        .to_string();
    let policy = guard.approvals.policy(&owner);
    no_store(json_response(
        StatusCode::OK,
        &ApprovalPolicyEnvelope {
            policy: ApprovalPolicyResponse::from(&policy),
        },
    ))
}

#[utoipa::path(put, path = "/api/agents/{agent_id}/approval-policy", tag = "approvals",
    params(("agent_id" = String, Path)),
    request_body = PolicyRequest,
    responses(
        (status = 200, description = "The policy, saved", body = ApprovalPolicyEnvelope),
        (status = 400, description = "An invalid body: all four classes are required", body = ErrorBody),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "Agent not found", body = ErrorBody),
        (status = 409, description = "A helper, which uses its companion's policy", body = ErrorBody),
        (status = 503, description = "The policy could not be saved; the old one stays", body = ErrorBody)
    ))]
pub(super) async fn put_approval_policy(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, false) {
        return response;
    }
    let input: PolicyRequest = match body(&state, request).await {
        Ok(input) => input,
        Err(response) => return response,
    };
    let policy = ApprovalPolicy {
        write: input.write,
        exec: input.exec,
        network: input.network,
        delegate: input.delegate,
    };
    let transaction = state.agent_runs.control_plane_transaction().await;
    let (previous, persist) = {
        let mut guard = state.daemon.write().await;
        match guard.agents.get(&agent_id) {
            None => return rejected(ApiError::not_found()),
            Some(runtime) if is_helper_config(runtime.config()) => {
                return rejected(ApiError::conflict(HELPERS_USE_COMPANION_APPROVALS))
            }
            Some(_) => {}
        }
        let previous = guard.approvals.set_policy(&agent_id, policy);
        (previous, guard.control_plane_persist_request())
    };
    if let Err(error) = persist.save().await {
        state
            .daemon
            .write()
            .await
            .approvals
            .restore_policy(&agent_id, previous);
        return rejected(ApiError::service_unavailable(error.to_string()));
    }
    drop(transaction);
    no_store(json_response(
        StatusCode::OK,
        &ApprovalPolicyEnvelope {
            policy: ApprovalPolicyResponse::from(&policy),
        },
    ))
}

/// Every registered tool a rule can cover (all but the read class), by name.
fn rule_tools(registry: &ToolRegistry) -> Vec<ApprovalToolResponse> {
    registry
        .tool_names()
        .into_iter()
        .filter(|name| risk_class(name) != RiskClass::Read)
        .map(|name| ApprovalToolResponse {
            class: risk_class(&name).as_str().into(),
            matcher_kinds: matcher_kinds(&name)
                .iter()
                .map(|kind| kind.as_str().to_string())
                .collect(),
            name,
        })
        .collect()
}

#[utoipa::path(get, path = "/api/agents/{agent_id}/approval-rules", tag = "approvals",
    params(("agent_id" = String, Path)),
    responses(
        (status = 200, description = "The agent's rules, oldest first (a helper's are its companion's), and the tools a rule can cover", body = ApprovalRulesEnvelope),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "Agent not found", body = ErrorBody)
    ))]
pub(super) async fn list_approval_rules(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, true) {
        return response;
    }
    let guard = state.daemon.read().await;
    let Some(runtime) = guard.agents.get(&agent_id) else {
        return rejected(ApiError::not_found());
    };
    let owner = config_helper_parent(runtime.config())
        .unwrap_or(&agent_id)
        .to_string();
    no_store(json_response(
        StatusCode::OK,
        &ApprovalRulesEnvelope {
            rules: guard
                .approvals
                .rules_for(&owner)
                .into_iter()
                .map(ApprovalRuleResponse::from)
                .collect(),
            tools: rule_tools(&guard.tool_registry),
        },
    ))
}

#[utoipa::path(post, path = "/api/agents/{agent_id}/approval-rules", tag = "approvals",
    params(("agent_id" = String, Path)),
    request_body = RuleRequest,
    responses(
        (status = 201, description = "The rule, saved", body = ApprovalRuleEnvelope),
        (status = 200, description = "The agent already had this exact rule", body = ApprovalRuleEnvelope),
        (status = 400, description = "An invalid body, an unknown or read-class tool, or a matcher that does not fit the tool", body = ErrorBody),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "Agent not found", body = ErrorBody),
        (status = 409, description = "A helper, or the agent already has 100 rules", body = ErrorBody),
        (status = 503, description = "The rule could not be saved", body = ErrorBody)
    ))]
pub(super) async fn create_approval_rule(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, false) {
        return response;
    }
    let input: RuleRequest = match body(&state, request).await {
        Ok(input) => input,
        Err(response) => return response,
    };
    let tool = input.tool.trim().to_string();
    let transaction = state.agent_runs.control_plane_transaction().await;
    let (rule, persist) = {
        let mut guard = state.daemon.write().await;
        match guard.agents.get(&agent_id) {
            None => return rejected(ApiError::not_found()),
            Some(runtime) if is_helper_config(runtime.config()) => {
                return rejected(ApiError::conflict(HELPERS_USE_COMPANION_APPROVALS))
            }
            Some(_) => {}
        }
        if guard.tool_registry.lookup(&tool).is_none() {
            return rejected(ApiError::bad_request_static(UNKNOWN_RULE_TOOL));
        }
        if risk_class(&tool) == RiskClass::Read {
            return rejected(ApiError::bad_request_static(READ_TOOLS_NEED_NO_RULE));
        }
        let matcher = match validate_matcher(&tool, &ApprovalMatcher::from(input.matcher)) {
            Ok(matcher) => matcher,
            Err(message) => return rejected(ApiError::bad_request_static(message)),
        };
        if let Some(existing) = guard.approvals.find_rule(&agent_id, &tool, &matcher) {
            return no_store(json_response(
                StatusCode::OK,
                &ApprovalRuleEnvelope {
                    rule: ApprovalRuleResponse::from(existing),
                },
            ));
        }
        let rule = ApprovalRule {
            id: format!("rule_{}", uuid::Uuid::new_v4()),
            agent_id: agent_id.clone(),
            tool,
            matcher,
            created_at_ms: now_millis(),
            from_approval_id: None,
        };
        if let Err(message) = guard.approvals.add_rule(rule.clone()) {
            return rejected(ApiError::conflict(message));
        }
        (rule, guard.control_plane_persist_request())
    };
    if let Err(error) = persist.save().await {
        state
            .daemon
            .write()
            .await
            .approvals
            .remove_rule(&agent_id, &rule.id);
        return rejected(ApiError::service_unavailable(error.to_string()));
    }
    drop(transaction);
    no_store(json_response(
        StatusCode::CREATED,
        &ApprovalRuleEnvelope {
            rule: ApprovalRuleResponse::from(&rule),
        },
    ))
}

#[utoipa::path(delete, path = "/api/agents/{agent_id}/approval-rules/{rule_id}", tag = "approvals",
    params(("agent_id" = String, Path), ("rule_id" = String, Path)),
    responses(
        (status = 200, description = "The rule is gone", body = DeleteResponse),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "Agent or rule not found", body = ErrorBody),
        (status = 503, description = "The removal could not be saved; the rule stays", body = ErrorBody)
    ))]
pub(super) async fn delete_approval_rule(
    State(state): State<AppState>,
    Path((agent_id, rule_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, false) {
        return response;
    }
    let transaction = state.agent_runs.control_plane_transaction().await;
    let (removed, persist) = {
        let mut guard = state.daemon.write().await;
        if !guard.agents.contains_key(&agent_id) {
            return rejected(ApiError::not_found());
        }
        let Some(removed) = guard.approvals.remove_rule(&agent_id, &rule_id) else {
            return rejected(ApiError::not_found());
        };
        (removed, guard.control_plane_persist_request())
    };
    if let Err(error) = persist.save().await {
        // The removal left room, so putting it back cannot hit the cap.
        let _ = state.daemon.write().await.approvals.add_rule(removed);
        return rejected(ApiError::service_unavailable(error.to_string()));
    }
    drop(transaction);
    no_store(json_response(
        StatusCode::OK,
        &DeleteResponse { deleted: true },
    ))
}
