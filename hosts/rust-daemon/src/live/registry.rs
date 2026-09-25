//! Runs in flight (spec §6): their controls and what a stream joining mid-run
//! needs — the current step, its text so far, and the tool cards.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use anima_core::{RunControl, TokenUsage};
use serde::Serialize;

use super::{MAX_LIVE_TOOL_CARDS, MAX_SNAPSHOT_TEXT_BYTES};
use crate::runs::{RunStepUsage, MAX_RUN_STEPS, MAX_RUN_TOOLS_STARTED};

/// One tool card of a run.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LiveToolView {
    pub(crate) step_id: String,
    pub(crate) tool_call_id: String,
    pub(crate) name: String,
    pub(crate) arguments_preview: String,
    pub(crate) arguments_truncated: bool,
    /// `running`, `success`, or `error`.
    pub(crate) status: String,
    pub(crate) duration_ms: Option<u64>,
    pub(crate) result_preview: Option<String>,
    pub(crate) truncated: bool,
}

/// What a snapshot shows of one run.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct LiveRunView {
    pub(crate) step_id: Option<String>,
    /// The newest `MAX_SNAPSHOT_TEXT_BYTES` of the step's text.
    pub(crate) text: String,
    /// UTF-16 units of the step's text dropped before `text`.
    pub(crate) text_offset: u64,
    /// The newest `MAX_LIVE_TOOL_CARDS` tool cards, oldest first.
    pub(crate) tools: Vec<LiveToolView>,
}

#[derive(Debug)]
struct LiveRunState {
    control: RunControl,
    view: LiveRunView,
    /// UTF-16 units of `view.text`, kept so an append costs only its own
    /// length rather than a recount of the retained text.
    text_units: u64,
    steps: Vec<RunStepUsage>,
    /// Distinct tool names, noted here instead of under the state write lock
    /// (M1 carry-forward); saves and reads merge them into the ledger record.
    tools_started: Vec<String>,
    /// Idempotency keys of the steers this run accepted, with their text.
    steer_keys: HashMap<String, String>,
}

#[derive(Debug, Default)]
pub(crate) struct LiveRuns {
    runs: Mutex<HashMap<String, LiveRunState>>,
}

fn utf16_len(text: &str) -> u64 {
    text.encode_utf16().count() as u64
}

impl LiveRuns {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, LiveRunState>> {
        self.runs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Registers a run and returns its control; a run registered already
    /// (accepted before it started) keeps its control.
    pub(crate) fn register(&self, run_id: &str) -> RunControl {
        self.lock()
            .entry(run_id.to_string())
            .or_insert_with(|| LiveRunState {
                control: RunControl::new(),
                view: LiveRunView::default(),
                text_units: 0,
                steps: Vec::new(),
                tools_started: Vec::new(),
                steer_keys: HashMap::new(),
            })
            .control
            .clone()
    }

    pub(crate) fn control(&self, run_id: &str) -> Option<RunControl> {
        self.lock().get(run_id).map(|run| run.control.clone())
    }

    /// Forgets a finished run; false when it was not registered.
    pub(crate) fn remove(&self, run_id: &str) -> bool {
        self.lock().remove(run_id).is_some()
    }

    pub(crate) fn view(&self, run_id: &str) -> Option<LiveRunView> {
        self.lock().get(run_id).map(|run| run.view.clone())
    }

    pub(crate) fn start_step(&self, run_id: &str, step_id: &str) {
        if let Some(run) = self.lock().get_mut(run_id) {
            run.view.step_id = Some(step_id.to_string());
            run.view.text.clear();
            run.view.text_offset = 0;
            run.text_units = 0;
        }
    }

    /// Appends streamed text to the current step; returns its UTF-16 offset
    /// within the step, or `None` for an unknown run.
    pub(crate) fn append_text(&self, run_id: &str, text: &str) -> Option<u64> {
        let mut runs = self.lock();
        let run = runs.get_mut(run_id)?;
        let offset = run.view.text_offset + run.text_units;
        run.view.text.push_str(text);
        run.text_units += utf16_len(text);
        if run.view.text.len() > MAX_SNAPSHOT_TEXT_BYTES {
            let mut cut = run.view.text.len() - MAX_SNAPSHOT_TEXT_BYTES;
            while !run.view.text.is_char_boundary(cut) {
                cut += 1;
            }
            let dropped = utf16_len(&run.view.text[..cut]);
            run.view.text_offset += dropped;
            run.text_units -= dropped;
            run.view.text.drain(..cut);
        }
        Some(offset)
    }

    pub(crate) fn tool_started(&self, run_id: &str, tool: LiveToolView) {
        if let Some(run) = self.lock().get_mut(run_id) {
            if run.tools_started.len() < MAX_RUN_TOOLS_STARTED
                && !run.tools_started.iter().any(|name| name == &tool.name)
            {
                run.tools_started.push(tool.name.clone());
            }
            let tools = &mut run.view.tools;
            tools.retain(|known| known.tool_call_id != tool.tool_call_id);
            tools.push(tool);
            if tools.len() > MAX_LIVE_TOOL_CARDS {
                tools.drain(..tools.len() - MAX_LIVE_TOOL_CARDS);
            }
        }
    }

    pub(crate) fn tool_finished(
        &self,
        run_id: &str,
        tool_call_id: &str,
        status: &'static str,
        duration_ms: u64,
        result_preview: String,
        truncated: bool,
    ) {
        if let Some(tool) = self.lock().get_mut(run_id).and_then(|run| {
            run.view
                .tools
                .iter_mut()
                .find(|tool| tool.tool_call_id == tool_call_id)
        }) {
            tool.status = status.to_string();
            tool.duration_ms = Some(duration_ms);
            tool.result_preview = Some(result_preview);
            tool.truncated = truncated;
        }
    }

    /// Keeps one model call's usage (spec §4.1 `steps`, at most 50).
    pub(crate) fn record_step_usage(&self, run_id: &str, step_id: &str, usage: TokenUsage) {
        if let Some(run) = self.lock().get_mut(run_id) {
            if run.steps.len() < MAX_RUN_STEPS {
                run.steps.push(RunStepUsage {
                    step_id: step_id.to_string(),
                    usage,
                });
            }
        }
    }

    pub(crate) fn steps(&self, run_id: &str) -> Vec<RunStepUsage> {
        self.lock()
            .get(run_id)
            .map(|run| run.steps.clone())
            .unwrap_or_default()
    }

    /// Distinct tools the run started, in first-use order.
    pub(crate) fn tools_started(&self, run_id: &str) -> Vec<String> {
        self.lock()
            .get(run_id)
            .map(|run| run.tools_started.clone())
            .unwrap_or_default()
    }

    /// Remembers a steer's idempotency key and text for the run it joined
    /// (Task 9), so a retried steer is answered instead of sent twice.
    #[allow(dead_code)] // M3 Task 9 records steer keys.
    pub(crate) fn note_steer_key(&self, run_id: &str, key: &str, text: &str) {
        if let Some(run) = self.lock().get_mut(run_id) {
            run.steer_keys.insert(key.to_string(), text.to_string());
        }
    }

    /// The text of the steer this run accepted with `key`.
    #[allow(dead_code)] // M3 Task 9 answers retried steers.
    pub(crate) fn steer_text(&self, run_id: &str, key: &str) -> Option<String> {
        self.lock()
            .get(run_id)
            .and_then(|run| run.steer_keys.get(key).cloned())
    }
}
