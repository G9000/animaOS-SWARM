//! Usage summaries, computed over paged `page_usage` reads so each history
//! store needs only `upsert_usage` and `page_usage` (spec §11.2).

use std::collections::BTreeMap;

use serde::Serialize;

use super::{PricingSource, UsageRecord, MAX_SESSION_GROUPS, MAX_SUMMARY_ROWS, USAGE_SCAN_PAGE};
use crate::history::{HistoryError, HistoryStore, UsagePageQuery};

/// Sums over a set of calls.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageTotals {
    pub(crate) calls: u64,
    pub(crate) prompt_tokens: u64,
    pub(crate) completion_tokens: u64,
    pub(crate) cached_prompt_tokens: u64,
    pub(crate) reasoning_tokens: u64,
    pub(crate) total_tokens: u64,
    /// The sum over the priced calls only.
    pub(crate) cost_micros: u64,
    /// Calls with no cost that are not subscription calls.
    pub(crate) unpriced_calls: u64,
    pub(crate) subscription_calls: u64,
}

impl UsageTotals {
    pub(crate) fn add(&mut self, record: &UsageRecord) {
        self.calls = self.calls.saturating_add(1);
        self.prompt_tokens = self.prompt_tokens.saturating_add(record.prompt_tokens);
        self.completion_tokens = self
            .completion_tokens
            .saturating_add(record.completion_tokens);
        self.cached_prompt_tokens = self
            .cached_prompt_tokens
            .saturating_add(record.cached_prompt_tokens);
        self.reasoning_tokens = self
            .reasoning_tokens
            .saturating_add(record.reasoning_tokens);
        self.total_tokens = self.total_tokens.saturating_add(record.total_tokens);
        match record.cost_micros {
            Some(cost) => self.cost_micros = self.cost_micros.saturating_add(cost),
            None if record.pricing_source == PricingSource::Subscription => {
                self.subscription_calls = self.subscription_calls.saturating_add(1);
            }
            None => self.unpriced_calls = self.unpriced_calls.saturating_add(1),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GroupBy {
    Day,
    Model,
    Source,
    Session,
}

#[derive(Clone, Debug)]
pub(crate) struct SummaryQuery {
    pub(crate) from_ms: u64,
    /// Exclusive.
    pub(crate) to_ms: u64,
    pub(crate) agent_id: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) group_by: Option<GroupBy>,
    pub(crate) tz_offset_minutes: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Summary {
    pub(crate) totals: UsageTotals,
    pub(crate) groups: Vec<(String, UsageTotals)>,
    /// The scan stopped at the row cap, so the figures are partial.
    pub(crate) truncated: bool,
}

/// Pages `page_usage` (`USAGE_SCAN_PAGE` rows at a time) until the range is
/// exhausted or `MAX_SUMMARY_ROWS` rows were read (then `truncated`).
pub(crate) async fn summarize(
    store: &dyn HistoryStore,
    query: SummaryQuery,
) -> Result<Summary, HistoryError> {
    summarize_with_cap(store, query, MAX_SUMMARY_ROWS).await
}

/// [`summarize`] with the row cap passed in, so tests need not write 200,000
/// rows.
pub(crate) async fn summarize_with_cap(
    store: &dyn HistoryStore,
    query: SummaryQuery,
    max_rows: usize,
) -> Result<Summary, HistoryError> {
    let mut totals = UsageTotals::default();
    let mut groups: BTreeMap<String, UsageTotals> = BTreeMap::new();
    let mut read = 0usize;
    let mut truncated = false;
    let mut before: Option<(u64, String)> = None;
    'scan: loop {
        // One row past the cap, so a row beyond it proves there is more.
        let limit = USAGE_SCAN_PAGE.min(max_rows.saturating_sub(read).saturating_add(1));
        let page = store
            .page_usage(&UsagePageQuery {
                from_ms: query.from_ms,
                to_ms: query.to_ms,
                agent_id: query.agent_id.clone(),
                session_id: query.session_id.clone(),
                before: before.clone(),
                limit,
            })
            .await?;
        let page_len = page.len();
        for record in &page {
            if read >= max_rows {
                truncated = true;
                break 'scan;
            }
            read += 1;
            totals.add(record);
            if let Some(group_by) = query.group_by {
                groups
                    .entry(group_key(record, group_by, query.tz_offset_minutes))
                    .or_default()
                    .add(record);
            }
        }
        match page.last() {
            Some(last) if page_len >= limit => {
                before = Some((last.created_at_ms, last.id.clone()));
            }
            _ => break,
        }
    }

    let mut groups = groups.into_iter().collect::<Vec<_>>();
    if !matches!(query.group_by, Some(GroupBy::Day)) {
        groups.sort_by(|left, right| {
            right
                .1
                .total_tokens
                .cmp(&left.1.total_tokens)
                .then_with(|| left.0.cmp(&right.0))
        });
    }
    if matches!(query.group_by, Some(GroupBy::Session)) {
        groups.truncate(MAX_SESSION_GROUPS);
    }
    Ok(Summary {
        totals,
        groups,
        truncated,
    })
}

fn group_key(record: &UsageRecord, group_by: GroupBy, tz_offset_minutes: i32) -> String {
    match group_by {
        GroupBy::Day => day_key(record.created_at_ms, tz_offset_minutes),
        GroupBy::Model => format!("{}/{}", record.provider, record.model),
        GroupBy::Source => record.source.as_str().to_string(),
        GroupBy::Session => record.session_id.clone().unwrap_or_default(),
    }
}

/// `YYYY-MM-DD` of the instant shifted by the caller's offset (UTC
/// arithmetic on `ms + offset`).
pub(crate) fn day_key(created_at_ms: u64, tz_offset_minutes: i32) -> String {
    let shifted = i128::from(created_at_ms) + i128::from(tz_offset_minutes) * 60_000;
    let days = shifted.div_euclid(86_400_000) as i64;
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::MemoryHistoryStore;
    use crate::usage::UsageSource;

    const HOUR: u64 = 3_600_000;
    const DAY: u64 = 86_400_000;

    fn record(id: &str, at_ms: u64) -> UsageRecord {
        UsageRecord {
            id: id.into(),
            agent_id: "agent-a".into(),
            session_id: Some("chat:a".into()),
            run_id: None,
            source: UsageSource::Chat,
            provider: "anthropic".into(),
            model: "claude-fable-5-1".into(),
            prompt_tokens: 10,
            completion_tokens: 5,
            cached_prompt_tokens: 2,
            reasoning_tokens: 1,
            total_tokens: 15,
            cost_micros: Some(100),
            pricing_source: PricingSource::Table,
            duration_ms: 1,
            created_at_ms: at_ms,
        }
    }

    fn query(group_by: Option<GroupBy>) -> SummaryQuery {
        SummaryQuery {
            from_ms: 0,
            to_ms: u64::MAX / 2,
            agent_id: None,
            session_id: None,
            group_by,
            tz_offset_minutes: 0,
        }
    }

    async fn store_with(records: &[UsageRecord]) -> MemoryHistoryStore {
        let store = MemoryHistoryStore::new();
        store.upsert_usage(records).await.unwrap();
        store
    }

    #[tokio::test]
    async fn totals_sum_every_field() {
        let store = store_with(&[record("a", 1), record("b", 2), record("c", 3)]).await;
        let summary = summarize(&store, query(None)).await.unwrap();
        assert_eq!(
            summary.totals,
            UsageTotals {
                calls: 3,
                prompt_tokens: 30,
                completion_tokens: 15,
                cached_prompt_tokens: 6,
                reasoning_tokens: 3,
                total_tokens: 45,
                cost_micros: 300,
                unpriced_calls: 0,
                subscription_calls: 0,
            }
        );
        assert!(summary.groups.is_empty());
        assert!(!summary.truncated);
        let json = serde_json::to_value(&summary.totals).unwrap();
        assert_eq!(json["costMicros"], 300);
        assert_eq!(json["unpricedCalls"], 0);
    }

    #[tokio::test]
    async fn unpriced_and_subscription_calls_are_counted_apart() {
        let mut unknown = record("u", 2);
        unknown.cost_micros = None;
        unknown.pricing_source = PricingSource::Unknown;
        let mut subscription = record("s", 3);
        subscription.cost_micros = None;
        subscription.pricing_source = PricingSource::Subscription;
        let mut free = record("f", 4);
        free.cost_micros = Some(0);
        free.pricing_source = PricingSource::Free;
        let store = store_with(&[record("a", 1), unknown, subscription, free]).await;
        let totals = summarize(&store, query(None)).await.unwrap().totals;
        assert_eq!(totals.calls, 4);
        assert_eq!(totals.unpriced_calls, 1);
        assert_eq!(totals.subscription_calls, 1);
        assert_eq!(totals.cost_micros, 100, "only priced rows add to the cost");
    }

    #[tokio::test]
    async fn groups_by_day_in_the_callers_offset() {
        // 23:30 UTC on 1970-01-02 and 00:10 UTC on 1970-01-03.
        let late = DAY + 23 * HOUR + HOUR / 2;
        let early = 2 * DAY + HOUR / 6;
        let store = store_with(&[
            record("a", late),
            record("b", early),
            record("c", early + 1),
        ])
        .await;

        let utc = summarize(&store, query(Some(GroupBy::Day))).await.unwrap();
        let keys = utc
            .groups
            .iter()
            .map(|(key, _)| key.as_str())
            .collect::<Vec<_>>();
        assert_eq!(keys, ["1970-01-02", "1970-01-03"], "days ascend");
        assert_eq!(utc.groups[1].1.calls, 2);

        let mut plus_one = query(Some(GroupBy::Day));
        plus_one.tz_offset_minutes = 60;
        let shifted = summarize(&store, plus_one).await.unwrap();
        assert_eq!(
            shifted.groups.len(),
            1,
            "23:30 UTC is already the next day at +60"
        );
        assert_eq!(shifted.groups[0].0, "1970-01-03");
        assert_eq!(shifted.groups[0].1.calls, 3);
    }

    #[tokio::test]
    async fn groups_by_model_source_and_session_sorted_by_tokens() {
        let mut big = record("big", 1);
        big.model = "gpt-5".into();
        big.provider = "openai".into();
        big.total_tokens = 500;
        big.source = UsageSource::Telegram;
        big.session_id = None;
        let mut tie_b = record("tie_b", 2);
        tie_b.model = "b-model".into();
        tie_b.total_tokens = 15;
        let mut tie_a = record("tie_a", 3);
        tie_a.model = "a-model".into();
        tie_a.total_tokens = 15;
        let store = store_with(&[big, tie_b, tie_a]).await;

        let by_model = summarize(&store, query(Some(GroupBy::Model)))
            .await
            .unwrap();
        let keys = by_model
            .groups
            .iter()
            .map(|(key, _)| key.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            keys,
            ["openai/gpt-5", "anthropic/a-model", "anthropic/b-model"],
            "tokens descending, then key"
        );

        let by_source = summarize(&store, query(Some(GroupBy::Source)))
            .await
            .unwrap();
        assert_eq!(by_source.groups[0].0, "telegram");
        assert_eq!(by_source.groups[1].0, "chat");
        assert_eq!(by_source.groups[1].1.calls, 2);

        let by_session = summarize(&store, query(Some(GroupBy::Session)))
            .await
            .unwrap();
        assert_eq!(
            by_session.groups[0].0, "",
            "a row with no session groups under an empty key"
        );
        assert_eq!(by_session.groups[1].0, "chat:a");
    }

    #[tokio::test]
    async fn session_groups_keep_the_top_twenty() {
        let mut records = Vec::new();
        for n in 0..25u64 {
            let mut row = record(&format!("r{n:02}"), n + 1);
            row.session_id = Some(format!("chat:{n:02}"));
            row.total_tokens = 100 + n;
            records.push(row);
        }
        let store = store_with(&records).await;
        let summary = summarize(&store, query(Some(GroupBy::Session)))
            .await
            .unwrap();
        assert_eq!(summary.groups.len(), MAX_SESSION_GROUPS);
        assert_eq!(summary.groups[0].0, "chat:24");
        assert_eq!(summary.groups[19].0, "chat:05");
        assert_eq!(summary.totals.calls, 25, "totals still cover every session");
    }

    #[tokio::test]
    async fn a_scan_past_the_row_cap_is_truncated() {
        let records = (0..7u64)
            .map(|n| record(&format!("r{n}"), n + 1))
            .collect::<Vec<_>>();
        let store = store_with(&records).await;
        let capped = summarize_with_cap(&store, query(None), 5).await.unwrap();
        assert!(capped.truncated);
        assert_eq!(capped.totals.calls, 5);

        let exact = summarize_with_cap(&store, query(None), 7).await.unwrap();
        assert!(!exact.truncated, "a range of exactly the cap is complete");
        assert_eq!(exact.totals.calls, 7);

        let roomy = summarize_with_cap(&store, query(None), 100).await.unwrap();
        assert!(!roomy.truncated);
    }

    #[test]
    fn day_key_handles_negative_offsets_and_year_ends() {
        assert_eq!(day_key(0, 0), "1970-01-01");
        assert_eq!(
            day_key(0, -1),
            "1969-12-31",
            "a negative offset reaches the day before the epoch"
        );
        assert_eq!(day_key(0, 840), "1970-01-01");
        assert_eq!(day_key(0, -840), "1969-12-31");
        // 2024-12-31 23:59:59.999 UTC, then one millisecond later.
        let new_year = 1_735_689_600_000u64;
        assert_eq!(day_key(new_year - 1, 0), "2024-12-31");
        assert_eq!(day_key(new_year, 0), "2025-01-01");
        assert_eq!(day_key(new_year - 1, 1), "2025-01-01");
        // A leap day.
        assert_eq!(day_key(1_709_164_800_000, 0), "2024-02-29");
        assert_eq!(day_key(1_709_164_800_000 - 1, 0), "2024-02-28");
    }
}
