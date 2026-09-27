//! Compacting a session's history into its summary (spec §5.4).

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::Arc;

use anima_core::primitives::now_millis;
use anima_core::{AgentConfig, CancelSignal, Content, Message};
use tracing::warn;

use super::AgentRunCoordinator;
use crate::live::{LiveEvent, LiveEventBody, LiveRun};
use crate::runs::RunRecord;
use crate::sessions::compaction::{
    auto_compact_enabled, compaction_input_chars, pruned_turns, summarize, PrunedSpan,
    COMPACTING_PHASE,
};
use crate::sessions::context::{mark_context_trimmed, newest_left_out, ContextBudget};
use crate::sessions::{SessionCompactionError, SessionSummary};
use crate::state::{RunBuild, RunContextReport};

/// A provider error is cut to this many characters on the session.
const MAX_COMPACTION_ERROR_CHARS: usize = 500;
const SESSION_GONE: &str = "The session no longer exists";

/// What a run brings to its automatic compaction.
struct RunCompaction<'a> {
    room_id: &'a str,
    input: &'a Content,
    /// The run's own configuration: its provider and model write the
    /// summary, and the rebuilt runtime keeps it.
    config: &'a AgentConfig,
    cancel: CancelSignal,
}

/// How a compaction ended.
enum Compacted {
    /// Every left-out turn is already covered.
    Nothing,
    /// The summary is saved, with the run's rebuilt runtime.
    Saved(Option<Box<RunBuild>>),
    /// The session records this error (unless it is gone).
    Failed(String),
    /// A stop ended the run first; nothing changed.
    Stopped,
}

fn compaction_error(message: &str, at_ms: u64) -> SessionCompactionError {
    SessionCompactionError {
        message: message.chars().take(MAX_COMPACTION_ERROR_CHARS).collect(),
        at_ms,
    }
}

impl AgentRunCoordinator {
    /// Folds `dropped` and the session's pruned turns no summary covers into
    /// its summary, through the newest of them, and saves it with
    /// `contextTrimmed` and `compactionError` cleared; when the call or the
    /// save fails, the session records the error instead. The caller holds
    /// the session's room, so no run starts meanwhile.
    pub(crate) async fn compact_session(
        &self,
        agent_id: &str,
        session_id: &str,
        dropped: &[Message],
    ) -> Result<(), String> {
        match self.compact(agent_id, session_id, dropped, None).await {
            Compacted::Failed(error) => Err(error),
            Compacted::Nothing | Compacted::Saved(_) | Compacted::Stopped => Ok(()),
        }
    }

    /// Spec §5.4, between a run's start save and its model work: when the
    /// run's selection leaves out turns no summary covers and `autoCompact`
    /// is not `false`, announces `run.progress { phase: "compacting" }` and
    /// compacts them. Returns the run rebuilt from the saved summary, whose
    /// selection's `contextTrimmed` was saved with it; `None` leaves the run
    /// as it was built, after a failure (recorded as `compactionError`) or a
    /// stop (the run then stops at its first checkpoint). `context.dropped`
    /// is released either way (Task 11 review carry).
    pub(super) async fn compact_before_run(
        &self,
        live_run: &LiveRun,
        started: &RunRecord,
        room_id: &str,
        input: &Content,
        config: &AgentConfig,
        context: &mut RunContextReport,
    ) -> Option<RunBuild> {
        let dropped = std::mem::take(&mut context.dropped);
        let cancel = live_run.control().cancel;
        if context.trimmed_through.is_none()
            || !auto_compact_enabled(config)
            || cancel.is_cancelled()
        {
            return None;
        }
        live_run.publish(LiveEvent::for_run(
            started,
            LiveEventBody::RunProgress {
                phase: COMPACTING_PHASE,
            },
        ));
        let run = RunCompaction {
            room_id,
            input,
            config,
            cancel,
        };
        match self
            .compact(&started.agent_id, &started.session_id, &dropped, Some(run))
            .await
        {
            Compacted::Saved(Some(mut rebuilt)) => {
                rebuilt.context.dropped = Vec::new();
                Some(*rebuilt)
            }
            Compacted::Failed(error) => {
                warn!(agent_id = %started.agent_id, session_id = %started.session_id, error = %error, "session compaction failed; the run goes on with its trimmed context");
                None
            }
            Compacted::Saved(None) | Compacted::Nothing | Compacted::Stopped => None,
        }
    }

