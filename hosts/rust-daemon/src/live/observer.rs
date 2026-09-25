//! Turns one run's frames into live events (spec §4.5, §6). Streamed text is
//! coalesced and published every 50 ms or 512 bytes, whichever comes first.
//! Every other event of the run is published after the buffered text, under
//! the same lock as the timer's flush, so a client always sees a step's text
//! before that step's tool cards and nothing is ever reordered.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use anima_core::primitives::now_millis;
use anima_core::{DataValue, RunControl, RunFrame, RunObserver};

use super::events::{preview, run_status_event, LiveEvent, LiveEventBody};
use super::fanout::LiveHub;
use super::registry::LiveToolView;
use super::{DELTA_FLUSH_BYTES, DELTA_FLUSH_MS};
use crate::routes::data_value_to_json;
use crate::runs::RunRecord;

/// Text ready to publish as one `step.delta`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DeltaChunk {
    pub(crate) step_id: String,
    pub(crate) offset: u64,
    pub(crate) text: String,
}

#[derive(Debug)]
struct PendingDelta {
    chunk: DeltaChunk,
    since_ms: u64,
}

/// Buffers a step's streamed text until it reaches `DELTA_FLUSH_BYTES` or is
/// `DELTA_FLUSH_MS` old.
#[derive(Debug, Default)]
pub(crate) struct DeltaCoalescer {
    pending: Option<PendingDelta>,
}

impl DeltaCoalescer {
    /// Buffers `text`, which starts at UTF-16 `offset` of `step_id`, and
    /// returns what is due now: the previous step's text when the step
    /// changed, then the buffer once it is full or old enough.
    pub(crate) fn push(
        &mut self,
        step_id: &str,
        offset: u64,
        text: &str,
        now_ms: u64,
    ) -> Vec<DeltaChunk> {
        let mut due = Vec::new();
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.chunk.step_id != step_id)
        {
            due.extend(self.take());
        }
        match &mut self.pending {
            Some(pending) => pending.chunk.text.push_str(text),
            None => {
                self.pending = Some(PendingDelta {
                    chunk: DeltaChunk {
                        step_id: step_id.to_string(),
                        offset,
                        text: text.to_string(),
                    },
                    since_ms: now_ms,
                });
            }
        }
        if self.pending.as_ref().is_some_and(|pending| {
            pending.chunk.text.len() >= DELTA_FLUSH_BYTES
                || now_ms.saturating_sub(pending.since_ms) >= DELTA_FLUSH_MS
        }) {
            due.extend(self.take());
        }
        due
    }

    /// Whatever is buffered.
    pub(crate) fn take(&mut self) -> Option<DeltaChunk> {
        self.pending.take().map(|pending| pending.chunk)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.pending.is_none()
    }
}

struct RunInner {
    hub: LiveHub,
    run_id: String,
    agent_id: String,
    session_id: String,
    parent_agent_id: Option<String>,
    coalescer: Mutex<DeltaCoalescer>,
    timer_armed: AtomicBool,
}

impl RunInner {
    fn coalescer(&self) -> MutexGuard<'_, DeltaCoalescer> {
        self.coalescer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn event(&self, body: LiveEventBody) -> LiveEvent {
        LiveEvent::new(&self.agent_id, body)
            .session(&self.session_id)
            .run(&self.run_id)
    }

    fn send(&self, event: LiveEvent) {
        self.hub.publish(event, self.parent_agent_id.as_deref());
    }

    fn send_chunk(&self, chunk: DeltaChunk) {
        self.send(self.event(LiveEventBody::StepDelta {
            step_id: chunk.step_id,
            offset: chunk.offset,
            text: chunk.text,
        }));
    }

    /// Buffered text, then `event`, both under the coalescer lock.
    fn publish_after_flush(&self, event: Option<LiveEvent>) {
        let mut coalescer = self.coalescer();
        if let Some(chunk) = coalescer.take() {
            self.send_chunk(chunk);
        }
        if let Some(event) = event {
            self.send(event);
        }
    }

    fn text(self: &Arc<Self>, step_id: &str, text: &str) {
        // Recorded for snapshots before it is published, so a stream that
        // joins in between gets it from its snapshot and drops the overlap.
        let Some(offset) = self.hub.runs().append_text(&self.run_id, text) else {
            return;
        };
        let mut coalescer = self.coalescer();
        for chunk in coalescer.push(step_id, offset, text, now_millis()) {
            self.send_chunk(chunk);
        }
        let buffered = !coalescer.is_empty();
        drop(coalescer);
        if buffered {
            self.arm_timer();
        }
    }

    /// Flushes buffered text `DELTA_FLUSH_MS` from now; one timer at a time.
    fn arm_timer(self: &Arc<Self>) {
        if self.timer_armed.swap(true, Ordering::AcqRel) {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            self.timer_armed.store(false, Ordering::Release);
            return;
        };
        let inner = Arc::clone(self);
        handle.spawn(async move {
            tokio::time::sleep(Duration::from_millis(DELTA_FLUSH_MS)).await;
            inner.timer_armed.store(false, Ordering::Release);
            inner.publish_after_flush(None);
        });
    }
}

