//! Log tail bodies (spec §11).

use serde::Serialize;
use utoipa::ToSchema;

use crate::logs::LogLine;

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LogLineResponse {
    /// Increases by one per captured line; the cursor for `after`.
    pub(crate) seq: u64,
    /// Epoch milliseconds.
    pub(crate) at: u64,
    /// `error`, `warn`, `info`, `debug`, or `trace`.
    pub(crate) level: String,
    pub(crate) target: String,
    /// Redacted, single-line, and bounded at capture.
    pub(crate) message: String,
}

impl From<&LogLine> for LogLineResponse {
    fn from(line: &LogLine) -> Self {
        Self {
            seq: line.seq,
            at: line.at,
            level: line.level.as_str().into(),
            target: line.target.clone(),
            message: line.message.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LogsEnvelope {
    /// Oldest first.
    pub(crate) lines: Vec<LogLineResponse>,
    /// The newest seq the buffer has assigned; 0 before the first line.
    pub(crate) newest_seq: u64,
}
