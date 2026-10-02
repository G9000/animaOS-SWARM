//! The approval records the control plane keeps (spec §7.3, §13.1): every
//! pending request, decided ones until the history store holds them, and
//! each agent's policy and rules.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use anima_core::{DataValue, ToolCall};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{
    risk_class, suggested_matcher, ApprovalMatcher, ApprovalPolicy, ApprovalRule, RiskClass,
    MAX_APPROVAL_ARGUMENTS_BYTES, MAX_APPROVAL_RULES_PER_AGENT, TOO_MANY_RULES,
};

/// Where an approval stands (spec §7.3). A timeout is `denied` by
/// `timeout`; stopping the run resolves it `stopped`; a restart `expired`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ApprovalStatus {
    Pending,
    Allowed,
    Denied,
    Stopped,
    Expired,
}

impl ApprovalStatus {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Allowed => "allowed",
            Self::Denied => "denied",
            Self::Stopped => "stopped",
            Self::Expired => "expired",
        }
    }
}

/// The owner's four decisions (spec §7.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ApprovalDecisionKind {
    AllowOnce,
    AllowSession,
    AllowAlways,
    Deny,
}

impl ApprovalDecisionKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::AllowOnce => "allow_once",
            Self::AllowSession => "allow_session",
            Self::AllowAlways => "allow_always",
            Self::Deny => "deny",
        }
    }
}

/// Who settled an approval.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ResolvedBy {
    Owner,
    Timeout,
    Stop,
    Restart,
}

impl ResolvedBy {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Timeout => "timeout",
            Self::Stop => "stop",
            Self::Restart => "restart",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalResolution {
    /// `None` for a stopped or expired request.
    #[serde(default)]
    pub(crate) decision: Option<ApprovalDecisionKind>,
    #[serde(default)]
    pub(crate) note: Option<String>,
    /// The allowance's or rule's matcher, for `allow_session` and `allow_always`.
    #[serde(default)]
    pub(crate) matcher: Option<ApprovalMatcher>,
    /// The rule `allow_always` created or found.
    #[serde(default)]
    pub(crate) rule_id: Option<String>,
    pub(crate) resolved_by: ResolvedBy,
    pub(crate) resolved_at_ms: u64,
}

/// One call waiting for, or decided by, the owner (spec §7.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalRequest {
    pub(crate) id: String,
    pub(crate) agent_id: String,
    pub(crate) session_id: String,
    pub(crate) run_id: String,
    pub(crate) tool_call_id: String,
    pub(crate) tool: String,
    pub(crate) class: RiskClass,
    /// The call's arguments as JSON text, cut to 16 KiB. The model wrote
    /// them: they are untrusted text to the owner.
    pub(crate) arguments: String,
    #[serde(default)]
    pub(crate) arguments_truncated: bool,
    pub(crate) suggested_matcher: ApprovalMatcher,
    pub(crate) created_at_ms: u64,
    pub(crate) expires_at_ms: u64,
    pub(crate) status: ApprovalStatus,
    /// 1 while pending; a resolution adds 1. A decision must carry it.
    pub(crate) revision: u64,
    #[serde(default)]
    pub(crate) resolution: Option<ApprovalResolution>,
}

/// What a new request is about.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PendingApprovalStart<'a> {
    pub(crate) agent_id: &'a str,
    pub(crate) session_id: &'a str,
    pub(crate) run_id: &'a str,
    pub(crate) call: &'a ToolCall,
    pub(crate) timeout_ms: u64,
}

/// `call`'s arguments as JSON text, cut to `MAX_APPROVAL_ARGUMENTS_BYTES` on
/// a char boundary, and whether they were cut.
pub(crate) fn bounded_arguments(call: &ToolCall) -> (String, bool) {
    let text = crate::routes::data_value_to_json(&DataValue::Object(call.args.clone())).to_string();
    if text.len() <= MAX_APPROVAL_ARGUMENTS_BYTES {
        return (text, false);
    }
    let mut end = MAX_APPROVAL_ARGUMENTS_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true)
}