struct RunEvents {
    inner: Arc<RunInner>,
}

impl RunObserver for RunEvents {
    fn on_frame(&self, frame: RunFrame) {
        let inner = &self.inner;
        match frame {
            RunFrame::StepStarted { step_id } => {
                inner.publish_after_flush(None);
                inner.hub.runs().start_step(&inner.run_id, &step_id);
            }
            RunFrame::TextDelta { step_id, text } => inner.text(&step_id, &text),
            RunFrame::StepUsage { step_id, usage } => {
                inner
                    .hub
                    .runs()
                    .record_step_usage(&inner.run_id, &step_id, usage);
            }
            RunFrame::StepFinished { .. } => inner.publish_after_flush(None),
            RunFrame::ToolStarted { step_id, tool_call } => {
                let arguments =
                    data_value_to_json(&DataValue::Object(tool_call.args.clone())).to_string();
                let (arguments_preview, arguments_truncated) = preview(&arguments);
                inner.hub.runs().tool_started(
                    &inner.run_id,
                    LiveToolView {
                        step_id: step_id.clone(),
                        tool_call_id: tool_call.id.clone(),
                        name: tool_call.name.clone(),
                        arguments_preview: arguments_preview.clone(),
                        arguments_truncated,
                        status: "running".into(),
                        duration_ms: None,
                        result_preview: None,
                        truncated: false,
                    },
                );
                inner.publish_after_flush(Some(inner.event(LiveEventBody::ToolStarted {
                    step_id,
                    tool_call_id: tool_call.id,
                    name: tool_call.name,
                    arguments_preview,
                    arguments_truncated,
                })));
            }
            RunFrame::ToolFinished {
                step_id,
                tool_call_id,
                name,
                status,
                duration_ms,
                result,
                recovered,
            } => {
                let (result_preview, truncated) = preview(&result);
                inner.hub.runs().tool_finished(
                    &inner.run_id,
                    &tool_call_id,
                    status.as_str(),
                    duration_ms,
                    result_preview.clone(),
                    truncated,
                );
                inner.publish_after_flush(Some(inner.event(LiveEventBody::ToolFinished {
                    step_id,
                    tool_call_id,
                    name,
                    status: status.as_str(),
                    duration_ms,
                    result_preview,
                    truncated,
                    recovered,
                })));
            }
            RunFrame::Steered { message_id, text } => {
                inner.publish_after_flush(Some(
                    inner.event(LiveEventBody::RunSteered { message_id, text }),
                ));
            }
        }
    }
}

/// One run's link to the live hub from its start to its end: its control,
/// its observer, and its events. Dropping it forgets the run's live state.
pub(crate) struct LiveRun {
    inner: Arc<RunInner>,
    control: RunControl,
}

impl LiveRun {
    /// Registers `record`'s run, keeping the control an accepted run was
    /// given at acceptance; its events also reach `parent_agent_id`'s stream.
    pub(crate) fn register(
        hub: LiveHub,
        record: &RunRecord,
        parent_agent_id: Option<String>,
    ) -> Self {
        let control = hub.runs().register(&record.id);
        Self {
            inner: Arc::new(RunInner {
                hub,
                run_id: record.id.clone(),
                agent_id: record.agent_id.clone(),
                session_id: record.session_id.clone(),
                parent_agent_id,
                coalescer: Mutex::new(DeltaCoalescer::default()),
                timer_armed: AtomicBool::new(false),
            }),
            control,
        }
    }

    pub(crate) fn control(&self) -> RunControl {
        self.control.clone()
    }

    pub(crate) fn observer(&self) -> Arc<dyn RunObserver> {
        Arc::new(RunEvents {
            inner: Arc::clone(&self.inner),
        })
    }

    /// Publishes `event` after any buffered text.
    pub(crate) fn publish(&self, event: LiveEvent) {
        self.inner.publish_after_flush(Some(event));
    }

    /// Publishes the lifecycle event of `record`'s status.
    pub(crate) fn publish_record(&self, record: &RunRecord) {
        self.publish(run_status_event(record));
    }

    /// Publishes buffered text now.
    pub(crate) fn flush(&self) {
        self.inner.publish_after_flush(None);
    }

    /// An event about this run's session.
    pub(crate) fn session_event(&self, body: LiveEventBody) -> LiveEvent {
        LiveEvent::new(&self.inner.agent_id, body).session(&self.inner.session_id)
    }
}

impl Drop for LiveRun {
    fn drop(&mut self) {
        // A run that ends early (an aborted task) sends its buffered text
        // now, before `InFlightRunGuard`'s `run.failed`, rather than from a
        // timer after it.
        self.inner.publish_after_flush(None);
        self.inner.hub.runs().remove(&self.inner.run_id);
    }
}
