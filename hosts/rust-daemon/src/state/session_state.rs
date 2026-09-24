//! Session bookkeeping on the daemon state (spec §3, §13.3).

use std::collections::HashMap;

use anima_core::{AgentConfigUpdate, AgentRuntimeSnapshot};
use tracing::warn;

use super::DaemonState;
use crate::agent_runs::{config_helper_parent, is_helper_config};
use crate::runs::{RunLink, RunSource};
use crate::sessions::migration::{
    derive_sessions_for_legacy_rooms, LegacyAgent, LegacySessionContext, ToolGrantSet,
};
use crate::sessions::{
    connector_id_of_room, derived_title, is_calendar_write_followup, job_id_of_room, kind_for_room,
    labelled_title, schedule_id_of_room, session_id_for_room, session_title, SessionKind,
    SessionRecord, TitleContext, TitleSource,
};

/// What `ensure_run_session` needs to know about a starting run.
pub(crate) struct RunSessionRequest<'a> {
    pub(crate) agent_id: &'a str,
    pub(crate) room_id: &'a str,
    pub(crate) source: RunSource,
    /// Ledger source reference (spec §4.1); `calendar-write:<id>` marks the
    /// calendar connector's own confirmation follow-up (Controller ruling,
    /// M2 pre-flight audit), not an owner-authored `api` call.
    pub(crate) source_ref: Option<&'a str>,
    /// The calendar write's own summary, carried structurally in the run's
    /// content metadata so the title need not parse the confirmation prose.
    pub(crate) calendar_summary: Option<&'a str>,
    /// The delegating agent of a `RunRoom::Delegated` run.
    pub(crate) delegated_parent: Option<&'a str>,
    /// The sending agent of a `RunRoom::Peer` run.
    pub(crate) peer_sender: Option<&'a str>,
    pub(crate) parent: Option<&'a RunLink>,
    pub(crate) first_text: &'a str,
    pub(crate) now_ms: u64,
}

impl DaemonState {
    /// Session records for rooms that have none yet (spec §13.3 step 2).
    pub(crate) fn derive_legacy_sessions(&self) -> Vec<SessionRecord> {
        let agent_names = self
            .agents
            .iter()
            .map(|(id, runtime)| (id.clone(), runtime.config().name.clone()))
            .collect::<HashMap<_, _>>();
        let agents = self
            .agents
            .iter()
            .map(|(id, runtime)| LegacyAgent {
                agent_id: id,
                config: runtime.config(),
                messages: runtime.messages(),
            })
            .collect::<Vec<_>>();
        derive_sessions_for_legacy_rooms(
            &self.sessions,
            &agents,
            &LegacySessionContext {
                schedules: &self.schedules,
                jobs: &self.jobs,
                connectors: &self.connectors,
                agent_names: &agent_names,
            },
        )
    }

    /// Applies the grant sets not applied before; returns the agents that
    /// gained tools. Helpers and agents without a tool list never gain any.
    pub(crate) fn apply_pending_tool_grants(&mut self, grants: &[ToolGrantSet]) -> Vec<String> {
        let mut changed: Vec<String> = Vec::new();
        for grant in grants {
            if self.tool_grants_applied.contains(grant.id) {
                continue;
            }
            for (agent_id, runtime) in self.agents.iter_mut() {
                let config = runtime.config();
                if is_helper_config(config) {
                    continue;
                }
                let Some(current) = config.tools.as_ref() else {
                    continue;
                };
                let has_write_file = current.iter().any(|tool| tool.name == "write_file");
                let mut names = current
                    .iter()
                    .map(|tool| tool.name.clone())
                    .collect::<Vec<_>>();
                let wanted = grant
                    .read_class
                    .iter()
                    .chain(grant.write_class.iter().filter(|_| has_write_file));
                let mut added = false;
                for name in wanted {
                    if self.tool_registry.descriptor(name).is_some()
                        && !names.iter().any(|known| known == name)
                    {
                        names.push((*name).to_string());
                        added = true;
                    }
                }
                if !added {
                    continue;
                }
                let tools = match self.tool_registry.resolve_descriptors(names) {
                    Ok(tools) => tools,
                    Err(error) => {
                        warn!(
                            agent_id = %agent_id,
                            grant_id = grant.id,
                            error = %error,
                            "skipping tool grant: a previously granted tool is no longer registered"
                        );
                        continue;
                    }
                };
                runtime.update_config(AgentConfigUpdate {
                    tools: Some(tools),
                    ..AgentConfigUpdate::default()
                });
                if !changed.contains(agent_id) {
                    changed.push(agent_id.clone());
                }
            }
            self.tool_grants_applied.insert(grant.id.to_string());
        }
        changed
    }

