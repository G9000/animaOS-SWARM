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
    /// What `view` shows, except that `view.text` keeps up to twice
    /// `MAX_SNAPSHOT_TEXT_BYTES` (S2-J); `view` cuts it to the cap.
    view: LiveRunView,
    /// UTF-16 units of `view.text`, kept so an append costs only its own
    /// length rather than a recount of the retained text.
    text_units: u64,
    steps: Vec<RunStepUsage>,
    /// Distinct tool names, noted here instead of under the state write lock
    /// (M1 carry-forward); saves and reads merge them into the ledger record.
    tools_started: Vec<String>,
}

#[derive(Debug, Default)]
pub(crate) struct LiveRuns {
    runs: Mutex<HashMap<String, LiveRunState>>,
}

fn utf16_len(text: &str) -> u64 {
    text.encode_utf16().count() as u64
}

/// Where `text` is cut to keep at most its newest `MAX_SNAPSHOT_TEXT_BYTES`:
/// on the first character boundary at or after the excess.
fn tail_cut(text: &str) -> usize {
    let Some(mut cut) = text.len().checked_sub(MAX_SNAPSHOT_TEXT_BYTES) else {
        return 0;
    };
    while !text.is_char_boundary(cut) {
        cut += 1;
    }
    cut
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

    /// What a snapshot shows of the run: at most the newest
    /// `MAX_SNAPSHOT_TEXT_BYTES` of its step's text.
    pub(crate) fn view(&self, run_id: &str) -> Option<LiveRunView> {
        self.lock().get(run_id).map(|run| {
            let text = &run.view.text;
            let cut = tail_cut(text);
            LiveRunView {
                step_id: run.view.step_id.clone(),
                text: text[cut..].to_string(),
                text_offset: run.view.text_offset + utf16_len(&text[..cut]),
                tools: run.view.tools.clone(),
            }
        })
    }

    /// Bytes of the current step's text the registry keeps.
    #[cfg(test)]
    pub(crate) fn kept_text_bytes(&self, run_id: &str) -> usize {
        self.lock().get(run_id).map_or(0, |run| run.view.text.len())
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
        // Trimmed back to the cap only once past twice it (final fix wave
        // S2-J): a trim moves every byte it keeps, so trimming on each delta
        // past the cap cost a whole cap per delta.
        if run.view.text.len() > 2 * MAX_SNAPSHOT_TEXT_BYTES {
            let cut = tail_cut(&run.view.text);
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
            // Keyed by step and call (final fix wave S2-K): a provider may
            // reuse a call id in a later step.
            let tools = &mut run.view.tools;
            tools.retain(|known| {
                known.step_id != tool.step_id || known.tool_call_id != tool.tool_call_id
            });
            tools.push(tool);
            if tools.len() > MAX_LIVE_TOOL_CARDS {
                tools.drain(..tools.len() - MAX_LIVE_TOOL_CARDS);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn tool_finished(
        &self,
        run_id: &str,
        step_id: &str,
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
                .find(|tool| tool.step_id == step_id && tool.tool_call_id == tool_call_id)
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
}
