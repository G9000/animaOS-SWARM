//! Coordinator runs: the durable run ledger (spec §4.1, §4.8).

mod ledger;

#[allow(unused_imports)] // Later M1 tasks consume the remaining names.
pub(crate) use ledger::{
    RunError, RunInput, RunLedger, RunRecord, RunSource, RunStart, RunStatus, RunStepUsage,
    RunStopRequest, AGENT_DELETED, COMMIT_FAILED, COMMIT_REJECTED, MAX_RUN_INPUT_TEXT_BYTES,
    MAX_RUN_TOOLS_STARTED, MAX_TERMINAL_RUNS_PER_AGENT, RESTART_BEFORE_START, RESTART_DURING_RUN,
    RUN_ABORTED, RUN_FAILED, TERMINAL_RUN_RETENTION_MS,
};
