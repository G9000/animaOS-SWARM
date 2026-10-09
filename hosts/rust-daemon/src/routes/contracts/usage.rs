//! Usage and pricing bodies (spec §11).

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::usage::summary::{Summary, UsageTotals};
use crate::usage::{PricingOverride, UsageRecord};

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageRecordResponse {
    pub(crate) id: String,
    pub(crate) agent_id: String,
    pub(crate) session_id: Option<String>,
    pub(crate) run_id: Option<String>,
    /// `chat`, `telegram`, `automation`, `job`, `helper`, `api`, `title`,
    /// `compaction`, `profile`, or `agency`.
    pub(crate) source: String,
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) prompt_tokens: u64,
    pub(crate) completion_tokens: u64,
    pub(crate) cached_prompt_tokens: u64,
    pub(crate) reasoning_tokens: u64,
    pub(crate) total_tokens: u64,
    /// Micro-USD; `null` when the call could not be priced.
    pub(crate) cost_micros: Option<u64>,
    /// `table`, `override`, `free`, `subscription`, or `unknown`.
    pub(crate) pricing_source: String,
    pub(crate) duration_ms: u64,
    pub(crate) created_at_ms: u64,
}

impl From<&UsageRecord> for UsageRecordResponse {
    fn from(record: &UsageRecord) -> Self {
        Self {
            id: record.id.clone(),
            agent_id: record.agent_id.clone(),
            session_id: record.session_id.clone(),
            run_id: record.run_id.clone(),
            source: record.source.as_str().into(),
            provider: record.provider.clone(),
            model: record.model.clone(),
            prompt_tokens: record.prompt_tokens,
            completion_tokens: record.completion_tokens,
            cached_prompt_tokens: record.cached_prompt_tokens,
            reasoning_tokens: record.reasoning_tokens,
            total_tokens: record.total_tokens,
            cost_micros: record.cost_micros,
            pricing_source: record.pricing_source.as_str().into(),
            duration_ms: record.duration_ms,
            created_at_ms: record.created_at_ms,
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageTotalsResponse {
    pub(crate) calls: u64,
    pub(crate) prompt_tokens: u64,
    pub(crate) completion_tokens: u64,
    pub(crate) cached_prompt_tokens: u64,
    pub(crate) reasoning_tokens: u64,
    pub(crate) total_tokens: u64,
    /// The sum over the priced calls, in micro-USD.
    pub(crate) cost_micros: u64,
    /// Calls with no cost that are not subscription calls.
    pub(crate) unpriced_calls: u64,
    pub(crate) subscription_calls: u64,
}

impl From<&UsageTotals> for UsageTotalsResponse {
    fn from(totals: &UsageTotals) -> Self {
        Self {
            calls: totals.calls,
            prompt_tokens: totals.prompt_tokens,
            completion_tokens: totals.completion_tokens,
            cached_prompt_tokens: totals.cached_prompt_tokens,
            reasoning_tokens: totals.reasoning_tokens,
            total_tokens: totals.total_tokens,
            cost_micros: totals.cost_micros,
            unpriced_calls: totals.unpriced_calls,
            subscription_calls: totals.subscription_calls,
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageGroupResponse {
    /// `YYYY-MM-DD`, `<provider>/<model>`, a source name, or a session id.
    pub(crate) key: String,
    pub(crate) totals: UsageTotalsResponse,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageSummaryResponse {
    pub(crate) from: u64,
    pub(crate) to: u64,
    pub(crate) group_by: Option<String>,
    pub(crate) tz_offset_minutes: i32,
    pub(crate) totals: UsageTotalsResponse,
    pub(crate) groups: Vec<UsageGroupResponse>,
    /// The scan stopped at its row cap, so the figures are partial.
    pub(crate) truncated: bool,
}

impl UsageSummaryResponse {
    pub(crate) fn new(
        range: (u64, u64),
        group_by: Option<&str>,
        tz_offset_minutes: i32,
        summary: &Summary,
    ) -> Self {
        Self {
            from: range.0,
            to: range.1,
            group_by: group_by.map(str::to_string),
            tz_offset_minutes,
            totals: UsageTotalsResponse::from(&summary.totals),
            groups: summary
                .groups
                .iter()
                .map(|(key, totals)| UsageGroupResponse {
                    key: key.clone(),
                    totals: UsageTotalsResponse::from(totals),
                })
                .collect(),
            truncated: summary.truncated,
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageRecordsEnvelope {
    pub(crate) records: Vec<UsageRecordResponse>,
    /// `<createdAtMs>:<id>`; `null` on the last page.
    pub(crate) next_cursor: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PricingOverrideBody {
    pub(crate) provider: String,
    /// A lowercase model prefix; the longest match wins.
    pub(crate) model: String,
    pub(crate) input_micros_per_mtok: u64,
    pub(crate) output_micros_per_mtok: u64,
    #[serde(default)]
    pub(crate) cached_input_micros_per_mtok: Option<u64>,
}

impl From<&PricingOverride> for PricingOverrideBody {
    fn from(entry: &PricingOverride) -> Self {
        Self {
            provider: entry.provider.clone(),
            model: entry.model.clone(),
            input_micros_per_mtok: entry.input_micros_per_mtok,
            output_micros_per_mtok: entry.output_micros_per_mtok,
            cached_input_micros_per_mtok: entry.cached_input_micros_per_mtok,
        }
    }
}

impl From<PricingOverrideBody> for PricingOverride {
    fn from(body: PricingOverrideBody) -> Self {
        Self {
            provider: body.provider,
            model: body.model,
            input_micros_per_mtok: body.input_micros_per_mtok,
            output_micros_per_mtok: body.output_micros_per_mtok,
            cached_input_micros_per_mtok: body.cached_input_micros_per_mtok,
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PricingEnvelope {
    pub(crate) overrides: Vec<PricingOverrideBody>,
    /// The date of the built-in price table.
    pub(crate) table_date: String,
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PricingPutRequest {
    pub(crate) overrides: Vec<PricingOverrideBody>,
}
