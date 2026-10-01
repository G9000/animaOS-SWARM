//! Per-tool approvals (spec §7): every tool's risk class, the owner's
//! policy and rules, and how a call is judged against them. Later M4 tasks
//! add the approval records (`registry`) and the gate in `execute_tool`
//! (`gate`).
#![allow(dead_code)] // M4 Task 8 removes this once the gate and the routes use every item.

pub(crate) mod policy;
pub(crate) mod registry;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[allow(unused_imports)] // M4 Tasks 3, 5, and 8 use the rest.
pub(crate) use policy::{
    evaluate, matcher_kinds, risk_class, suggested_matcher, validate_matcher, Verdict,
};
#[allow(unused_imports)] // M4 Tasks 3–8 use the rest.
pub(crate) use registry::{
    AgentApprovalPolicy, ApprovalDecisionKind, ApprovalRegistry, ApprovalRequest,
    ApprovalResolution, ApprovalSnapshot, ApprovalStatus, PendingApprovalStart, ResolvedBy,
};

/// The tool result of a call the owner's policy denies (spec §7.2).
pub(crate) const DENIED_BY_POLICY: &str = "Denied by owner policy";
/// An owner-supplied matcher whose kind does not fit the tool.
pub(crate) const MATCHER_KIND_NOT_FOR_TOOL: &str = "This matcher kind does not apply to this tool";
/// An owner-supplied matcher value that could never match safely.
pub(crate) const MATCHER_VALUE_INVALID: &str = "matcher value is not valid for its kind";
/// A matcher value's length in characters (plan bound; spec §1 bounded growth).
pub(crate) const MAX_MATCHER_VALUE_CHARS: usize = 512;
/// Rules one companion may keep (plan bound).
pub(crate) const MAX_APPROVAL_RULES_PER_AGENT: usize = 100;
/// "Allow for this session" grants one session may keep (plan bound).
pub(crate) const MAX_SESSION_ALLOWANCES: usize = 50;
/// A request keeps at most this much of its call's arguments (spec §7.3, §16).
pub(crate) const MAX_APPROVAL_ARGUMENTS_BYTES: usize = 16 * 1024;
/// `ApprovalRegistry::add_rule` past `MAX_APPROVAL_RULES_PER_AGENT`.
pub(crate) const TOO_MANY_RULES: &str =
    "This companion already has 100 approval rules; remove one first";
/// The longest owner note (spec §7.3, §16), in characters after trimming.
pub(crate) const MAX_APPROVAL_NOTE_CHARS: usize = 1_000;
pub(crate) const APPROVAL_NOTE_TOO_LONG: &str = "note must be at most 1,000 characters";
/// A timeout's note (spec §7.3): the call's result reads
/// "Denied by owner: Approval timed out".
pub(crate) const APPROVAL_TIMED_OUT: &str = "Approval timed out";
/// The tool result when a run that is not in flight tries to ask.
pub(crate) const APPROVAL_UNAVAILABLE: &str =
    "Needs owner approval, but this run cannot ask for it; the tool did not run";
pub(crate) const TOO_MANY_SESSION_ALLOWANCES: &str = "This session already has 50 allowances";
/// `allow_session` for a session that no longer exists.
pub(crate) const APPROVAL_SESSION_GONE: &str = "This approval's session no longer exists";

/// How much a tool can change (spec §7.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RiskClass {
    Read,
    Write,
    Exec,
    Network,
    Delegate,
}

impl RiskClass {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Exec => "exec",
            Self::Network => "network",
            Self::Delegate => "delegate",
        }
    }
}

/// What a class does under a policy (spec §7.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PolicyAction {
    Allow,
    Ask,
    Deny,
}

impl PolicyAction {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Ask => "ask",
            Self::Deny => "deny",
        }
    }
}

/// One agent's policy (spec §7.2). Read-class tools never ask, so there is
/// no `read` entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalPolicy {
    pub(crate) write: PolicyAction,
    pub(crate) exec: PolicyAction,
    pub(crate) network: PolicyAction,
    pub(crate) delegate: PolicyAction,
}

impl Default for ApprovalPolicy {
    /// `exec: ask`, everything else `allow` (spec §7.2).
    fn default() -> Self {
        Self {
            write: PolicyAction::Allow,
            exec: PolicyAction::Ask,
            network: PolicyAction::Allow,
            delegate: PolicyAction::Allow,
        }
    }
}

impl ApprovalPolicy {
    /// What `class` does; `None` for read-class tools, which never ask.
    pub(crate) const fn action(&self, class: RiskClass) -> Option<PolicyAction> {
        match class {
            RiskClass::Read => None,
            RiskClass::Write => Some(self.write),
            RiskClass::Exec => Some(self.exec),
            RiskClass::Network => Some(self.network),
            RiskClass::Delegate => Some(self.delegate),
        }
    }

    /// This policy with `class` set to `action` (read-class tools are not
    /// set). The routes replace a policy whole; tests build them this way.
    #[cfg(test)]
    pub(crate) fn with(mut self, class: RiskClass, action: PolicyAction) -> Self {
        match class {
            RiskClass::Read => {}
            RiskClass::Write => self.write = action,
            RiskClass::Exec => self.exec = action,
            RiskClass::Network => self.network = action,
            RiskClass::Delegate => self.delegate = action,
        }
        self
    }
}

/// How a rule or allowance picks the calls it covers (spec §7.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MatcherKind {
    CommandPrefix,
    PathGlob,
    Domain,
    Any,
}

impl MatcherKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::CommandPrefix => "command_prefix",
            Self::PathGlob => "path_glob",
            Self::Domain => "domain",
            Self::Any => "any",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalMatcher {
    pub(crate) kind: MatcherKind,
    /// Empty for `any`.
    #[serde(default)]
    pub(crate) value: String,
}

impl ApprovalMatcher {
    /// Every call of the tool.
    pub(crate) fn any() -> Self {
        Self {
            kind: MatcherKind::Any,
            value: String::new(),
        }
    }
}

/// A standing "Always allow" (spec §7.2), managed on the Approvals page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalRule {
    pub(crate) id: String,
    pub(crate) agent_id: String,
    pub(crate) tool: String,
    pub(crate) matcher: ApprovalMatcher,
    pub(crate) created_at_ms: u64,
    /// The approval whose "Always allow" created it; `null` for a rule the
    /// owner added on the Approvals page.
    #[serde(default)]
    pub(crate) from_approval_id: Option<String>,
}

/// An "Allow for this session" grant (spec §3.2, §7.3), kept on its
/// session record and gone with it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionAllowance {
    pub(crate) tool: String,
    pub(crate) matcher: ApprovalMatcher,
    pub(crate) created_at_ms: u64,
    pub(crate) from_approval_id: String,
}
