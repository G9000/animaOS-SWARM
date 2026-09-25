//! Live runs (spec §6): per-agent event fanouts, the registry of runs in
//! flight with their streamed text and tool cards, and the observer that
//! turns runtime frames into events.

pub(crate) mod events;
pub(crate) mod fanout;
pub(crate) mod observer;
pub(crate) mod registry;

#[allow(unused_imports)] // M3 Tasks 7 and 8 publish run status events directly.
pub(crate) use events::run_status_event;
pub(crate) use events::{
    committed_message_events, resync_json, snapshot_json, LiveEvent, LiveEventBody, SnapshotRun,
};
pub(crate) use fanout::{LiveDelivery, LiveHub, LiveSubscription};
pub(crate) use observer::{LiveRun, LiveRunEnd};
#[cfg(test)]
pub(crate) use registry::LiveToolView;

/// Events buffered per agent before a slow stream lags (spec §6, §16);
/// `ANIMAOS_RS_SESSION_EVENT_BUFFER` overrides it (read in `main.rs`).
pub(crate) const DEFAULT_SESSION_EVENT_BUFFER: usize = 1_024;
/// Streams one agent may have open at once; the next is refused (spec §6).
pub(crate) const MAX_EVENT_SUBSCRIBERS_PER_AGENT: usize = 16;
/// Streamed text is published at least this often (spec §4.5, §16)...
pub(crate) const DELTA_FLUSH_MS: u64 = 50;
/// ...or as soon as this many bytes are buffered, whichever comes first.
pub(crate) const DELTA_FLUSH_BYTES: usize = 512;
/// Tool argument and result previews are cut to this many bytes (spec §6).
pub(crate) const MAX_PREVIEW_BYTES: usize = 2 * 1024;
/// A snapshot carries at most this much of a step's text, the newest part.
pub(crate) const MAX_SNAPSHOT_TEXT_BYTES: usize = 64 * 1024;
/// Tool cards a run keeps for new streams' snapshots, the newest ones; a
/// run's tool rounds (`max_tool_iterations`) have no upper bound.
pub(crate) const MAX_LIVE_TOOL_CARDS: usize = 50;
/// Keep-alive comments go out this often (spec §6).
pub(crate) const EVENT_KEEP_ALIVE_SECS: u64 = 15;

#[cfg(test)]
mod tests;
