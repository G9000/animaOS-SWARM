//! Live, non-recorded frames of a run (spec §4.5). Hosts stream a run's
//! progress to clients; frames never enter the stored event log, and every
//! model call still ends as an ordinary recorded message.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::agent::TokenUsage;
use crate::model::{ModelGenerateResponse, ModelStreamFrame, ModelStreamSink, ToolCall};
use crate::primitives::{Content, LockRecover, TaskResult, TaskStatus};

/// The host run a message belongs to (set only when the host set a run id).
pub const RUN_ID_METADATA_KEY: &str = "runId";
/// The model call (`<runId>:<n>`) an assistant or tool message came from.
pub const STEP_ID_METADATA_KEY: &str = "stepId";
/// An earlier draft an evaluator asked to revise (spec §4.5).
pub const REVISED_METADATA_KEY: &str = "revised";
/// Text a model call streamed before it failed or could not be accepted.
pub const INCOMPLETE_METADATA_KEY: &str = "incomplete";
/// `"success"` or `"error"` on a tool result message.
pub const TOOL_STATUS_METADATA_KEY: &str = "toolStatus";
/// How long a tool took, in milliseconds, on a tool result message.
pub const TOOL_DURATION_METADATA_KEY: &str = "toolDurationMs";
/// A stream that ended without its final response fails the run with this.
pub const MODEL_STREAM_WITHOUT_FINAL: &str = "model stream ended without a final response";

/// One live frame of a run. Frames are never recorded.
#[derive(Clone, Debug, PartialEq)]
pub enum RunFrame {
    /// A model call starts.
    StepStarted { step_id: String },
    /// Text the model call streamed.
    TextDelta { step_id: String, text: String },
    /// Provider-reported usage of one model call.
    StepUsage { step_id: String, usage: TokenUsage },
    /// The model call ended; `message_id` is the message it recorded, if any.
    StepFinished {
        step_id: String,
        message_id: Option<String>,
    },
    /// A tool of the step starts (or is recovered from persistence).
    ToolStarted {
        step_id: String,
        tool_call: ToolCall,
    },
    /// A tool of the step finished.
    ToolFinished {
        step_id: String,
        tool_call_id: String,
        name: String,
        status: TaskStatus,
        duration_ms: u64,
        /// The result text the model sees (the error text on failure).
        result: String,
        recovered: bool,
    },
    /// A steered owner message joined the conversation (spec §4.7).
    Steered { message_id: String, text: String },
}

/// Receives a run's live frames. The runtime calls it inline, so an
/// implementation must return quickly and never block on I/O.
pub trait RunObserver: Send + Sync {
    fn on_frame(&self, frame: RunFrame);
}

/// The id of a run's `step`-th model call, counting from 1 (spec §4.5).
pub fn run_step_id(run_id: &str, step: u64) -> String {
    format!("{run_id}:{step}")
}

/// The text a tool result frame carries: the content on success, the error
/// otherwise.
pub(crate) fn tool_result_text(result: &TaskResult<Content>) -> String {
    match result.status {
        TaskStatus::Success => result
            .data
            .as_ref()
            .map(|content| content.text.clone())
            .unwrap_or_default(),
        TaskStatus::Error => result.error.clone().unwrap_or_default(),
    }
}

/// The sink one streamed model call writes into: deltas reach the observer
/// as they arrive, the streamed text is kept for a call that never finishes,
/// and the final response is captured for the runtime. It owns what it
/// holds, so the runtime can keep recording messages while it lives.
pub(crate) struct StepSink {
    observer: Option<Arc<dyn RunObserver>>,
    step_id: String,
    text: Mutex<String>,
    response: Mutex<Option<ModelGenerateResponse>>,
}

impl StepSink {
    pub(crate) fn new(observer: Option<Arc<dyn RunObserver>>, step_id: String) -> Self {
        Self {
            observer,
            step_id,
            text: Mutex::new(String::new()),
            response: Mutex::new(None),
        }
    }

    /// Everything the call streamed so far.
    pub(crate) fn streamed_text(&self) -> String {
        self.text.lock_recover().clone()
    }

    pub(crate) fn take_response(&self) -> Option<ModelGenerateResponse> {
        self.response.lock_recover().take()
    }
}

#[async_trait]
impl ModelStreamSink for StepSink {
    async fn emit(&self, frame: ModelStreamFrame) -> Result<(), String> {
        match frame {
            ModelStreamFrame::TextDelta(text) => {
                if text.is_empty() {
                    return Ok(());
                }
                self.text.lock_recover().push_str(&text);
                if let Some(observer) = &self.observer {
                    observer.on_frame(RunFrame::TextDelta {
                        step_id: self.step_id.clone(),
                        text,
                    });
                }
            }
            ModelStreamFrame::Final(response) => {
                *self.response.lock_recover() = Some(response);
            }
        }
        Ok(())
    }
}
