//! Isolated per-run copies of an agent runtime, and merging one run's changes
//! back into the canonical record. Hosts use this to run several rooms of one
//! agent at once without checking the canonical runtime out.

use std::collections::HashSet;

use super::{
    next_id, AgentRuntime, AgentRuntimeSnapshot, EVENT_TRIM_SLACK, MAX_RETAINED_EVENTS,
    NEXT_ROOM_ID,
};
use crate::agent::{AgentStatus, TokenUsage};
use crate::events::EngineEvent;
use crate::primitives::{Content, Message, TaskResult};

/// A fresh room id in the runtime's own `room-<ms>-<n>` format, for hosts that
/// must know a generated room before the run starts.
pub fn new_room_id() -> String {
    next_id("room", &NEXT_ROOM_ID)
}

/// Counters captured when a run starts on an isolated copy.
#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeRunBase {
    pub message_count: usize,
    pub event_total: usize,
    pub token_usage: TokenUsage,
    pub step_count: u64,
}

/// Everything one run added to its isolated copy.
#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeRunDelta {
    pub messages: Vec<Message>,
    /// The run's retained events (the newest ones if it exceeded the event cap).
    pub events: Vec<EngineEvent>,
    /// How many events the run recorded, including any no longer retained.
    pub event_total: usize,
    pub token_usage: TokenUsage,
    pub step_count: u64,
    pub last_task: Option<TaskResult<Content>>,
    pub status: AgentStatus,
}

/// What `apply_run_delta` replaced, so `revert_run_delta` can restore it.
///
/// Limits: reverting cannot bring back canonical events that `apply_run_delta`
/// trimmed under the [`MAX_RETAINED_EVENTS`] cap — a cosmetic loss, since
/// `event_total` and the other counters still revert exactly. And because
/// `revert_run_delta` tells "this delta's result is still current" apart from
/// "something else already replaced it" by comparing `TaskResult` values, two
/// runs that happen to finish with an identical reply and duration are
/// indistinguishable to it. Both limits are safe only because commits are
/// applied and reverted in LIFO order under the control-plane transaction.
#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeRunUndo {
    last_task: Option<TaskResult<Content>>,
    status: AgentStatus,
}

impl AgentRuntime {
    /// A snapshot for one run's isolated copy: the given room history, no
    /// retained events, and this record's state, counters, and last task.
    /// Restore it with `from_snapshot` and re-attach adapters.
    pub fn run_snapshot(&self, history: Vec<Message>) -> AgentRuntimeSnapshot {
        AgentRuntimeSnapshot {
            state: self.state.clone(),
            message_count: history.len(),
            messages: history,
            event_count: self.event_total,
            events: Vec::new(),
            last_task: self.last_task.clone(),
            step_count: self.step_counter,
        }
    }

    /// Counters to diff against once the run finishes.
    pub fn run_base(&self) -> RuntimeRunBase {
        RuntimeRunBase {
            message_count: self.messages.len(),
            event_total: self.event_total,
            token_usage: self.state.token_usage.clone(),
            step_count: self.step_counter,
        }
    }

    /// Everything recorded since `base`.
    pub fn run_delta_since(&self, base: &RuntimeRunBase) -> RuntimeRunDelta {
        let event_total = self.event_total.saturating_sub(base.event_total);
        let retained = event_total.min(self.events.len());
        RuntimeRunDelta {
            messages: self
                .messages
                .get(base.message_count..)
                .unwrap_or_default()
                .to_vec(),
            events: self.events[self.events.len() - retained..].to_vec(),
            event_total,
            token_usage: usage_since(&self.state.token_usage, &base.token_usage),
            step_count: self.step_counter.saturating_sub(base.step_count),
            last_task: self.last_task.clone(),
            status: self.state.status,
        }
    }

