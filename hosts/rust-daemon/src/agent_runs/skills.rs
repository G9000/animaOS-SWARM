//! Skills from the coordinator (spec §8): the service its tools and the
//! routes use. M5 Task 9 adds what a run sees.

use std::sync::Arc;

use super::AgentRunCoordinator;
use crate::skills::SkillService;

impl AgentRunCoordinator {
    /// The workspace's skills, changed under this coordinator's
    /// control-plane transaction.
    pub(crate) fn skills(&self) -> SkillService {
        SkillService::new(Arc::clone(&self.state), self.control_plane_transactions())
    }
}
