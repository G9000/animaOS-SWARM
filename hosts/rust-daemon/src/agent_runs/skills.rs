//! Skills from the coordinator (spec §8): the service its tools and the
//! routes use, and what each run sees.

use std::sync::Arc;

use anima_core::{AgentRuntime, Content};
use tracing::warn;

use super::AgentRunCoordinator;
use crate::skills::runtime::{index_text, requested_skill, requested_text, SkillContextProvider};
use crate::skills::SkillService;

impl AgentRunCoordinator {
    /// The workspace's skills, changed under this coordinator's
    /// control-plane transaction.
    pub(crate) fn skills(&self) -> SkillService {
        SkillService::new(Arc::clone(&self.state), self.control_plane_transactions())
    }

    /// Adds the skills index and a `/skill` message's instructions to this
    /// run's isolated runtime (spec §8.3). Neither ever fails the run: a
    /// skill that cannot be loaded is logged and noted for the model.
    pub(super) async fn apply_skills(&self, runtime: &mut AgentRuntime, content: &Content) {
        let skills = self.skills();
        // An agent without `load_skill` could not load a listed skill.
        if runtime.config().allows_tool("load_skill") {
            if let Some(text) = index_text(&skills.index().await) {
                runtime.register_provider(Arc::new(SkillContextProvider::index(text)));
            }
        }
        if let Some(slug) = requested_skill(content) {
            // By exact slug, reread and rehashed now: the route's check at
            // acceptance is advisory.
            let loaded = skills.load_slug(&slug).await;
            if let Err(problem) = &loaded {
                warn!(skill = %slug, problem = %problem, "a skill message's skill could not be loaded");
            }
            runtime.register_provider(Arc::new(SkillContextProvider::requested(requested_text(
                &slug,
                loaded.as_ref().map_err(String::as_str),
            ))));
        }
    }
}