    /// One compaction. The history-store read and the model call run with
    /// no lock held; the save takes the control-plane transaction, then the
    /// state lock.
    async fn compact(
        &self,
        agent_id: &str,
        session_id: &str,
        dropped: &[Message],
        run: Option<RunCompaction<'_>>,
    ) -> Compacted {
        let (adapter, config, previous, pruned, store, through) = {
            let guard = self.state.read().await;
            let Some(runtime) = guard.agents.get(agent_id) else {
                return Compacted::Failed("The companion no longer exists".into());
            };
            let Some(session) = guard.sessions.get(agent_id, session_id) else {
                return Compacted::Failed(SESSION_GONE.into());
            };
            // The hot history is copied only to place a pruned span.
            let pruned = match session.pruned_through {
                Some(_) => PrunedSpan::uncovered(
                    session,
                    &guard.model_visible_history(agent_id, session.room_id()),
                ),
                None => None,
            };
            let Some(through) = newest_left_out(
                dropped.last(),
                pruned.as_ref().map(|pruned| &pruned.through),
            ) else {
                return Compacted::Nothing;
            };
            (
                Arc::clone(&guard.model_adapter),
                run.as_ref()
                    .map_or_else(|| runtime.config().clone(), |run| run.config.clone()),
                session.summary.clone(),
                pruned,
                guard.history.store(),
                through,
            )
        };
        let budget_tokens = ContextBudget::for_config(&config).budget_tokens;
        let work = async {
            let turns: Cow<'_, [Message]> = match &pruned {
                None => Cow::Borrowed(dropped),
                Some(span) => {
                    let mut turns = pruned_turns(
                        store.as_ref(),
                        agent_id,
                        session_id,
                        span,
                        compaction_input_chars(budget_tokens),
                    )
                    .await
                    .map_err(|error| format!("Earlier turns could not be read: {error}"))?;
                    // A turn pruned after the caller read it is summarized once.
                    let hot: HashSet<&str> =
                        dropped.iter().map(|message| message.id.as_str()).collect();
                    turns.retain(|message| !hot.contains(message.id.as_str()));
                    turns.extend_from_slice(dropped);
                    Cow::Owned(turns)
                }
            };
            if turns.is_empty() {
                return Ok(None);
            }
            let text = summarize(
                adapter.as_ref(),
                &config,
                previous.as_ref().map(|summary| summary.text.as_str()),
                &turns,
                budget_tokens,
            )
            .await?;
            Ok::<_, String>(Some((text, turns.len())))
        };
        // Controller ruling 3 (audit M7): a stop ends the run at once.
        let outcome: Result<Option<(String, usize)>, String> = match &run {
            Some(run) => {
                tokio::select! {
                    outcome = work => outcome,
                    () = run.cancel.cancelled() => return Compacted::Stopped,
                }
            }
            None => work.await,
        };
        let summary = match outcome {
            Ok(None) => return Compacted::Nothing,
            Ok(Some(summary)) => Ok(summary),
            Err(error) => Err(error),
        };

        let now_ms = now_millis();
        let transaction = self.control_plane_transaction().await;
        let (before, rebuilt, persist) = {
            let mut guard = self.state.write().await;
            let Some(session) = guard.sessions.get_mut(agent_id, session_id) else {
                return Compacted::Failed(SESSION_GONE.into());
            };
            let before = (session.summary.clone(), session.context_trimmed.clone());
            let mut rebuilt = None;
            match &summary {
                Ok((text, folded)) => {
                    session.summary = Some(SessionSummary {
                        text: text.clone(),
                        through_message_id: through,
                        created_at_ms: now_ms,
                        source_message_count: previous
                            .as_ref()
                            .map_or(0, |summary| summary.source_message_count)
                            + folded,
                    });
                    session.compaction_error = None;
                    // The run goes on from the new summary (controller
                    // ruling 3): what its selection still leaves out is the
                    // session's `contextTrimmed`, saved with the summary.
                    rebuilt = run.as_ref().and_then(|run| {
                        let mut build =
                            guard.build_run_runtime(agent_id, run.room_id, run.input)?;
                        // A PATCH meanwhile applies to later runs (spec §4.4).
                        build.runtime.replace_config(run.config.clone());
                        Some(build)
                    });
                    if let Some(session) = guard.sessions.get_mut(agent_id, session_id) {
                        mark_context_trimmed(
                            session,
                            rebuilt.as_ref().and_then(|build: &RunBuild| {
                                build.context.trimmed_through.as_deref()
                            }),
                            now_ms,
                        );
                    }
                }
                Err(error) => session.compaction_error = Some(compaction_error(error, now_ms)),
            }
            (before, rebuilt, guard.control_plane_persist_request())
        };
        if let Err(error) = persist.save().await {
            let message = match &summary {
                Ok(_) => format!("The summary could not be saved: {error}"),
                Err(error) => error.clone(),
            };
            let mut guard = self.state.write().await;
            if let Some(session) = guard.sessions.get_mut(agent_id, session_id) {
                (session.summary, session.context_trimmed) = before;
                // Kept unsaved: the next save persists it.
                session.compaction_error = Some(compaction_error(&message, now_ms));
            }
            return Compacted::Failed(message);
        }
        drop(transaction);
        self.state.read().await.publish_session_event(
            agent_id,
            session_id,
            LiveEventBody::SessionUpdated,
        );
        match summary {
            Ok(_) => Compacted::Saved(rebuilt.map(Box::new)),
            Err(error) => Compacted::Failed(error),
        }
    }
}