impl ApprovalRequest {
    pub(crate) fn pending(start: PendingApprovalStart<'_>, now_ms: u64) -> Self {
        let (arguments, arguments_truncated) = bounded_arguments(start.call);
        Self {
            id: format!("apr_{}", uuid::Uuid::new_v4()),
            agent_id: start.agent_id.to_string(),
            session_id: start.session_id.to_string(),
            run_id: start.run_id.to_string(),
            tool_call_id: start.call.id.clone(),
            tool: start.call.name.clone(),
            class: risk_class(&start.call.name),
            arguments,
            arguments_truncated,
            suggested_matcher: suggested_matcher(start.call),
            created_at_ms: now_ms,
            expires_at_ms: now_ms.saturating_add(start.timeout_ms),
            status: ApprovalStatus::Pending,
            revision: 1,
            resolution: None,
        }
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.status == ApprovalStatus::Pending
    }

    /// Resolves this request; its revision moves on.
    pub(crate) fn resolve(&mut self, status: ApprovalStatus, resolution: ApprovalResolution) {
        self.status = status;
        self.resolution = Some(resolution);
        self.revision += 1;
    }

    /// Whether the owner decided it as `kind` (an idempotent replay).
    pub(crate) fn was_decided_as(&self, kind: ApprovalDecisionKind) -> bool {
        self.resolution.as_ref().is_some_and(|resolution| {
            resolution.resolved_by == ResolvedBy::Owner && resolution.decision == Some(kind)
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentApprovalPolicy {
    pub(crate) agent_id: String,
    pub(crate) policy: ApprovalPolicy,
}

/// The registry as the control-plane snapshot stores it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ApprovalSnapshot {
    pub(crate) approvals: Vec<ApprovalRequest>,
    pub(crate) policies: Vec<AgentApprovalPolicy>,
    pub(crate) rules: Vec<ApprovalRule>,
}

fn oldest_first(left: &&ApprovalRequest, right: &&ApprovalRequest) -> Ordering {
    (left.created_at_ms, &left.id).cmp(&(right.created_at_ms, &right.id))
}

fn resolved_at(approval: &ApprovalRequest) -> u64 {
    approval
        .resolution
        .as_ref()
        .map_or(approval.created_at_ms, |resolution| {
            resolution.resolved_at_ms
        })
}

/// Pending approvals, decided ones not yet in the history store, and each
/// agent's policy and rules (spec §2's control plane).
#[derive(Clone, Debug, Default)]
pub(crate) struct ApprovalRegistry {
    approvals: HashMap<String, ApprovalRequest>,
    policies: HashMap<String, ApprovalPolicy>,
    rules: HashMap<String, ApprovalRule>,
}

impl ApprovalRegistry {
    pub(crate) fn get(&self, id: &str) -> Option<&ApprovalRequest> {
        self.approvals.get(id)
    }

    pub(crate) fn get_mut(&mut self, id: &str) -> Option<&mut ApprovalRequest> {
        self.approvals.get_mut(id)
    }

    pub(crate) fn insert(&mut self, approval: ApprovalRequest) {
        self.approvals.insert(approval.id.clone(), approval);
    }

    pub(crate) fn remove(&mut self, id: &str) -> Option<ApprovalRequest> {
        self.approvals.remove(id)
    }

    /// Pending approvals, oldest first.
    pub(crate) fn pending(&self) -> Vec<&ApprovalRequest> {
        let mut pending = self
            .approvals
            .values()
            .filter(|approval| approval.is_pending())
            .collect::<Vec<_>>();
        pending.sort_by(oldest_first);
        pending
    }

    /// The pending approvals of one run, oldest first.
    pub(crate) fn pending_ids_for_run(&self, run_id: &str) -> Vec<String> {
        self.pending()
            .into_iter()
            .filter(|approval| approval.run_id == run_id)
            .map(|approval| approval.id.clone())
            .collect()
    }

    pub(crate) fn pending_count_for_session(&self, agent_id: &str, session_id: &str) -> usize {
        self.approvals
            .values()
            .filter(|approval| {
                approval.is_pending()
                    && approval.agent_id == agent_id
                    && approval.session_id == session_id
            })
            .count()
    }

    /// Decided approvals the control plane still holds, newest first.
    pub(crate) fn decided(&self) -> Vec<&ApprovalRequest> {
        let mut decided = self
            .approvals
            .values()
            .filter(|approval| !approval.is_pending())
            .collect::<Vec<_>>();
        decided.sort_by(|left, right| oldest_first(right, left));
        decided
    }

    /// `agent_id`'s policy, or the default (spec §7.2).
    pub(crate) fn policy(&self, agent_id: &str) -> ApprovalPolicy {
        self.policies.get(agent_id).copied().unwrap_or_default()
    }

    /// Replaces `agent_id`'s policy; returns the stored one it replaced.
    pub(crate) fn set_policy(
        &mut self,
        agent_id: &str,
        policy: ApprovalPolicy,
    ) -> Option<ApprovalPolicy> {
        self.policies.insert(agent_id.to_string(), policy)
    }

    /// Puts back what `set_policy` replaced, after its save failed.
    pub(crate) fn restore_policy(&mut self, agent_id: &str, previous: Option<ApprovalPolicy>) {
        match previous {
            Some(policy) => {
                self.policies.insert(agent_id.to_string(), policy);
            }
            None => {
                self.policies.remove(agent_id);
            }
        }
    }

    /// `agent_id`'s rules, oldest first.
    pub(crate) fn rules_for(&self, agent_id: &str) -> Vec<&ApprovalRule> {
        let mut rules = self
            .rules
            .values()
            .filter(|rule| rule.agent_id == agent_id)
            .collect::<Vec<_>>();
        rules.sort_by(|left, right| {
            (left.created_at_ms, &left.id).cmp(&(right.created_at_ms, &right.id))
        });
        rules
    }

    /// `agent_id`'s rule for exactly this tool and matcher.
    pub(crate) fn find_rule(
        &self,
        agent_id: &str,
        tool: &str,
        matcher: &ApprovalMatcher,
    ) -> Option<&ApprovalRule> {
        self.rules
            .values()
            .find(|rule| rule.agent_id == agent_id && rule.tool == tool && rule.matcher == *matcher)
    }

    pub(crate) fn add_rule(&mut self, rule: ApprovalRule) -> Result<(), &'static str> {
        let held = self
            .rules
            .values()
            .filter(|existing| existing.agent_id == rule.agent_id)
            .count();
        if held >= MAX_APPROVAL_RULES_PER_AGENT {
            return Err(TOO_MANY_RULES);
        }
        self.rules.insert(rule.id.clone(), rule);
        Ok(())
    }

    /// Removes `rule_id` if it is `agent_id`'s.
    pub(crate) fn remove_rule(&mut self, agent_id: &str, rule_id: &str) -> Option<ApprovalRule> {
        if self
            .rules
            .get(rule_id)
            .is_some_and(|rule| rule.agent_id == agent_id)
        {
            self.rules.remove(rule_id)
        } else {
            None
        }
    }

    /// Drops the decided approvals `keep` refuses (those of deleted agents
    /// or sessions); pending ones always stay. Returns how many went.
    pub(crate) fn retain_decided(&mut self, keep: impl Fn(&ApprovalRequest) -> bool) -> usize {
        let before = self.approvals.len();
        self.approvals
            .retain(|_, approval| approval.is_pending() || keep(approval));
        before - self.approvals.len()
    }

    /// Decided approvals for the history store, the `limit` oldest
    /// resolutions first.
    pub(crate) fn unmirrored_decided(&self, limit: usize) -> Vec<ApprovalRequest> {
        let mut decided = self
            .approvals
            .values()
            .filter(|approval| !approval.is_pending())
            .collect::<Vec<_>>();
        decided.sort_by(|left, right| {
            (resolved_at(left), &left.id).cmp(&(resolved_at(right), &right.id))
        });
        decided.into_iter().take(limit).cloned().collect()
    }

    /// Removes each written approval the registry still holds unchanged: the
    /// history store holds it now (spec §7.3 "moves the record"). A record
    /// that changed since is written again by the next flush.
    pub(crate) fn mark_mirrored(&mut self, written: &[ApprovalRequest]) -> usize {
        let mut removed = 0;
        for approval in written {
            if self
                .approvals
                .get(&approval.id)
                .is_some_and(|current| !current.is_pending() && current == approval)
            {
                self.approvals.remove(&approval.id);
                removed += 1;
            }
        }
        removed
    }

    /// What the control plane saves: nothing of agents that no longer exist,
    /// and no decided approval `keep` refuses.
    pub(crate) fn snapshot(
        &self,
        live_agents: &HashSet<String>,
        keep: impl Fn(&ApprovalRequest) -> bool,
    ) -> ApprovalSnapshot {
        let mut approvals = self
            .approvals
            .values()
            .filter(|approval| {
                live_agents.contains(&approval.agent_id)
                    && (approval.is_pending() || keep(approval))
            })
            .collect::<Vec<_>>();
        approvals.sort_by(oldest_first);
        let mut policies = self
            .policies
            .iter()
            .filter(|(agent_id, _)| live_agents.contains(*agent_id))
            .map(|(agent_id, policy)| AgentApprovalPolicy {
                agent_id: agent_id.clone(),
                policy: *policy,
            })
            .collect::<Vec<_>>();
        policies.sort_by(|left, right| left.agent_id.cmp(&right.agent_id));
        let mut rules = self
            .rules
            .values()
            .filter(|rule| live_agents.contains(&rule.agent_id))
            .cloned()
            .collect::<Vec<_>>();
        rules.sort_by(|left, right| {
            (&left.agent_id, left.created_at_ms, &left.id).cmp(&(
                &right.agent_id,
                right.created_at_ms,
                &right.id,
            ))
        });
        ApprovalSnapshot {
            approvals: approvals.into_iter().cloned().collect(),
            policies,
            rules,
        }
    }

    pub(crate) fn validate(
        approvals: &[ApprovalRequest],
        policies: &[AgentApprovalPolicy],
        rules: &[ApprovalRule],
    ) -> Result<(), String> {
        let mut ids = HashSet::new();
        for approval in approvals {
            if approval.id.trim().is_empty() || !ids.insert(approval.id.as_str()) {
                return Err(format!(
                    "duplicate or empty approval id in snapshot: {}",
                    approval.id
                ));
            }
            if approval.agent_id.trim().is_empty()
                || approval.session_id.trim().is_empty()
                || approval.run_id.trim().is_empty()
            {
                return Err(format!(
                    "approval '{}' has an empty agent, session, or run id",
                    approval.id
                ));
            }
            if approval.revision == 0 || approval.is_pending() != approval.resolution.is_none() {
                return Err(format!(
                    "approval '{}' has a revision or resolution that does not fit its status",
                    approval.id
                ));
            }
        }
        let mut agents = HashSet::new();
        for entry in policies {
            if entry.agent_id.trim().is_empty() || !agents.insert(entry.agent_id.as_str()) {
                return Err(format!(
                    "duplicate or empty approval policy agent in snapshot: {}",
                    entry.agent_id
                ));
            }
        }
        let mut rule_ids = HashSet::new();
        for rule in rules {
            if rule.agent_id.trim().is_empty() {
                return Err(format!("approval rule '{}' has an empty agent id", rule.id));
            }
            if rule.id.trim().is_empty() || !rule_ids.insert(rule.id.as_str()) {
                return Err(format!(
                    "duplicate or empty approval rule id in snapshot: {}",
                    rule.id
                ));
            }
        }
        Ok(())
    }

    /// The registry after a restart (spec §4.8): every pending approval is
    /// `expired`, since no call waits on it any more, and the records of
    /// agents that no longer exist are dropped.
    pub(crate) fn restored(
        snapshot: ApprovalSnapshot,
        live_agents: &HashSet<String>,
        now_ms: u64,
    ) -> Self {
        let mut registry = Self::default();
        for mut approval in snapshot.approvals {
            if !live_agents.contains(&approval.agent_id) {
                continue;
            }
            if approval.is_pending() {
                approval.resolve(
                    ApprovalStatus::Expired,
                    ApprovalResolution {
                        decision: None,
                        note: None,
                        matcher: None,
                        rule_id: None,
                        resolved_by: ResolvedBy::Restart,
                        resolved_at_ms: now_ms,
                    },
                );
            }
            registry.insert(approval);
        }
        for entry in snapshot.policies {
            if live_agents.contains(&entry.agent_id) {
                registry.policies.insert(entry.agent_id, entry.policy);
            }
        }
        for rule in snapshot.rules {
            if live_agents.contains(&rule.agent_id) {
                registry.rules.insert(rule.id.clone(), rule);
            }
        }
        registry
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashSet};

    use anima_core::{AgentConfig, AgentSettings, DataValue, ToolCall};

    use super::*;
    use crate::approvals::{
        ApprovalMatcher, ApprovalPolicy, ApprovalRule, MatcherKind, PolicyAction, RiskClass,
        SessionAllowance, MAX_APPROVAL_ARGUMENTS_BYTES, MAX_APPROVAL_RULES_PER_AGENT,
        TOO_MANY_RULES,
    };
    use crate::runs::{RunRecord, RunSource, RunStart, RunStatus, RESTART_DURING_RUN};
    use crate::sessions::{SessionKind, SessionOrigin, SessionRecord, TitleSource};
    use crate::state::DaemonState;

    fn remember(text: &str) -> ToolCall {
        ToolCall {
            id: "call-1".into(),
            name: "memory_add".into(),
            args: BTreeMap::from([("content".to_string(), DataValue::String(text.into()))]),
        }
    }

    fn request(id: &str, agent: &str, session: &str, run: &str, at_ms: u64) -> ApprovalRequest {
        let call = remember("the plan");
        let mut approval = ApprovalRequest::pending(
            PendingApprovalStart {
                agent_id: agent,
                session_id: session,
                run_id: run,
                call: &call,
                timeout_ms: 1_000,
            },
            at_ms,
        );
        approval.id = id.into();
        approval
    }

    fn resolution(by: ResolvedBy, at_ms: u64) -> ApprovalResolution {
        ApprovalResolution {
            decision: Some(ApprovalDecisionKind::AllowOnce),
            note: None,
            matcher: None,
            rule_id: None,
            resolved_by: by,
            resolved_at_ms: at_ms,
        }
    }

    fn decided(id: &str, agent: &str, session: &str, at_ms: u64) -> ApprovalRequest {
        let mut approval = request(id, agent, session, "run_1", at_ms);
        approval.resolve(
            ApprovalStatus::Allowed,
            resolution(ResolvedBy::Owner, at_ms + 1),
        );
        approval
    }

    fn rule(id: &str, agent: &str, at_ms: u64) -> ApprovalRule {
        ApprovalRule {
            id: id.into(),
            agent_id: agent.into(),
            tool: "memory_add".into(),
            matcher: ApprovalMatcher::any(),
            created_at_ms: at_ms,
            from_approval_id: None,
        }
    }

    fn config(name: &str) -> AgentConfig {
        AgentConfig {
            name: name.into(),
            model: "deterministic".into(),
            bio: None,
            lore: None,
            knowledge: None,
            topics: None,
            adjectives: None,
            style: None,
            provider: None,
            system: None,
            tools: None,
            plugins: None,
            settings: Some(AgentSettings::default()),
        }
    }

    #[test]
    fn a_pending_request_bounds_its_arguments_and_suggests_a_matcher() {
        let call = ToolCall {
            id: "call-7".into(),
            name: "bash".into(),
            args: BTreeMap::from([(
                "command".to_string(),
                DataValue::String(format!("git status {}", "é".repeat(12_000))),
            )]),
        };
        let approval = ApprovalRequest::pending(
            PendingApprovalStart {
                agent_id: "agent-1",
                session_id: "chat:a",
                run_id: "run_1",
                call: &call,
                timeout_ms: 60_000,
            },
            1_000,
        );

        assert!(approval.id.starts_with("apr_"));
        assert_eq!(approval.tool_call_id, "call-7");
        assert_eq!(approval.class, RiskClass::Exec);
        assert!(approval.arguments.len() <= MAX_APPROVAL_ARGUMENTS_BYTES);
        assert!(approval.arguments_truncated);
        assert!(approval.arguments.starts_with("{\"command\":\"git status "));
        assert_eq!(
            approval.suggested_matcher,
            ApprovalMatcher {
                kind: MatcherKind::CommandPrefix,
                value: "git status".into()
            }
        );
        assert_eq!(approval.expires_at_ms, 61_000);
        assert_eq!(
            (approval.status, approval.revision),
            (ApprovalStatus::Pending, 1)
        );
        let (small, cut) = bounded_arguments(&remember("short"));
        assert_eq!(small, "{\"content\":\"short\"}");
        assert!(!cut);
    }

    #[test]
    fn pending_approvals_are_listed_oldest_first_by_run_and_by_session() {
        let mut registry = ApprovalRegistry::default();
        registry.insert(request("apr_b", "agent-1", "chat:a", "run_1", 20));
        registry.insert(request("apr_a", "agent-1", "chat:a", "run_1", 10));
        registry.insert(request("apr_c", "agent-1", "chat:b", "run_2", 30));
        registry.insert(decided("apr_d", "agent-1", "chat:a", 5));

        let ids = registry
            .pending()
            .iter()
            .map(|approval| approval.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["apr_a", "apr_b", "apr_c"]);
        assert_eq!(registry.pending_ids_for_run("run_1"), ["apr_a", "apr_b"]);
        assert_eq!(registry.pending_count_for_session("agent-1", "chat:a"), 2);
        assert_eq!(registry.pending_count_for_session("agent-2", "chat:a"), 0);
        assert_eq!(registry.decided().len(), 1);
    }

    #[test]
    fn a_policy_defaults_to_asking_before_exec_and_can_be_put_back() {
        let mut registry = ApprovalRegistry::default();
        assert_eq!(registry.policy("agent-1"), ApprovalPolicy::default());
        let strict = ApprovalPolicy::default().with(RiskClass::Write, PolicyAction::Ask);
        assert_eq!(registry.set_policy("agent-1", strict), None);
        assert_eq!(registry.policy("agent-1"), strict);
        let previous = registry.set_policy("agent-1", ApprovalPolicy::default());
        registry.restore_policy("agent-1", previous);
        assert_eq!(registry.policy("agent-1"), strict);
        registry.restore_policy("agent-1", None);
        assert_eq!(registry.policy("agent-1"), ApprovalPolicy::default());
    }

    #[test]
    fn rules_are_per_agent_capped_and_found_by_tool_and_matcher() {
        let mut registry = ApprovalRegistry::default();
        for index in 0..MAX_APPROVAL_RULES_PER_AGENT {
            registry
                .add_rule(rule(&format!("rule_{index}"), "agent-1", index as u64))
                .unwrap();
        }
        assert_eq!(
            registry.add_rule(rule("rule_extra", "agent-1", 999)),
            Err(TOO_MANY_RULES)
        );
        registry.add_rule(rule("rule_other", "agent-2", 1)).unwrap();
        assert_eq!(
            registry.rules_for("agent-1").len(),
            MAX_APPROVAL_RULES_PER_AGENT
        );
        assert_eq!(registry.rules_for("agent-1")[0].id, "rule_0");
        assert!(registry
            .find_rule("agent-2", "memory_add", &ApprovalMatcher::any())
            .is_some());
        assert!(registry
            .find_rule("agent-2", "todo_write", &ApprovalMatcher::any())
            .is_none());
        assert!(
            registry.remove_rule("agent-1", "rule_other").is_none(),
            "a rule is removed only through its own agent"
        );
        assert!(registry.remove_rule("agent-2", "rule_other").is_some());
    }

    #[test]
    fn a_restart_expires_pending_approvals_and_drops_missing_agents() {
        let snapshot = ApprovalSnapshot {
            approvals: vec![
                request("apr_waiting", "agent-1", "chat:a", "run_1", 10),
                decided("apr_done", "agent-1", "chat:a", 5),
                request("apr_orphan", "agent-gone", "chat:a", "run_9", 10),
            ],
            policies: vec![
                AgentApprovalPolicy {
                    agent_id: "agent-1".into(),
                    policy: ApprovalPolicy::default().with(RiskClass::Network, PolicyAction::Deny),
                },
                AgentApprovalPolicy {
                    agent_id: "agent-gone".into(),
                    policy: ApprovalPolicy::default(),
                },
            ],
            rules: vec![
                rule("rule_1", "agent-1", 1),
                rule("rule_2", "agent-gone", 1),
            ],
        };
        let live = HashSet::from(["agent-1".to_string()]);

        let registry = ApprovalRegistry::restored(snapshot, &live, 500);

        let expired = registry.get("apr_waiting").unwrap();
        assert_eq!(expired.status, ApprovalStatus::Expired);
        assert_eq!(expired.revision, 2);
        let resolution = expired.resolution.as_ref().unwrap();
        assert_eq!(
            (
                resolution.resolved_by,
                resolution.resolved_at_ms,
                resolution.decision
            ),
            (ResolvedBy::Restart, 500, None)
        );
        assert_eq!(
            registry.get("apr_done").unwrap().status,
            ApprovalStatus::Allowed
        );
        assert!(registry.get("apr_orphan").is_none());
        assert!(registry.pending().is_empty());
        assert_eq!(registry.policy("agent-1").network, PolicyAction::Deny);
        assert_eq!(registry.policy("agent-gone"), ApprovalPolicy::default());
        assert_eq!(registry.rules_for("agent-1").len(), 1);
        assert!(registry.rules_for("agent-gone").is_empty());
    }

    #[test]
    fn mirroring_removes_only_unchanged_decided_approvals() {
        let mut registry = ApprovalRegistry::default();
        registry.insert(decided("apr_late", "agent-1", "chat:a", 30));
        registry.insert(decided("apr_early", "agent-1", "chat:a", 10));
        registry.insert(request("apr_waiting", "agent-1", "chat:a", "run_1", 20));
        let written = registry.unmirrored_decided(10);
        assert_eq!(
            written
                .iter()
                .map(|approval| approval.id.as_str())
                .collect::<Vec<_>>(),
            ["apr_early", "apr_late"],
            "oldest resolution first, never a pending one"
        );
        registry
            .get_mut("apr_late")
            .unwrap()
            .resolution
            .as_mut()
            .unwrap()
            .note = Some("changed since".into());

        assert_eq!(registry.mark_mirrored(&written), 1);
        assert!(registry.get("apr_early").is_none());
        assert!(
            registry.get("apr_late").is_some(),
            "a changed record is written again"
        );
        assert!(registry.get("apr_waiting").is_some());
        assert_eq!(registry.unmirrored_decided(1).len(), 1);
        assert_eq!(registry.retain_decided(|_| false), 1);
        assert!(
            registry.get("apr_waiting").is_some(),
            "pending ones always stay"
        );
    }

    #[test]
    fn validation_rejects_duplicate_and_empty_ids() {
        let one = request("apr_1", "agent-1", "chat:a", "run_1", 1);
        assert!(ApprovalRegistry::validate(&[one.clone(), one.clone()], &[], &[]).is_err());
        let mut blank = one.clone();
        blank.run_id = " ".into();
        assert!(ApprovalRegistry::validate(&[blank], &[], &[]).is_err());
        let policy = AgentApprovalPolicy {
            agent_id: "agent-1".into(),
            policy: ApprovalPolicy::default(),
        };
        assert!(ApprovalRegistry::validate(&[], &[policy.clone(), policy], &[]).is_err());
        let duplicate = rule("rule_1", "agent-1", 1);
        assert!(ApprovalRegistry::validate(&[], &[], &[duplicate.clone(), duplicate]).is_err());
        assert!(ApprovalRegistry::validate(&[one], &[], &[rule("rule_1", "agent-1", 1)]).is_ok());
    }

    #[test]
    fn validation_rejects_a_status_its_revision_or_resolution_does_not_fit() {
        let mut no_revision = request("apr_1", "agent-1", "chat:a", "run_1", 1);
        no_revision.revision = 0;
        assert!(ApprovalRegistry::validate(&[no_revision], &[], &[]).is_err());
        let mut allowed_without_resolution = request("apr_2", "agent-1", "chat:a", "run_1", 1);
        allowed_without_resolution.status = ApprovalStatus::Allowed;
        assert!(ApprovalRegistry::validate(&[allowed_without_resolution], &[], &[]).is_err());
        let mut ownerless = rule("rule_1", "agent-1", 1);
        ownerless.agent_id = " ".into();
        let error = ApprovalRegistry::validate(&[], &[], &[ownerless]).unwrap_err();
        assert!(error.contains("empty agent id"), "{error}");
    }

    #[test]
    fn approvals_policies_rules_and_allowances_survive_a_save_and_a_restart() {
        let mut source = DaemonState::new();
        let agent_id = source.create_agent(config("companion")).unwrap().state.id;
        let mut session = SessionRecord::new(
            &agent_id,
            "chat:kept",
            SessionKind::Chat,
            SessionOrigin::Web,
            "Kept".into(),
            TitleSource::Owner,
            1,
        );
        session.session_allowances.push(SessionAllowance {
            tool: "memory_add".into(),
            matcher: ApprovalMatcher::any(),
            created_at_ms: 2,
            from_approval_id: "apr_session".into(),
        });
        source.sessions.insert(session);
        let mut awaiting = RunRecord::running(
            RunStart {
                agent_id: agent_id.clone(),
                session_id: "chat:kept".into(),
                source: RunSource::Web,
                source_ref: None,
                idempotency_key: None,
                text: "remember the plan".into(),
                model: "deterministic".into(),
                provider: None,
                parent_run_id: None,
            },
            1,
        );
        awaiting.status = RunStatus::AwaitingApproval;
        let run_id = awaiting.id.clone();
        source.runs.insert(awaiting);
        source
            .approvals
            .insert(request("apr_waiting", &agent_id, "chat:kept", &run_id, 3));
        source
            .approvals
            .insert(decided("apr_done", &agent_id, "chat:kept", 2));
        source
            .approvals
            .insert(decided("apr_deleted_session", &agent_id, "chat:gone", 2));
        source.approvals.set_policy(
            &agent_id,
            ApprovalPolicy::default().with(RiskClass::Write, PolicyAction::Ask),
        );
        source
            .approvals
            .add_rule(rule("rule_1", &agent_id, 4))
            .unwrap();

        let payload = serde_json::to_value(source.control_plane_snapshot()).unwrap();
        assert_eq!(payload["version"], 7);
        assert_eq!(payload["approvals"].as_array().unwrap().len(), 2);
        assert_eq!(payload["approvalPolicies"][0]["policy"]["write"], "ask");
        assert_eq!(payload["approvalRules"][0]["id"], "rule_1");
        assert_eq!(
            payload["sessions"][0]["sessionAllowances"][0]["fromApprovalId"],
            "apr_session"
        );

        let mut restored = DaemonState::new();
        restored
            .restore_control_plane_snapshot(serde_json::from_value(payload).unwrap())
            .unwrap();
        let expired = restored.approvals.get("apr_waiting").unwrap();
        assert_eq!(expired.status, ApprovalStatus::Expired);
        assert_eq!(
            expired.resolution.as_ref().unwrap().resolved_by,
            ResolvedBy::Restart
        );
        let run = restored.runs.get(&run_id).unwrap();
        assert_eq!(run.status, RunStatus::Interrupted);
        assert_eq!(run.error.as_ref().unwrap().code, RESTART_DURING_RUN);
        assert!(restored.approvals.get("apr_done").is_some());
        assert!(restored.approvals.get("apr_deleted_session").is_none());
        assert_eq!(
            restored.approvals.policy(&agent_id).write,
            PolicyAction::Ask
        );
        assert_eq!(restored.approvals.rules_for(&agent_id).len(), 1);
        assert_eq!(
            restored
                .sessions
                .get(&agent_id, "chat:kept")
                .unwrap()
                .session_allowances
                .len(),
            1
        );
    }
}