    /// Appends a run's messages and events and adds its usage and steps; the
    /// run's result and status become this record's. Events are not re-sent to
    /// the event listener: the copy already emitted them.
    pub fn apply_run_delta(&mut self, delta: &RuntimeRunDelta) -> RuntimeRunUndo {
        let undo = RuntimeRunUndo {
            last_task: self.last_task.clone(),
            status: self.state.status,
        };
        self.messages.extend(delta.messages.iter().cloned());
        self.events.extend(delta.events.iter().cloned());
        self.event_total += delta.event_total;
        if self.events.len() > MAX_RETAINED_EVENTS + EVENT_TRIM_SLACK {
            let excess = self.events.len() - MAX_RETAINED_EVENTS;
            self.events.drain(..excess);
        }
        self.apply_token_usage(&delta.token_usage);
        self.step_counter += delta.step_count;
        if delta.last_task.is_some() {
            self.last_task = delta.last_task.clone();
        }
        self.state.status = delta.status;
        undo
    }

    /// Removes exactly the delta's messages and events by id and subtracts its
    /// usage and steps. The last task and status go back to their earlier
    /// values only while no later delta has replaced this one's result.
    ///
    /// Limits: canonical events that `apply_run_delta` trimmed under the
    /// [`MAX_RETAINED_EVENTS`] cap cannot come back — `events` is filtered by
    /// id, so a trimmed event simply is not there to remove (cosmetic loss;
    /// `event_total` still goes back to its exact earlier value). Whether the
    /// last task and status are "still this run's" is decided by comparing
    /// `TaskResult` values, so two runs that finish with an identical reply
    /// and duration are indistinguishable — reverting either looks the same.
    /// Both limits are safe only because commits are applied and reverted in
    /// LIFO order under the control-plane transaction.
    pub fn revert_run_delta(&mut self, delta: &RuntimeRunDelta, undo: RuntimeRunUndo) {
        let message_ids: HashSet<&str> = delta
            .messages
            .iter()
            .map(|message| message.id.as_str())
            .collect();
        self.messages
            .retain(|message| !message_ids.contains(message.id.as_str()));
        let event_ids: HashSet<&str> = delta.events.iter().map(|event| event.id.as_str()).collect();
        self.events
            .retain(|event| !event_ids.contains(event.id.as_str()));
        self.event_total = self.event_total.saturating_sub(delta.event_total);
        subtract_usage(&mut self.state.token_usage, &delta.token_usage);
        self.step_counter = self.step_counter.saturating_sub(delta.step_count);
        let still_this_run = match &delta.last_task {
            Some(task) => self.last_task.as_ref() == Some(task),
            None => self.last_task == undo.last_task,
        };
        if still_this_run {
            self.last_task = undo.last_task;
            self.state.status = undo.status;
        }
    }

    /// Removes the transcript messages `keep` rejects and returns them in
    /// transcript order. Hosts use this to drop messages they keep elsewhere
    /// (for example ones mirrored to a history store) or a deleted room.
    /// Counters, events, usage, status, and the last task are untouched.
    pub fn retain_messages(&mut self, mut keep: impl FnMut(&Message) -> bool) -> Vec<Message> {
        let mut removed = Vec::new();
        let mut kept = Vec::with_capacity(self.messages.len());
        for message in self.messages.drain(..) {
            if keep(&message) {
                kept.push(message);
            } else {
                removed.push(message);
            }
        }
        self.messages = kept;
        removed
    }
}

fn usage_since(after: &TokenUsage, before: &TokenUsage) -> TokenUsage {
    TokenUsage {
        prompt_tokens: after.prompt_tokens.saturating_sub(before.prompt_tokens),
        completion_tokens: after
            .completion_tokens
            .saturating_sub(before.completion_tokens),
        total_tokens: after.total_tokens.saturating_sub(before.total_tokens),
        cached_prompt_tokens: after
            .cached_prompt_tokens
            .saturating_sub(before.cached_prompt_tokens),
        reasoning_tokens: after
            .reasoning_tokens
            .saturating_sub(before.reasoning_tokens),
    }
}

fn subtract_usage(total: &mut TokenUsage, delta: &TokenUsage) {
    total.prompt_tokens = total.prompt_tokens.saturating_sub(delta.prompt_tokens);
    total.completion_tokens = total
        .completion_tokens
        .saturating_sub(delta.completion_tokens);
    total.total_tokens = total.total_tokens.saturating_sub(delta.total_tokens);
    total.cached_prompt_tokens = total
        .cached_prompt_tokens
        .saturating_sub(delta.cached_prompt_tokens);
    total.reasoning_tokens = total
        .reasoning_tokens
        .saturating_sub(delta.reasoning_tokens);
}
