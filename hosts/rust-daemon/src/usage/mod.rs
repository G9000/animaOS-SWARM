//! Usage records (spec §11): one row per model call, priced when written.
//! Run-step rows are derived from the terminal run record; secondary calls
//! (titles, compaction, profile, agency) are recorded by their callers.

pub(crate) mod metered;
pub(crate) mod pricing;
pub(crate) mod summary;

use anima_core::TokenUsage;
use serde::{Deserialize, Serialize};

use crate::runs::{RunRecord, RunSource};
pub(crate) use pricing::{price_call, PricingOverride};

pub(crate) const DEFAULT_USAGE_RANGE_DAYS: u64 = 30;
pub(crate) const MAX_USAGE_RANGE_DAYS: u64 = 366;
pub(crate) const DEFAULT_RECORDS_LIMIT: usize = 50;
pub(crate) const MAX_RECORDS_LIMIT: usize = 200;
pub(crate) const USAGE_SCAN_PAGE: usize = 2_000;
pub(crate) const MAX_SUMMARY_ROWS: usize = 200_000;
pub(crate) const MAX_SESSION_GROUPS: usize = 20;
pub(crate) const MAX_CSV_ROWS: usize = 100_000;
pub(crate) const USAGE_QUEUE_MAX: usize = 10_000;
pub(crate) const HISTORY_USAGE_BATCH: usize = 500;

/// What made a model call.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum UsageSource {
    Chat,
    Telegram,
    Automation,
    Job,
    Helper,
    Api,
    Title,
    Compaction,
    Profile,
    Agency,
}

impl UsageSource {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Telegram => "telegram",
            Self::Automation => "automation",
            Self::Job => "job",
            Self::Helper => "helper",
            Self::Api => "api",
            Self::Title => "title",
            Self::Compaction => "compaction",
            Self::Profile => "profile",
            Self::Agency => "agency",
        }
    }
}

impl From<RunSource> for UsageSource {
    fn from(source: RunSource) -> Self {
        match source {
            RunSource::Web => Self::Chat,
            RunSource::Api => Self::Api,
            RunSource::Telegram => Self::Telegram,
            RunSource::Schedule => Self::Automation,
            RunSource::Job => Self::Job,
            RunSource::Delegation | RunSource::Peer => Self::Helper,
        }
    }
}

/// How a record's cost was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PricingSource {
    Table,
    Override,
    Free,
    Subscription,
    Unknown,
}

impl PricingSource {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Table => "table",
            Self::Override => "override",
            Self::Free => "free",
            Self::Subscription => "subscription",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageRecord {
    pub(crate) id: String,
    pub(crate) agent_id: String,
    pub(crate) session_id: Option<String>,
    pub(crate) run_id: Option<String>,
    pub(crate) source: UsageSource,
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) prompt_tokens: u64,
    pub(crate) completion_tokens: u64,
    pub(crate) cached_prompt_tokens: u64,
    pub(crate) reasoning_tokens: u64,
    pub(crate) total_tokens: u64,
    /// Micro-USD; `None` when the call could not be priced.
    pub(crate) cost_micros: Option<u64>,
    pub(crate) pricing_source: PricingSource,
    pub(crate) duration_ms: u64,
    pub(crate) created_at_ms: u64,
}

/// Where and when one model call happened.
#[derive(Clone, Debug)]
pub(crate) struct UsageCall {
    pub(crate) id: String,
    pub(crate) agent_id: String,
    pub(crate) session_id: Option<String>,
    pub(crate) run_id: Option<String>,
    pub(crate) source: UsageSource,
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) duration_ms: u64,
    pub(crate) created_at_ms: u64,
}

/// Builds a record from one call's `TokenUsage`, priced with [`price_call`].
pub(crate) fn usage_record(
    call: &UsageCall,
    usage: &TokenUsage,
    overrides: &[PricingOverride],
) -> UsageRecord {
    let (cost_micros, pricing_source) = price_call(&call.provider, &call.model, usage, overrides);
    UsageRecord {
        id: call.id.clone(),
        agent_id: call.agent_id.clone(),
        session_id: call.session_id.clone(),
        run_id: call.run_id.clone(),
        source: call.source,
        provider: call.provider.clone(),
        model: call.model.clone(),
        prompt_tokens: usage.prompt_tokens,
        completion_tokens: usage.completion_tokens,
        cached_prompt_tokens: usage.cached_prompt_tokens,
        reasoning_tokens: usage.reasoning_tokens,
        total_tokens: usage.total_tokens,
        cost_micros,
        pricing_source,
        duration_ms: call.duration_ms,
        created_at_ms: call.created_at_ms,
    }
}

