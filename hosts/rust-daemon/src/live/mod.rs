//! Live runs (spec §6): per-agent event fanouts, the registry of runs in
//! flight with their streamed text and tool cards, and (Task 6) the
//! observer that turns runtime frames into events.

pub(crate) mod events;
pub(crate) mod fanout;
pub(crate) mod registry;

#[allow(unused_imports)] // Tasks 6–9 consume the remaining names.
pub(crate) use events::{
    preview, resync_json, run_status_event, snapshot_json, LiveEvent, LiveEventBody, SnapshotRun,
};
pub(crate) use fanout::{LiveDelivery, LiveHub, LiveSubscription};
#[allow(unused_imports)] // Tasks 6–9 consume the remaining names.
pub(crate) use registry::{LiveRunView, LiveRuns, LiveToolView};

/// Events buffered per agent before a slow stream lags (spec §6, §16);
/// `ANIMAOS_RS_SESSION_EVENT_BUFFER` overrides it (read in `main.rs`).
pub(crate) const DEFAULT_SESSION_EVENT_BUFFER: usize = 1_024;
/// Streams one agent may have open at once; the next is refused (spec §6).
pub(crate) const MAX_EVENT_SUBSCRIBERS_PER_AGENT: usize = 16;
/// Tool argument and result previews are cut to this many bytes (spec §6).
pub(crate) const MAX_PREVIEW_BYTES: usize = 2 * 1024;
/// A snapshot carries at most this much of a step's text, the newest part.
pub(crate) const MAX_SNAPSHOT_TEXT_BYTES: usize = 64 * 1024;
/// Keep-alive comments go out this often (spec §6).
pub(crate) const EVENT_KEEP_ALIVE_SECS: u64 = 15;

#[cfg(test)]
mod tests;
