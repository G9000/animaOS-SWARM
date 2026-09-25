//! The live hub on the daemon state (spec §6).

use std::collections::HashSet;

use super::DaemonState;
use crate::agent_runs::config_helper_parent;
use crate::live::{LiveEvent, LiveEventBody, LiveHub, SnapshotRun};
use crate::runs::RunRecord;

impl DaemonState {
    /// Replaces the hub; used once at startup to apply the configured buffer.
    pub(crate) fn set_live_hub(&mut self, hub: LiveHub) {
        self.live = hub;
    }

    /// `record` with the tools its live run started since the ledger record
    /// was last written, so saves and reads made mid-run report them (spec
    /// §4.8). Terminal records already hold their final list.
    pub(crate) fn with_live_tools(&self, mut record: RunRecord) -> RunRecord {
        if !record.status.is_terminal() {
            for name in self.live.runs().tools_started(&record.id) {
                record.note_tool_started(&name);
            }
        }
        record
    }

    /// The runs a new stream of `agent_id` starts from (spec §6): queued,
    /// running, and awaiting-approval runs of the agent, of its helpers, and
    /// of sessions delegated from it, oldest first, with their live state.
    pub(crate) fn live_snapshot_runs(&self, agent_id: &str) -> Vec<SnapshotRun> {
        let helpers = self
            .agents
            .iter()
            .filter(|(_, runtime)| config_helper_parent(runtime.config()) == Some(agent_id))
            .map(|(id, _)| id.as_str())
            .collect::<HashSet<_>>();
        let mut runs = self
            .runs
            .active_records()
            .into_iter()
            .filter(|record| {
                record.agent_id == agent_id
                    || helpers.contains(record.agent_id.as_str())
                    || self
                        .sessions
                        .get(&record.agent_id, &record.session_id)
                        .and_then(|session| session.parent_agent_id.as_deref())
                        == Some(agent_id)
            })
            .map(|record| SnapshotRun {
                live: self.live.runs().view(&record.id),
                record: self.with_live_tools(record.clone()),
            })
            .collect::<Vec<_>>();
        runs.sort_by(|left, right| {
            left.record
                .created_at_ms
                .cmp(&right.record.created_at_ms)
                .then_with(|| left.record.id.cmp(&right.record.id))
        });
        runs
    }

    /// The agent whose stream also carries events of `agent_id`'s session
    /// `session_id`: a helper's companion, otherwise the agent a delegated,
    /// helper, or peer session came from (spec §6).
    pub(crate) fn live_parent_agent(&self, agent_id: &str, session_id: &str) -> Option<String> {
        self.agents
            .get(agent_id)
            .and_then(|runtime| config_helper_parent(runtime.config()))
            .map(str::to_string)
            .or_else(|| {
                self.sessions
                    .get(agent_id, session_id)
                    .and_then(|session| session.parent_agent_id.clone())
            })
    }

    /// Publishes a session lifecycle event (spec §6).
    pub(crate) fn publish_session_event(
        &self,
        agent_id: &str,
        session_id: &str,
        body: LiveEventBody,
    ) {
        let parent = self.live_parent_agent(agent_id, session_id);
        self.live.publish(
            LiveEvent::new(agent_id, body).session(session_id),
            parent.as_deref(),
        );
    }
}