fn is_zero(usage: &TokenUsage) -> bool {
    usage.prompt_tokens == 0
        && usage.completion_tokens == 0
        && usage.total_tokens == 0
        && usage.cached_prompt_tokens == 0
        && usage.reasoning_tokens == 0
}

fn saturating_sub(total: &TokenUsage, used: &TokenUsage) -> TokenUsage {
    TokenUsage {
        prompt_tokens: total.prompt_tokens.saturating_sub(used.prompt_tokens),
        completion_tokens: total
            .completion_tokens
            .saturating_sub(used.completion_tokens),
        total_tokens: total.total_tokens.saturating_sub(used.total_tokens),
        cached_prompt_tokens: total
            .cached_prompt_tokens
            .saturating_sub(used.cached_prompt_tokens),
        reasoning_tokens: total.reasoning_tokens.saturating_sub(used.reasoning_tokens),
    }
}

/// One record per `RunStepUsage` (id = the step id), plus one `<runId>:rest`
/// record for any usage the run total holds beyond the steps (a run past the
/// step cap). Steps whose usage is all zero make no record.
pub(crate) fn usage_records_for_run(
    run: &RunRecord,
    overrides: &[PricingOverride],
) -> Vec<UsageRecord> {
    let provider = run.provider.clone().unwrap_or_else(|| "unknown".into());
    let source = UsageSource::from(run.source);
    let fallback_ms = run.started_at_ms.unwrap_or(run.created_at_ms);
    let call = |id: String, duration_ms: u64, created_at_ms: u64| UsageCall {
        id,
        agent_id: run.agent_id.clone(),
        session_id: Some(run.session_id.clone()),
        run_id: Some(run.id.clone()),
        source,
        provider: provider.clone(),
        model: run.model.clone(),
        duration_ms,
        created_at_ms,
    };

    let mut records = Vec::new();
    let mut accounted = TokenUsage::default();
    for step in &run.steps {
        accounted.saturating_add(&step.usage);
        if is_zero(&step.usage) {
            continue;
        }
        let at_ms = if step.at_ms == 0 {
            fallback_ms
        } else {
            step.at_ms
        };
        records.push(usage_record(
            &call(step.step_id.clone(), step.duration_ms, at_ms),
            &step.usage,
            overrides,
        ));
    }
    let rest = saturating_sub(&run.usage, &accounted);
    if rest.total_tokens > 0 {
        let at_ms = run.finished_at_ms.unwrap_or(fallback_ms);
        records.push(usage_record(
            &call(format!("{}:rest", run.id), 0, at_ms),
            &rest,
            overrides,
        ));
    }
    records
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::{RunStart, RunStepUsage};

    fn tokens(prompt: u64, completion: u64) -> TokenUsage {
        TokenUsage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: prompt + completion,
            ..TokenUsage::default()
        }
    }

    fn step(id: &str, usage: TokenUsage, at_ms: u64, duration_ms: u64) -> RunStepUsage {
        RunStepUsage {
            step_id: id.into(),
            usage,
            at_ms,
            duration_ms,
        }
    }

    fn run(source: RunSource) -> RunRecord {
        let mut run = RunRecord::running(
            RunStart {
                agent_id: "agent-a".into(),
                session_id: "chat:a".into(),
                source,
                source_ref: None,
                idempotency_key: None,
                text: "hi".into(),
                model: "claude-fable-5-1".into(),
                provider: Some("anthropic".into()),
                parent_run_id: None,
            },
            1_000,
        );
        run.id = "run_1".into();
        run.finished_at_ms = Some(9_000);
        run
    }

    #[test]
    fn run_steps_become_one_record_each_with_the_runs_context() {
        let mut run = run(RunSource::Schedule);
        run.steps = vec![
            step("run_1:1", tokens(10, 5), 2_000, 300),
            step("run_1:2", tokens(20, 6), 0, 0),
        ];
        run.usage = tokens(30, 11);
        let records = usage_records_for_run(&run, &[]);
        assert_eq!(records.len(), 2);
        let first = &records[0];
        assert_eq!(first.id, "run_1:1");
        assert_eq!(first.agent_id, "agent-a");
        assert_eq!(first.session_id.as_deref(), Some("chat:a"));
        assert_eq!(first.run_id.as_deref(), Some("run_1"));
        assert_eq!(first.source, UsageSource::Automation);
        assert_eq!(
            (first.provider.as_str(), first.model.as_str()),
            ("anthropic", "claude-fable-5-1")
        );
        assert_eq!(
            (
                first.prompt_tokens,
                first.completion_tokens,
                first.total_tokens
            ),
            (10, 5, 15)
        );
        assert_eq!((first.created_at_ms, first.duration_ms), (2_000, 300));
        assert_eq!(first.pricing_source, PricingSource::Table);
        assert!(first.cost_micros.is_some());
        assert_eq!(
            records[1].created_at_ms, 1_000,
            "a step without timing falls back to the run's start"
        );

        run.provider = None;
        let records = usage_records_for_run(&run, &[]);
        assert_eq!(records[0].provider, "unknown");
        assert_eq!(records[0].pricing_source, PricingSource::Unknown);
        assert_eq!(records[0].cost_micros, None);
    }

    #[test]
    fn a_zero_usage_step_makes_no_record() {
        let mut run = run(RunSource::Web);
        run.steps = vec![
            step("run_1:1", TokenUsage::default(), 2_000, 10),
            step("run_1:2", tokens(4, 1), 3_000, 10),
        ];
        run.usage = tokens(4, 1);
        let records = usage_records_for_run(&run, &[]);
        assert_eq!(
            records.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["run_1:2"]
        );
    }

    #[test]
    fn steps_past_the_cap_add_one_remainder_row() {
        let mut run = run(RunSource::Web);
        run.steps = vec![
            step("run_1:1", tokens(10, 10), 2_000, 1),
            step("run_1:2", tokens(10, 10), 3_000, 1),
            step("run_1:3", tokens(10, 10), 4_000, 1),
        ];
        run.usage = tokens(70, 30);
        let records = usage_records_for_run(&run, &[]);
        assert_eq!(records.len(), 4);
        let rest = records.last().unwrap();
        assert_eq!(rest.id, "run_1:rest");
        assert_eq!(rest.total_tokens, 40);
        assert_eq!((rest.prompt_tokens, rest.completion_tokens), (40, 0));
        assert_eq!((rest.duration_ms, rest.created_at_ms), (0, 9_000));
        assert_eq!(rest.source, UsageSource::Chat);
        assert_eq!(
            records.iter().map(|r| r.total_tokens).sum::<u64>(),
            run.usage.total_tokens,
            "totals match the run"
        );
    }

    #[test]
    fn no_remainder_when_the_steps_account_for_the_total() {
        let mut run = run(RunSource::Web);
        run.steps = vec![step("run_1:1", tokens(10, 10), 2_000, 1)];
        run.usage = tokens(10, 10);
        assert_eq!(usage_records_for_run(&run, &[]).len(), 1);
        run.usage = tokens(5, 5);
        assert_eq!(
            usage_records_for_run(&run, &[]).len(),
            1,
            "a run total below its steps saturates to no remainder"
        );
    }

    #[test]
    fn run_source_maps_to_usage_source() {
        for (source, expected) in [
            (RunSource::Web, UsageSource::Chat),
            (RunSource::Api, UsageSource::Api),
            (RunSource::Telegram, UsageSource::Telegram),
            (RunSource::Schedule, UsageSource::Automation),
            (RunSource::Job, UsageSource::Job),
            (RunSource::Delegation, UsageSource::Helper),
            (RunSource::Peer, UsageSource::Helper),
        ] {
            assert_eq!(UsageSource::from(source), expected);
        }
        assert_eq!(UsageSource::Compaction.as_str(), "compaction");
        assert_eq!(
            serde_json::to_value(UsageSource::Automation).unwrap(),
            "automation"
        );
    }

    #[test]
    fn constants() {
        assert_eq!(DEFAULT_USAGE_RANGE_DAYS, 30);
        assert_eq!(MAX_USAGE_RANGE_DAYS, 366);
        assert_eq!(DEFAULT_RECORDS_LIMIT, 50);
        assert_eq!(MAX_RECORDS_LIMIT, 200);
        assert_eq!(USAGE_SCAN_PAGE, 2_000);
        assert_eq!(MAX_SUMMARY_ROWS, 200_000);
        assert_eq!(MAX_SESSION_GROUPS, 20);
        assert_eq!(MAX_CSV_ROWS, 100_000);
        assert_eq!(USAGE_QUEUE_MAX, 10_000);
        assert_eq!(HISTORY_USAGE_BATCH, 500);
    }

    #[test]
    fn a_record_serializes_camel_case_with_null_for_absent_values() {
        let record = usage_record(
            &UsageCall {
                id: "u1".into(),
                agent_id: "a".into(),
                session_id: None,
                run_id: None,
                source: UsageSource::Title,
                provider: "chatgpt".into(),
                model: "gpt-5".into(),
                duration_ms: 5,
                created_at_ms: 7,
            },
            &tokens(1, 1),
            &[],
        );
        let json = serde_json::to_value(&record).unwrap();
        assert_eq!(json["sessionId"], serde_json::Value::Null);
        assert_eq!(json["costMicros"], serde_json::Value::Null);
        assert_eq!(json["pricingSource"], "subscription");
        assert_eq!(json["source"], "title");
        assert_eq!(serde_json::from_value::<UsageRecord>(json).unwrap(), record);
    }
}
