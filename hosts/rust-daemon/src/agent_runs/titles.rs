//! Naming a new chat after its first completed reply (spec §12.3).

use std::collections::HashSet;
use std::sync::Arc;

use anima_core::{Content, DataValue, Message, MessageRole, TaskResult};
use tracing::warn;

use super::AgentRunCoordinator;
use crate::live::LiveEventBody;
use crate::runs::RunChangeSet;
use crate::sessions::titles::{auto_title_enabled, generate_title};
use crate::sessions::{SessionKind, TitleSource};

/// Ruling 2 (M3 pre-flight audit, M19): an assistant message a run's own
/// stop or failure left behind never blocks AI titles for good, because that
/// run never reached `RunStatus::Completed` and so never spent the
/// session's one shot at a first-message title.
fn is_unusable_reply(message: &Message) -> bool {
    message.content.metadata.as_ref().is_some_and(|metadata| {
        matches!(
            metadata.get(anima_core::STOPPED_METADATA_KEY),
            Some(DataValue::Bool(true))
        ) || matches!(
            metadata.get(anima_core::INCOMPLETE_METADATA_KEY),
            Some(DataValue::Bool(true))
        )
    })
}

impl AgentRunCoordinator {
    /// Starts a background title for the session of a run that just
    /// committed; nothing it does can affect the run.
    pub(crate) fn title_after_first_reply(
        &self,
        agent_id: &str,
        session_id: &str,
        room_id: &str,
        change_set: &RunChangeSet,
        result: &TaskResult<Content>,
    ) {
        let Some(reply) = result.data.as_ref().map(|content| content.text.clone()) else {
            return;
        };
        let Some(first) = change_set
            .delta
            .messages
            .iter()
            .find(|message| message.role == MessageRole::User)
            .map(|message| message.content.text.clone())
        else {
            return;
        };
        let run_messages: HashSet<String> = change_set.message_ids.iter().cloned().collect();
        let coordinator = self.clone();
        let (agent_id, session_id, room_id) = (
            agent_id.to_string(),
            session_id.to_string(),
            room_id.to_string(),
        );
        tokio::spawn(async move {
            coordinator
                .title_session(agent_id, session_id, room_id, run_messages, first, reply)
                .await;
        });
    }

    async fn title_session(
        &self,
        agent_id: String,
        session_id: String,
        room_id: String,
        run_messages: HashSet<String>,
        first: String,
        reply: String,
    ) {
        let (adapter, config) = {
            let guard = self.state.read().await;
            if !guard.generated_titles {
                return;
            }
            let (Some(runtime), Some(session)) = (
                guard.agents.get(&agent_id),
                guard.sessions.get(&agent_id, &session_id),
            ) else {
                return;
            };
            if session.kind != SessionKind::Chat
                || session.title_source != TitleSource::FirstMessage
                || !auto_title_enabled(runtime.config())
            {
                return;
            }
            // Only the first completed reply names the chat.
            let earlier_reply = runtime.messages().iter().any(|message| {
                message.room_id == room_id
                    && message.role == MessageRole::Assistant
                    && !run_messages.contains(&message.id)
                    && !is_unusable_reply(message)
            });
            if earlier_reply {
                return;
            }
            (Arc::clone(&guard.model_adapter), runtime.config().clone())
        };
        let title = match tokio::time::timeout(
            self.title_timeout,
            generate_title(adapter.as_ref(), &config, &first, &reply),
        )
        .await
        {
            Ok(Ok(title)) => title,
            Ok(Err(error)) => {
                warn!(agent_id = %agent_id, session_id = %session_id, error = %error, "could not title the chat");
                return;
            }
            Err(_) => {
                warn!(agent_id = %agent_id, session_id = %session_id, "the title call timed out");
                return;
            }
        };
        let transaction = self.control_plane_transaction().await;
        let (previous, persist) = {
            let mut guard = self.state.write().await;
            let Some(previous) =
                guard
                    .sessions
                    .apply_generated_title(&agent_id, &session_id, &title)
            else {
                // Renamed meanwhile (spec §12.3).
                return;
            };
            (previous, guard.control_plane_persist_request())
        };
        if let Err(error) = persist.save().await {
            let mut guard = self.state.write().await;
            if let Some(session) = guard.sessions.get_mut(&agent_id, &session_id) {
                if session.title_source == TitleSource::Generated && session.title == title {
                    (session.title, session.title_source) = previous;
                }
            }
            warn!(agent_id = %agent_id, session_id = %session_id, error = %error, "could not save the chat's title");
            return;
        }
        drop(transaction);
        self.state.read().await.publish_session_event(
            &agent_id,
            &session_id,
            LiveEventBody::SessionUpdated,
        );
    }
}
