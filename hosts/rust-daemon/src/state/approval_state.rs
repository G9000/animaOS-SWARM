//! Approval state changes (spec §7.3), made under the control-plane
//! transaction and the state write lock: opening a request, settling it,
//! and putting either back when its save fails. Nothing here awaits.
#![allow(dead_code)] // M4 Task 8 removes this once the gate and the routes use every item.

use anima_core::{AgentState, Content, TaskResult, ToolCall, CANCELLED_TOOL_RESULT};

use super::DaemonState;
use crate::agent_runs::{config_helper_parent, is_helper_config};
use crate::approvals::{
    evaluate, validate_matcher, ApprovalDecisionKind, ApprovalMatcher, ApprovalRequest,
    ApprovalResolution, ApprovalRule, ApprovalStatus, PendingApprovalStart, ResolvedBy,
    SessionAllowance, Verdict, APPROVAL_NOTE_TOO_LONG, APPROVAL_SESSION_GONE, APPROVAL_TIMED_OUT,
    APPROVAL_UNAVAILABLE, MAX_APPROVAL_NOTE_CHARS, MAX_SESSION_ALLOWANCES,
    TOO_MANY_SESSION_ALLOWANCES,
};
use crate::runs::{RunLink, RunRecord, RunStatus};

/// A call that needs the owner (spec §7.3).
#[derive(Clone, Debug)]
pub(crate) struct ApprovalAsk {
    pub(crate) run: RunLink,
    pub(crate) call: ToolCall,
    pub(crate) timeout_ms: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct OpenedApproval {
    pub(crate) approval: ApprovalRequest,
    /// The run as it now is, when this request moved it to `awaiting_approval`.
    pub(crate) run: Option<RunRecord>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OpenRefusal {
    /// The run's stop is saved: the call never runs.
    Stopped,
    /// The run is not in flight in the ledger.
    Unavailable,
}

impl OpenRefusal {
    pub(crate) fn result(self) -> TaskResult<Content> {
        TaskResult::error(
            match self {
                Self::Stopped => CANCELLED_TOOL_RESULT,
                Self::Unavailable => APPROVAL_UNAVAILABLE,
            },
            0,
        )
    }
}

/// The owner's decision as the decision route received it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OwnerDecision {
    pub(crate) kind: ApprovalDecisionKind,
    pub(crate) note: Option<String>,
    pub(crate) matcher: Option<ApprovalMatcher>,
    pub(crate) revision: u64,
}

/// What settles a pending approval (spec §7.3).
#[derive(Clone, Debug)]
pub(crate) enum Settlement {
    Owner(OwnerDecision),
    TimedOut,
    Stopped,
}

/// Why a settlement changed nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SettleRefusal {
    /// The control plane holds no such approval.
    NotFound,
    /// Something settled it first: the record as it stands.
    Resolved(ApprovalRequest),
    /// The decision carried an old revision.
    Stale,
    /// A 400: the note or the matcher.
    Invalid(&'static str),
    /// A 409: a cap, or the session is gone.
    Conflict(&'static str),
}

/// What a settlement changed, as it was, so a failed save can put it back.
#[derive(Clone, Debug)]
pub(crate) struct ApprovalUndo {
    previous: ApprovalRequest,
    added_rule: Option<String>,
    added_allowance: bool,
    resumed_run: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct SettledApproval {
    pub(crate) approval: ApprovalRequest,
    /// The run as it now is, when this settlement moved it back to `running`.
    pub(crate) run: Option<RunRecord>,
    pub(crate) undo: ApprovalUndo,
}

/// A trimmed note of at most `MAX_APPROVAL_NOTE_CHARS`, `None` when blank.
fn normalized_note(note: Option<&str>) -> Result<Option<String>, SettleRefusal> {
    let Some(note) = note.map(str::trim).filter(|note| !note.is_empty()) else {
        return Ok(None);
    };
    if note.chars().count() > MAX_APPROVAL_NOTE_CHARS {
        return Err(SettleRefusal::Invalid(APPROVAL_NOTE_TOO_LONG));
    }
    Ok(Some(note.to_string()))
}

impl DaemonState {
    /// What `call` needs in `session_id` (spec §7.2). A helper answers to
    /// its companion's policy and rules and has no session allowances.
    pub(crate) fn approval_verdict(
        &self,
        agent: &AgentState,
        session_id: &str,
        call: &ToolCall,
    ) -> Verdict {
        let owner = config_helper_parent(&agent.config).unwrap_or(agent.id.as_str());
        let policy = self.approvals.policy(owner);
        let rules = self.approvals.rules_for(owner);
        let allowances: &[SessionAllowance] = if is_helper_config(&agent.config) {
            &[]
        } else {
            self.sessions
                .get(&agent.id, session_id)
                .map(|session| session.session_allowances.as_slice())
                .unwrap_or(&[])
        };
        evaluate(&policy, &rules, allowances, call)
    }

    /// Records a pending approval for `ask` and moves its run to
    /// `awaiting_approval` (spec §7.3). The caller saves, then announces.
    pub(crate) fn open_approval(
        &mut self,
        ask: &ApprovalAsk,
        now_ms: u64,
    ) -> Result<OpenedApproval, OpenRefusal> {
        let run = self
            .runs
            .get_mut(&ask.run.run_id)
            .filter(|run| run.agent_id == ask.run.agent_id)
            .ok_or(OpenRefusal::Unavailable)?;
        if run.stop.is_some() {
            return Err(OpenRefusal::Stopped);
        }
        if !run.status.is_in_flight() {
            return Err(OpenRefusal::Unavailable);
        }
        let moved = run.status == RunStatus::Running;
        if moved {
            run.status = RunStatus::AwaitingApproval;
        }
        let moved = moved.then(|| run.clone());
        let approval = ApprovalRequest::pending(
            PendingApprovalStart {
                agent_id: &ask.run.agent_id,
                session_id: &ask.run.session_id,
                run_id: &ask.run.run_id,
                call: &ask.call,
                timeout_ms: ask.timeout_ms,
            },
            now_ms,
        );
        self.approvals.insert(approval.clone());
        Ok(OpenedApproval {
            approval,
            run: moved.map(|record| self.with_live_tools(record)),
        })
    }

    /// Takes back a request whose save failed.
    pub(crate) fn revert_open_approval(&mut self, opened: &OpenedApproval) {
        self.approvals.remove(&opened.approval.id);
        self.resume_if_unblocked(&opened.approval.run_id);
    }

    /// Moves `run_id` back to `running` once it waits on no approval.
    fn resume_if_unblocked(&mut self, run_id: &str) -> Option<RunRecord> {
        if !self.approvals.pending_ids_for_run(run_id).is_empty() {
            return None;
        }
        let run = self
            .runs
            .get_mut(run_id)
            .filter(|run| run.status == RunStatus::AwaitingApproval)?;
        run.status = RunStatus::Running;
        let record = run.clone();
        Some(self.with_live_tools(record))
    }

    /// Settles pending approval `id` (spec §7.3): validates, then resolves it
    /// once, adds the allowance or rule, and resumes the run when it waits on
    /// nothing else. The caller saves, and reverts with `undo` if that fails.
    pub(crate) fn settle_approval(
        &mut self,
        id: &str,
        settlement: Settlement,
        now_ms: u64,
    ) -> Result<SettledApproval, SettleRefusal> {
        let previous = self
            .approvals
            .get(id)
            .cloned()
            .ok_or(SettleRefusal::NotFound)?;
        if !previous.is_pending() {
            return Err(SettleRefusal::Resolved(previous));
        }
        let (status, mut resolution, grant) = match settlement {
            Settlement::Owner(decision) => {
                if decision.revision != previous.revision {
                    return Err(SettleRefusal::Stale);
                }
                let note = normalized_note(decision.note.as_deref())?;
                // An explicit matcher, or the approval's suggestion; either
                // way it must pass for the tool (a suggestion may carry an
                // empty value when no safe one exists).
                let matcher = match decision.kind {
                    ApprovalDecisionKind::AllowSession | ApprovalDecisionKind::AllowAlways => Some(
                        validate_matcher(
                            &previous.tool,
                            decision
                                .matcher
                                .as_ref()
                                .unwrap_or(&previous.suggested_matcher),
                        )
                        .map_err(SettleRefusal::Invalid)?,
                    ),
                    ApprovalDecisionKind::AllowOnce | ApprovalDecisionKind::Deny => None,
                };
                let status = if decision.kind == ApprovalDecisionKind::Deny {
                    ApprovalStatus::Denied
                } else {
                    ApprovalStatus::Allowed
                };
                (
                    status,
                    ApprovalResolution {
                        decision: Some(decision.kind),
                        note,
                        matcher: matcher.clone(),
                        rule_id: None,
                        resolved_by: ResolvedBy::Owner,
                        resolved_at_ms: now_ms,
                    },
                    matcher.map(|matcher| (decision.kind, matcher)),
                )
            }
            Settlement::TimedOut => (
                ApprovalStatus::Denied,
                ApprovalResolution {
                    decision: Some(ApprovalDecisionKind::Deny),
                    note: Some(APPROVAL_TIMED_OUT.to_string()),
                    matcher: None,
                    rule_id: None,
                    resolved_by: ResolvedBy::Timeout,
                    resolved_at_ms: now_ms,
                },
                None,
            ),
            Settlement::Stopped => (
                ApprovalStatus::Stopped,
                ApprovalResolution {
                    decision: None,
                    note: None,
                    matcher: None,
                    rule_id: None,
                    resolved_by: ResolvedBy::Stop,
                    resolved_at_ms: now_ms,
                },
                None,
            ),
        };
        let mut undo = ApprovalUndo {
            previous: previous.clone(),
            added_rule: None,
            added_allowance: false,
            resumed_run: false,
        };
        match grant {
            Some((ApprovalDecisionKind::AllowSession, matcher)) => {
                let session = self
                    .sessions
                    .get_mut(&previous.agent_id, &previous.session_id)
                    .ok_or(SettleRefusal::Conflict(APPROVAL_SESSION_GONE))?;
                let held = session.session_allowances.iter().any(|allowance| {
                    allowance.tool == previous.tool && allowance.matcher == matcher
                });
                if !held {
                    if session.session_allowances.len() >= MAX_SESSION_ALLOWANCES {
                        return Err(SettleRefusal::Conflict(TOO_MANY_SESSION_ALLOWANCES));
                    }
                    session.session_allowances.push(SessionAllowance {
                        tool: previous.tool.clone(),
                        matcher,
                        created_at_ms: now_ms,
                        from_approval_id: previous.id.clone(),
                    });
                    undo.added_allowance = true;
                }
            }
            Some((_, matcher)) => {
                let existing = self
                    .approvals
                    .find_rule(&previous.agent_id, &previous.tool, &matcher)
                    .map(|rule| rule.id.clone());
                match existing {
                    Some(rule_id) => resolution.rule_id = Some(rule_id),
                    None => {
                        let rule = ApprovalRule {
                            id: format!("rule_{}", uuid::Uuid::new_v4()),
                            agent_id: previous.agent_id.clone(),
                            tool: previous.tool.clone(),
                            matcher,
                            created_at_ms: now_ms,
                            from_approval_id: Some(previous.id.clone()),
                        };
                        let rule_id = rule.id.clone();
                        self.approvals
                            .add_rule(rule)
                            .map_err(SettleRefusal::Conflict)?;
                        resolution.rule_id = Some(rule_id.clone());
                        undo.added_rule = Some(rule_id);
                    }
                }
            }
            None => {}
        }
        let approval = {
            let record = self
                .approvals
                .get_mut(id)
                .expect("the approval was found above");
            record.resolve(status, resolution);
            record.clone()
        };
        let run = self.resume_if_unblocked(&approval.run_id);
        undo.resumed_run = run.is_some();
        Ok(SettledApproval {
            approval,
            run,
            undo,
        })
    }

    /// Puts back what a settlement changed after its save failed. Nothing
    /// else changes the approval meanwhile: every settlement holds the
    /// control-plane transaction until it is saved or reverted.
    pub(crate) fn revert_settled_approval(&mut self, undo: ApprovalUndo) {
        let ApprovalUndo {
            previous,
            added_rule,
            added_allowance,
            resumed_run,
        } = undo;
        if let Some(rule_id) = added_rule {
            self.approvals.remove_rule(&previous.agent_id, &rule_id);
        }
        if added_allowance {
            if let Some(session) = self
                .sessions
                .get_mut(&previous.agent_id, &previous.session_id)
            {
                session
                    .session_allowances
                    .retain(|allowance| allowance.from_approval_id != previous.id);
            }
        }
        if resumed_run {
            if let Some(run) = self
                .runs
                .get_mut(&previous.run_id)
                .filter(|run| run.status == RunStatus::Running)
            {
                run.status = RunStatus::AwaitingApproval;
            }
        }
        self.approvals.insert(previous);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use anima_core::{AgentConfig, AgentSettings, AgentState, DataValue, ToolCall};

    use super::*;
    use crate::approvals::{
        ApprovalDecisionKind, ApprovalMatcher, ApprovalPolicy, ApprovalRule, ApprovalStatus,
        MatcherKind, PolicyAction, ResolvedBy, RiskClass, SessionAllowance, Verdict,
        APPROVAL_NOTE_TOO_LONG, APPROVAL_SESSION_GONE, APPROVAL_TIMED_OUT, APPROVAL_UNAVAILABLE,
        MATCHER_KIND_NOT_FOR_TOOL, MATCHER_VALUE_INVALID, MAX_APPROVAL_RULES_PER_AGENT,
        MAX_SESSION_ALLOWANCES, TOO_MANY_RULES, TOO_MANY_SESSION_ALLOWANCES,
    };
    use crate::runs::{RunLink, RunRecord, RunSource, RunStart, RunStatus, RunStopRequest};
    use crate::sessions::{SessionKind, SessionOrigin, SessionRecord, TitleSource};
    use crate::state::DaemonState;

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

    fn call(name: &str, key: &str, value: &str) -> ToolCall {
        ToolCall {
            id: "call-1".into(),
            name: name.into(),
            args: BTreeMap::from([(key.to_string(), DataValue::String(value.into()))]),
        }
    }

    fn remember() -> ToolCall {
        call("memory_add", "content", "the plan")
    }

    fn fetch() -> ToolCall {
        call("web_fetch", "url", "https://docs.rs/serde")
    }

    /// A daemon with one agent, its chat `chat:a`, and a running run there.
    fn daemon() -> (DaemonState, RunLink) {
        let mut state = DaemonState::new();
        let agent_id = state.create_agent(config("companion")).unwrap().state.id;
        state.sessions.insert(SessionRecord::new(
            &agent_id,
            "chat:a",
            SessionKind::Chat,
            SessionOrigin::Web,
            "A".into(),
            TitleSource::Owner,
            1,
        ));
        let run = RunRecord::running(
            RunStart {
                agent_id: agent_id.clone(),
                session_id: "chat:a".into(),
                source: RunSource::Web,
                source_ref: None,
                idempotency_key: None,
                text: "remember".into(),
                model: "deterministic".into(),
                provider: None,
                parent_run_id: None,
            },
            1,
        );
        let link = RunLink {
            run_id: run.id.clone(),
            session_id: "chat:a".into(),
            agent_id,
        };
        state.runs.insert(run);
        (state, link)
    }

    fn ask(link: &RunLink, call: ToolCall) -> ApprovalAsk {
        ApprovalAsk {
            run: link.clone(),
            call,
            timeout_ms: 60_000,
        }
    }

    fn owner(kind: ApprovalDecisionKind, revision: u64) -> Settlement {
        Settlement::Owner(OwnerDecision {
            kind,
            note: None,
            matcher: None,
            revision,
        })
    }

    fn agent_state(state: &DaemonState, agent_id: &str) -> AgentState {
        state.get_agent(agent_id).unwrap().state
    }

    fn run_status(state: &DaemonState, link: &RunLink) -> RunStatus {
        state.runs.get(&link.run_id).unwrap().status
    }

    #[test]
    fn a_verdict_follows_the_policy_rules_and_session_allowances() {
        let (mut state, link) = daemon();
        let agent = agent_state(&state, &link.agent_id);
        assert_eq!(
            state.approval_verdict(&agent, "chat:a", &remember()),
            Verdict::Allow
        );

        state.approvals.set_policy(
            &link.agent_id,
            ApprovalPolicy::default().with(RiskClass::Write, PolicyAction::Ask),
        );
        assert_eq!(
            state.approval_verdict(&agent, "chat:a", &remember()),
            Verdict::Ask
        );

        state
            .sessions
            .get_mut(&link.agent_id, "chat:a")
            .unwrap()
            .session_allowances
            .push(SessionAllowance {
                tool: "memory_add".into(),
                matcher: ApprovalMatcher::any(),
                created_at_ms: 1,
                from_approval_id: "apr_1".into(),
            });
        assert_eq!(
            state.approval_verdict(&agent, "chat:a", &remember()),
            Verdict::Allow
        );
        assert_eq!(
            state.approval_verdict(&agent, "chat:b", &remember()),
            Verdict::Ask,
            "an allowance covers its own session only"
        );
    }

    #[test]
    fn a_helper_answers_to_its_companion_without_session_allowances() {
        let (mut state, link) = daemon();
        let mut helper = config("helper");
        let settings = helper.settings.as_mut().unwrap();
        settings
            .additional
            .insert("workspaceRole".into(), DataValue::String("helper".into()));
        settings.additional.insert(
            "parentAgentId".into(),
            DataValue::String(link.agent_id.clone()),
        );
        let helper = state.create_agent(helper).unwrap().state;
        let mut room = SessionRecord::new(
            &helper.id,
            "room-helper",
            SessionKind::Helper,
            SessionOrigin::Delegation,
            "Helper".into(),
            TitleSource::System,
            1,
        );
        room.session_allowances.push(SessionAllowance {
            tool: "memory_add".into(),
            matcher: ApprovalMatcher::any(),
            created_at_ms: 1,
            from_approval_id: "apr_1".into(),
        });
        state.sessions.insert(room);
        state.approvals.set_policy(
            &link.agent_id,
            ApprovalPolicy::default().with(RiskClass::Write, PolicyAction::Ask),
        );

        assert_eq!(
            state.approval_verdict(&helper, "room-helper", &remember()),
            Verdict::Ask,
            "the companion's policy, and no allowance counts for a helper"
        );
        state
            .approvals
            .add_rule(ApprovalRule {
                id: "rule_1".into(),
                agent_id: link.agent_id.clone(),
                tool: "memory_add".into(),
                matcher: ApprovalMatcher::any(),
                created_at_ms: 1,
                from_approval_id: None,
            })
            .unwrap();
        assert_eq!(
            state.approval_verdict(&helper, "room-helper", &remember()),
            Verdict::Allow,
            "the companion's rules cover its helpers"
        );
    }

    #[test]
    fn opening_a_request_moves_the_run_to_awaiting_approval_once() {
        let (mut state, link) = daemon();
        let first = state.open_approval(&ask(&link, remember()), 10).unwrap();
        assert_eq!(first.approval.status, ApprovalStatus::Pending);
        assert_eq!(first.approval.expires_at_ms, 60_010);
        assert_eq!(
            first.run.as_ref().map(|run| run.status),
            Some(RunStatus::AwaitingApproval)
        );
        let second = state.open_approval(&ask(&link, fetch()), 11).unwrap();
        assert!(second.run.is_none(), "the run already awaits approval");
        assert_eq!(state.approvals.pending_ids_for_run(&link.run_id).len(), 2);

        state.revert_open_approval(&second);
        assert_eq!(run_status(&state, &link), RunStatus::AwaitingApproval);
        state.revert_open_approval(&first);
        assert_eq!(run_status(&state, &link), RunStatus::Running);
        assert!(state.approvals.pending().is_empty());
    }

    #[test]
    fn a_stopped_or_missing_run_cannot_ask() {
        let (mut state, link) = daemon();
        state.runs.get_mut(&link.run_id).unwrap().stop =
            Some(RunStopRequest { requested_at_ms: 5 });
        let refusal = state
            .open_approval(&ask(&link, remember()), 10)
            .unwrap_err();
        assert_eq!(refusal, OpenRefusal::Stopped);
        assert_eq!(
            refusal.result().error.as_deref(),
            Some(anima_core::CANCELLED_TOOL_RESULT)
        );
        let missing = RunLink {
            run_id: "run_missing".into(),
            ..link.clone()
        };
        let refusal = state
            .open_approval(&ask(&missing, remember()), 10)
            .unwrap_err();
        assert_eq!(refusal, OpenRefusal::Unavailable);
        assert_eq!(
            refusal.result().error.as_deref(),
            Some(APPROVAL_UNAVAILABLE)
        );
        assert!(state.approvals.pending().is_empty());
    }

    #[test]
    fn an_owner_decision_settles_once_and_needs_the_current_revision() {
        let (mut state, link) = daemon();
        let id = state
            .open_approval(&ask(&link, remember()), 10)
            .unwrap()
            .approval
            .id;
        assert_eq!(
            state
                .settle_approval(&id, owner(ApprovalDecisionKind::AllowOnce, 2), 20)
                .unwrap_err(),
            SettleRefusal::Stale
        );
        let settled = state
            .settle_approval(&id, owner(ApprovalDecisionKind::AllowOnce, 1), 20)
            .unwrap();
        assert_eq!(settled.approval.status, ApprovalStatus::Allowed);
        assert_eq!(settled.approval.revision, 2);
        let resolution = settled.approval.resolution.clone().unwrap();
        assert_eq!(resolution.resolved_by, ResolvedBy::Owner);
        assert_eq!(resolution.decision, Some(ApprovalDecisionKind::AllowOnce));
        assert_eq!(resolution.resolved_at_ms, 20);
        assert_eq!(settled.run.map(|run| run.status), Some(RunStatus::Running));

        match state.settle_approval(&id, owner(ApprovalDecisionKind::Deny, 2), 30) {
            Err(SettleRefusal::Resolved(record)) => {
                assert!(record.was_decided_as(ApprovalDecisionKind::AllowOnce));
                assert!(!record.was_decided_as(ApprovalDecisionKind::Deny));
            }
            other => panic!("expected the resolved record, got {other:?}"),
        }
        assert_eq!(
            state
                .settle_approval("apr_missing", owner(ApprovalDecisionKind::AllowOnce, 1), 30)
                .unwrap_err(),
            SettleRefusal::NotFound
        );
    }

    #[test]
    fn allow_session_adds_one_allowance_and_allow_always_one_rule() {
        let (mut state, link) = daemon();
        let first = state
            .open_approval(&ask(&link, fetch()), 10)
            .unwrap()
            .approval;
        let settled = state
            .settle_approval(&first.id, owner(ApprovalDecisionKind::AllowSession, 1), 11)
            .unwrap();
        let domain = ApprovalMatcher {
            kind: MatcherKind::Domain,
            value: "docs.rs".into(),
        };
        assert_eq!(settled.approval.resolution.unwrap().matcher, Some(domain));
        let again = state
            .open_approval(&ask(&link, fetch()), 12)
            .unwrap()
            .approval;
        state
            .settle_approval(&again.id, owner(ApprovalDecisionKind::AllowSession, 1), 13)
            .unwrap();
        {
            let allowances = &state
                .sessions
                .get(&link.agent_id, "chat:a")
                .unwrap()
                .session_allowances;
            assert_eq!(allowances.len(), 1, "the same grant is kept once");
            assert_eq!(allowances[0].from_approval_id, first.id);
        }

        let third = state
            .open_approval(&ask(&link, fetch()), 14)
            .unwrap()
            .approval;
        let always = state
            .settle_approval(
                &third.id,
                Settlement::Owner(OwnerDecision {
                    kind: ApprovalDecisionKind::AllowAlways,
                    note: Some("  docs are fine  ".into()),
                    matcher: Some(ApprovalMatcher {
                        kind: MatcherKind::Any,
                        value: "ignored".into(),
                    }),
                    revision: 1,
                }),
                15,
            )
            .unwrap()
            .approval;
        let resolution = always.resolution.unwrap();
        assert_eq!(resolution.note.as_deref(), Some("docs are fine"));
        let rule_id = {
            let rules = state.approvals.rules_for(&link.agent_id);
            assert_eq!(rules.len(), 1);
            assert_eq!(rules[0].matcher, ApprovalMatcher::any());
            assert_eq!(rules[0].tool, "web_fetch");
            assert_eq!(
                rules[0].from_approval_id.as_deref(),
                Some(third.id.as_str())
            );
            rules[0].id.clone()
        };
        assert_eq!(resolution.rule_id.as_deref(), Some(rule_id.as_str()));

        let fourth = state
            .open_approval(&ask(&link, fetch()), 16)
            .unwrap()
            .approval;
        let repeat = state
            .settle_approval(
                &fourth.id,
                Settlement::Owner(OwnerDecision {
                    kind: ApprovalDecisionKind::AllowAlways,
                    note: None,
                    matcher: Some(ApprovalMatcher::any()),
                    revision: 1,
                }),
                17,
            )
            .unwrap()
            .approval;
        assert_eq!(
            repeat.resolution.unwrap().rule_id.as_deref(),
            Some(rule_id.as_str()),
            "an identical rule is reused"
        );
        assert_eq!(state.approvals.rules_for(&link.agent_id).len(), 1);
    }

    #[test]
    fn a_decision_is_validated_before_anything_changes() {
        let (mut state, link) = daemon();
        let id = state
            .open_approval(&ask(&link, remember()), 10)
            .unwrap()
            .approval
            .id;
        let long = Settlement::Owner(OwnerDecision {
            kind: ApprovalDecisionKind::Deny,
            note: Some("x".repeat(1_001)),
            matcher: None,
            revision: 1,
        });
        assert_eq!(
            state.settle_approval(&id, long, 11).unwrap_err(),
            SettleRefusal::Invalid(APPROVAL_NOTE_TOO_LONG)
        );
        let misfit = Settlement::Owner(OwnerDecision {
            kind: ApprovalDecisionKind::AllowAlways,
            note: None,
            matcher: Some(ApprovalMatcher {
                kind: MatcherKind::PathGlob,
                value: "**".into(),
            }),
            revision: 1,
        });
        assert_eq!(
            state.settle_approval(&id, misfit, 11).unwrap_err(),
            SettleRefusal::Invalid(MATCHER_KIND_NOT_FOR_TOOL)
        );
        assert!(state.approvals.get(&id).unwrap().is_pending());
        assert!(state.approvals.rules_for(&link.agent_id).is_empty());

        let exactly = Settlement::Owner(OwnerDecision {
            kind: ApprovalDecisionKind::Deny,
            note: Some("y".repeat(1_000)),
            matcher: None,
            revision: 1,
        });
        let denied = state.settle_approval(&id, exactly, 12).unwrap().approval;
        assert_eq!(denied.status, ApprovalStatus::Denied);
        assert_eq!(
            denied.resolution.unwrap().note.unwrap().chars().count(),
            1_000
        );
    }

    #[test]
    fn an_empty_suggestion_is_refused_as_a_fallback_for_a_scoped_grant() {
        let (mut state, link) = daemon();
        // No safe glob can be suggested for an absolute path.
        let id = state
            .open_approval(
                &ask(&link, call("write_file", "file_path", "/etc/passwd")),
                10,
            )
            .unwrap()
            .approval;
        assert_eq!(id.suggested_matcher.kind, MatcherKind::PathGlob);
        assert!(id.suggested_matcher.value.is_empty());
        for kind in [
            ApprovalDecisionKind::AllowSession,
            ApprovalDecisionKind::AllowAlways,
        ] {
            assert_eq!(
                state
                    .settle_approval(&id.id, owner(kind, 1), 11)
                    .unwrap_err(),
                SettleRefusal::Invalid(MATCHER_VALUE_INVALID)
            );
        }
        assert!(state.approvals.get(&id.id).unwrap().is_pending());
        assert!(state.approvals.rules_for(&link.agent_id).is_empty());
        assert!(state
            .sessions
            .get(&link.agent_id, "chat:a")
            .unwrap()
            .session_allowances
            .is_empty());

        // The owner can still choose `any` explicitly.
        let any = Settlement::Owner(OwnerDecision {
            kind: ApprovalDecisionKind::AllowAlways,
            note: None,
            matcher: Some(ApprovalMatcher::any()),
            revision: 1,
        });
        assert!(state.settle_approval(&id.id, any, 12).is_ok());
    }

    #[test]
    fn a_timeout_is_a_denial_and_a_stop_is_stopped() {
        let (mut state, link) = daemon();
        let timed = state
            .open_approval(&ask(&link, remember()), 10)
            .unwrap()
            .approval;
        let stopped = state
            .open_approval(&ask(&link, fetch()), 10)
            .unwrap()
            .approval;

        let timed = state
            .settle_approval(&timed.id, Settlement::TimedOut, 20)
            .unwrap()
            .approval;
        assert_eq!(timed.status, ApprovalStatus::Denied);
        let resolution = timed.resolution.unwrap();
        assert_eq!(resolution.decision, Some(ApprovalDecisionKind::Deny));
        assert_eq!(resolution.note.as_deref(), Some(APPROVAL_TIMED_OUT));
        assert_eq!(resolution.resolved_by, ResolvedBy::Timeout);

        let stopped = state
            .settle_approval(&stopped.id, Settlement::Stopped, 21)
            .unwrap()
            .approval;
        assert_eq!(stopped.status, ApprovalStatus::Stopped);
        let resolution = stopped.resolution.unwrap();
        assert_eq!(resolution.decision, None);
        assert_eq!(resolution.resolved_by, ResolvedBy::Stop);
    }

    #[test]
    fn the_run_resumes_only_when_its_last_approval_settles() {
        let (mut state, link) = daemon();
        let first = state
            .open_approval(&ask(&link, remember()), 10)
            .unwrap()
            .approval;
        let second = state
            .open_approval(&ask(&link, fetch()), 11)
            .unwrap()
            .approval;

        let settled = state
            .settle_approval(&first.id, owner(ApprovalDecisionKind::AllowOnce, 1), 12)
            .unwrap();
        assert!(settled.run.is_none());
        assert_eq!(run_status(&state, &link), RunStatus::AwaitingApproval);
        let settled = state
            .settle_approval(&second.id, owner(ApprovalDecisionKind::Deny, 1), 13)
            .unwrap();
        assert_eq!(settled.run.map(|run| run.status), Some(RunStatus::Running));
        assert_eq!(run_status(&state, &link), RunStatus::Running);
    }

    #[test]
    fn a_reverted_settlement_puts_everything_back() {
        let (mut state, link) = daemon();
        let always = state
            .open_approval(&ask(&link, fetch()), 10)
            .unwrap()
            .approval;
        let settled = state
            .settle_approval(&always.id, owner(ApprovalDecisionKind::AllowAlways, 1), 11)
            .unwrap();
        assert_eq!(state.approvals.rules_for(&link.agent_id).len(), 1);
        state.revert_settled_approval(settled.undo);
        let restored = state.approvals.get(&always.id).unwrap();
        assert!(restored.is_pending());
        assert_eq!(restored.revision, 1);
        assert!(state.approvals.rules_for(&link.agent_id).is_empty());
        assert_eq!(run_status(&state, &link), RunStatus::AwaitingApproval);

        let settled = state
            .settle_approval(&always.id, owner(ApprovalDecisionKind::AllowSession, 1), 12)
            .unwrap();
        state.revert_settled_approval(settled.undo);
        assert!(state
            .sessions
            .get(&link.agent_id, "chat:a")
            .unwrap()
            .session_allowances
            .is_empty());
        assert!(state.approvals.get(&always.id).unwrap().is_pending());
    }

    #[test]
    fn full_rules_and_allowances_refuse_the_next_grant() {
        let (mut state, link) = daemon();
        for index in 0..MAX_APPROVAL_RULES_PER_AGENT {
            state
                .approvals
                .add_rule(ApprovalRule {
                    id: format!("rule_{index}"),
                    agent_id: link.agent_id.clone(),
                    tool: "web_fetch".into(),
                    matcher: ApprovalMatcher {
                        kind: MatcherKind::Domain,
                        value: format!("d{index}.example"),
                    },
                    created_at_ms: 1,
                    from_approval_id: None,
                })
                .unwrap();
        }
        let id = state
            .open_approval(&ask(&link, fetch()), 10)
            .unwrap()
            .approval
            .id;
        assert_eq!(
            state
                .settle_approval(&id, owner(ApprovalDecisionKind::AllowAlways, 1), 11)
                .unwrap_err(),
            SettleRefusal::Conflict(TOO_MANY_RULES)
        );

        let session = state.sessions.get_mut(&link.agent_id, "chat:a").unwrap();
        for index in 0..MAX_SESSION_ALLOWANCES {
            session.session_allowances.push(SessionAllowance {
                tool: "web_fetch".into(),
                matcher: ApprovalMatcher {
                    kind: MatcherKind::Domain,
                    value: format!("s{index}.example"),
                },
                created_at_ms: 1,
                from_approval_id: format!("apr_{index}"),
            });
        }
        assert_eq!(
            state
                .settle_approval(&id, owner(ApprovalDecisionKind::AllowSession, 1), 12)
                .unwrap_err(),
            SettleRefusal::Conflict(TOO_MANY_SESSION_ALLOWANCES)
        );
        assert!(state.approvals.get(&id).unwrap().is_pending());

        state.sessions.remove(&link.agent_id, "chat:a");
        assert_eq!(
            state
                .settle_approval(&id, owner(ApprovalDecisionKind::AllowSession, 1), 13)
                .unwrap_err(),
            SettleRefusal::Conflict(APPROVAL_SESSION_GONE)
        );
    }
}