    /// Makes sure the run's room has a session record (spec §3); returns
    /// whether it created one, so a failed run-start save can remove it.
    pub(crate) fn ensure_run_session(&mut self, request: RunSessionRequest<'_>) -> bool {
        let session_id = session_id_for_room(request.room_id);
        if self.sessions.contains(request.agent_id, &session_id) {
            return false;
        }
        let helper_parent = self
            .agents
            .get(request.agent_id)
            .and_then(|runtime| config_helper_parent(runtime.config()))
            .map(str::to_string);
        let (kind, origin) = kind_for_room(
            request.room_id,
            Some(request.source),
            helper_parent.is_some(),
        );
        let peer_sender_name = request
            .peer_sender
            .and_then(|id| self.agents.get(id))
            .map(|runtime| runtime.config().name.clone());
        let title_context = TitleContext {
            first_user_text: Some(request.first_text),
            schedule_prompt: schedule_id_of_room(request.room_id)
                .and_then(|id| self.schedules.get(id))
                .map(|schedule| schedule.prompt.as_str()),
            job_title: job_id_of_room(request.room_id)
                .and_then(|id| self.jobs.get(id))
                .map(|job| job.title.as_str()),
            bot_username: connector_id_of_room(request.room_id)
                .and_then(|id| self.connectors.get(id))
                .and_then(|connector| connector.bot.username.as_deref()),
            peer_sender_name: peer_sender_name.as_deref(),
        };
        let (title, title_source) = if is_calendar_write_followup(request.source_ref) {
            // Controller ruling (M2 pre-flight audit): a system follow-up,
            // titled from the write's own summary, not the owner's first
            // message.
            (
                labelled_title(
                    "Calendar",
                    request.calendar_summary.and_then(derived_title),
                    "Calendar update",
                ),
                TitleSource::System,
            )
        } else {
            session_title(kind, origin, &title_context)
        };
        let mut record = SessionRecord::new(
            request.agent_id,
            request.room_id,
            kind,
            origin,
            title,
            title_source,
            request.now_ms,
        );
        if kind == SessionKind::Helper {
            record.parent_session_id = request.parent.map(|link| link.session_id.clone());
            record.parent_run_id = request.parent.map(|link| link.run_id.clone());
            record.parent_agent_id = request
                .parent
                .map(|link| link.agent_id.clone())
                .or_else(|| request.delegated_parent.map(str::to_string))
                .or_else(|| request.peer_sender.map(str::to_string))
                .or(helper_parent);
        }
        self.sessions.insert(record);
        true
    }

    /// Agents without their transcripts or events, ordered like `list_agents`
    /// (`GET /api/agents?view=summary`, spec §3.3).
    pub(crate) fn agent_summaries(&self) -> Vec<AgentRuntimeSnapshot> {
        let mut summaries = self
            .agents
            .values()
            .map(|runtime| {
                self.with_derived_status(AgentRuntimeSnapshot {
                    message_count: runtime.messages().len(),
                    ..runtime.run_snapshot(Vec::new())
                })
            })
            .collect::<Vec<_>>();
        summaries.sort_by(|left, right| {
            left.state
                .created_at_ms
                .cmp(&right.state.created_at_ms)
                .then_with(|| left.state.id.cmp(&right.state.id))
        });
        summaries
    }
}
