//! Conversation search for an agent's own runs (spec §7.1).

use super::AgentRunCoordinator;
use crate::sessions::views::{self, SessionView};

impl AgentRunCoordinator {
    pub(crate) async fn search_conversations(
        &self,
        agent_id: &str,
        exclude_session: Option<&str>,
        query: &str,
        limit: usize,
    ) -> Option<Vec<SessionView>> {
        views::search_conversations(&self.state, agent_id, exclude_session, query, limit).await
    }
}
