//! Automations from the coordinator (spec §9.3): the service the
//! companion's tools change them through, under this coordinator's
//! control-plane transaction.

use std::sync::Arc;

use super::AgentRunCoordinator;
use crate::schedules::AutomationService;

impl AgentRunCoordinator {
    pub(crate) fn automations(&self) -> AutomationService {
        AutomationService::new(Arc::clone(&self.state), self.control_plane_transactions())
    }
}
