//! Approval bodies (spec §7.3).

use serde::Serialize;
use utoipa::ToSchema;

use crate::approvals::{
    matcher_kinds, ApprovalMatcher, ApprovalPolicy, ApprovalRequest, ApprovalResolution,
    ApprovalRule,
};

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalMatcherResponse {
    /// `command_prefix`, `path_glob`, `domain`, or `any`.
    pub(crate) kind: String,
    /// Empty for `any`.
    pub(crate) value: String,
}

impl From<&ApprovalMatcher> for ApprovalMatcherResponse {
    fn from(matcher: &ApprovalMatcher) -> Self {
        Self {
            kind: matcher.kind.as_str().into(),
            value: matcher.value.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalResolutionResponse {
    /// `allow_once`, `allow_session`, `allow_always`, or `deny`; `null` for
    /// a stopped or expired request.
    pub(crate) decision: Option<String>,
    pub(crate) note: Option<String>,
    /// The allowance's or rule's matcher.
    pub(crate) matcher: Option<ApprovalMatcherResponse>,
    pub(crate) rule_id: Option<String>,
    /// `owner`, `timeout`, `stop`, or `restart`.
    pub(crate) resolved_by: String,
    pub(crate) resolved_at_ms: u64,
}

impl From<&ApprovalResolution> for ApprovalResolutionResponse {
    fn from(resolution: &ApprovalResolution) -> Self {
        Self {
            decision: resolution.decision.map(|decision| decision.as_str().into()),
            note: resolution.note.clone(),
            matcher: resolution
                .matcher
                .as_ref()
                .map(ApprovalMatcherResponse::from),
            rule_id: resolution.rule_id.clone(),
            resolved_by: resolution.resolved_by.as_str().into(),
            resolved_at_ms: resolution.resolved_at_ms,
        }
    }
}

/// A call waiting for, or decided by, the owner (spec §7.3).
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalResponse {
    pub(crate) id: String,
    pub(crate) agent_id: String,
    pub(crate) session_id: String,
    pub(crate) run_id: String,
    pub(crate) tool_call_id: String,
    pub(crate) tool: String,
    /// `write`, `exec`, `network`, or `delegate`.
    pub(crate) class: String,
    /// The call's arguments as JSON text, at most 16 KiB. The model wrote
    /// them: show them as text, never as markup.
    pub(crate) arguments: String,
    pub(crate) arguments_truncated: bool,
    pub(crate) suggested_matcher: ApprovalMatcherResponse,
    /// The matcher kinds an allowance or rule for this tool may use.
    pub(crate) matcher_kinds: Vec<String>,
    pub(crate) created_at_ms: u64,
    pub(crate) expires_at_ms: u64,
    /// `pending`, `allowed`, `denied`, `stopped`, or `expired`.
    pub(crate) status: String,
    /// Send it back with a decision.
    pub(crate) revision: u64,
    pub(crate) resolution: Option<ApprovalResolutionResponse>,
}

impl From<&ApprovalRequest> for ApprovalResponse {
    fn from(approval: &ApprovalRequest) -> Self {
        Self {
            id: approval.id.clone(),
            agent_id: approval.agent_id.clone(),
            session_id: approval.session_id.clone(),
            run_id: approval.run_id.clone(),
            tool_call_id: approval.tool_call_id.clone(),
            tool: approval.tool.clone(),
            class: approval.class.as_str().into(),
            arguments: approval.arguments.clone(),
            arguments_truncated: approval.arguments_truncated,
            suggested_matcher: ApprovalMatcherResponse::from(&approval.suggested_matcher),
            matcher_kinds: matcher_kinds(&approval.tool)
                .iter()
                .map(|kind| kind.as_str().to_string())
                .collect(),
            created_at_ms: approval.created_at_ms,
            expires_at_ms: approval.expires_at_ms,
            status: approval.status.as_str().into(),
            revision: approval.revision,
            resolution: approval
                .resolution
                .as_ref()
                .map(ApprovalResolutionResponse::from),
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalsEnvelope {
    pub(crate) approvals: Vec<ApprovalResponse>,
    /// `decided` only: pass it as `cursor` for the next, older page.
    pub(crate) next_cursor: Option<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalEnvelope {
    pub(crate) approval: ApprovalResponse,
}

/// What each class does: `allow`, `ask`, or `deny` (spec §7.2).
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalPolicyResponse {
    pub(crate) write: String,
    pub(crate) exec: String,
    pub(crate) network: String,
    pub(crate) delegate: String,
}

impl From<&ApprovalPolicy> for ApprovalPolicyResponse {
    fn from(policy: &ApprovalPolicy) -> Self {
        Self {
            write: policy.write.as_str().into(),
            exec: policy.exec.as_str().into(),
            network: policy.network.as_str().into(),
            delegate: policy.delegate.as_str().into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalPolicyEnvelope {
    pub(crate) policy: ApprovalPolicyResponse,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalRuleResponse {
    pub(crate) id: String,
    pub(crate) agent_id: String,
    pub(crate) tool: String,
    pub(crate) matcher: ApprovalMatcherResponse,
    pub(crate) created_at_ms: u64,
    /// The approval whose "Always allow" created it; `null` for one the
    /// owner added on the Approvals page.
    pub(crate) from_approval_id: Option<String>,
}

impl From<&ApprovalRule> for ApprovalRuleResponse {
    fn from(rule: &ApprovalRule) -> Self {
        Self {
            id: rule.id.clone(),
            agent_id: rule.agent_id.clone(),
            tool: rule.tool.clone(),
            matcher: ApprovalMatcherResponse::from(&rule.matcher),
            created_at_ms: rule.created_at_ms,
            from_approval_id: rule.from_approval_id.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalRuleEnvelope {
    pub(crate) rule: ApprovalRuleResponse,
}

/// A tool a rule can cover.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalToolResponse {
    pub(crate) name: String,
    /// `write`, `exec`, `network`, or `delegate`.
    pub(crate) class: String,
    pub(crate) matcher_kinds: Vec<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalRulesEnvelope {
    pub(crate) rules: Vec<ApprovalRuleResponse>,
    /// Every registered tool that is not read class, by name.
    pub(crate) tools: Vec<ApprovalToolResponse>,
}
