//! Session bookkeeping on the daemon state (spec §3, §13.3).

use std::collections::HashMap;

use anima_core::AgentConfigUpdate;

use super::DaemonState;
use crate::agent_runs::config_helper_parent;
use crate::sessions::migration::{
    derive_sessions_for_legacy_rooms, LegacyAgent, LegacySessionContext, ToolGrantSet,
};
use crate::sessions::SessionRecord;

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
                if config_helper_parent(config).is_some() {
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
                let tools = self
                    .tool_registry
                    .resolve_descriptors(names)
                    .expect("only registered tools are granted");
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
        for agent_id in &changed {
            if let Some(runtime) = self.agents.get(agent_id) {
                self.agent_snapshots
                    .insert(agent_id.clone(), runtime.snapshot());
            }
        }
        changed
    }
}
