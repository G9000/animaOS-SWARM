//! Usage (spec §11): the summary, the records, the CSV export, and the
//! owner's price overrides. Every route requires the local owner and answers
//! `Cache-Control: no-store`, errors included.

use std::collections::HashMap;
use std::sync::Arc;

use anima_core::primitives::now_millis;
use anima_model_adapters::PRICING_TABLE_DATE;
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, SecondsFormat, Utc};

use super::contracts::{
    ErrorBody, PricingEnvelope, PricingOverrideBody, PricingPutRequest, UsageRecordResponse,
    UsageRecordsEnvelope, UsageSummaryResponse, UsageTotalsResponse,
};
use super::http::{json_response, request_query};
use super::jobs::{authorize, body, no_store};
use super::sessions::rejected;
use super::{ApiError, AppState};
use crate::history::{HistoryError, HistoryStore, UsagePageQuery};
use crate::usage::pricing::validate_overrides;
use crate::usage::summary::{day_key, summarize, GroupBy, SummaryQuery};
use crate::usage::{
    PricingOverride, UsageRecord, DEFAULT_RECORDS_LIMIT, DEFAULT_USAGE_RANGE_DAYS, MAX_CSV_ROWS,
    MAX_RECORDS_LIMIT, MAX_USAGE_RANGE_DAYS, USAGE_SCAN_PAGE,
};

pub(super) const USAGE_RANGE_INVALID: &str =
    "from and to must be epoch milliseconds, from before to, at most 366 days apart";
pub(super) const USAGE_GROUP_INVALID: &str = "groupBy must be one of day, model, source, session";
pub(super) const USAGE_LIMIT_INVALID: &str = "limit must be from 1 to 200";
pub(super) const USAGE_CURSOR_INVALID: &str = "cursor is not valid";
pub(super) const USAGE_TZ_INVALID: &str = "tzOffsetMinutes must be from -840 to 840";
pub(super) const USAGE_EXPORT_TOO_LARGE: &str =
    "That range has too many calls to export; choose a shorter range";
pub(super) const USAGE_TASK_FAILED: &str =
    "The usage change did not finish; check Usage and try again";

pub(super) const CSV_HEADER: &str = "id,createdAt,agentId,sessionId,runId,source,provider,model,promptTokens,completionTokens,cachedPromptTokens,reasoningTokens,totalTokens,costUsd,pricingSource,durationMs";

const DAY_MS: u64 = 86_400_000;
const MAX_TZ_OFFSET_MINUTES: i32 = 840;

type Params = HashMap<String, String>;

fn params(request: &Request) -> Result<Params, ApiError> {
    request_query(request.uri()).map_err(|()| ApiError::bad_request_static(USAGE_RANGE_INVALID))
}

/// `from`/`to` (epoch ms, `to` exclusive). Absent: `to` is `now_ms + 1` and
/// `from` is 30 days before it. The effective range must be forward and at
/// most 366 days.
fn range(params: &Params, now_ms: u64) -> Result<(u64, u64), ApiError> {
    let invalid = || ApiError::bad_request_static(USAGE_RANGE_INVALID);
    let number = |name: &str| -> Result<Option<u64>, ApiError> {
        params
            .get(name)
            .map(|value| value.parse::<u64>().map_err(|_| invalid()))
            .transpose()
    };
    let to = number("to")?.unwrap_or_else(|| now_ms.saturating_add(1));
    let from =
        number("from")?.unwrap_or_else(|| to.saturating_sub(DEFAULT_USAGE_RANGE_DAYS * DAY_MS));
    if from >= to || to - from > MAX_USAGE_RANGE_DAYS * DAY_MS {
        return Err(invalid());
    }
    Ok((from, to))
}

fn group_by(params: &Params) -> Result<Option<GroupBy>, ApiError> {
    match params.get("groupBy").map(String::as_str) {
        None => Ok(None),
        Some("day") => Ok(Some(GroupBy::Day)),
        Some("model") => Ok(Some(GroupBy::Model)),
        Some("source") => Ok(Some(GroupBy::Source)),
        Some("session") => Ok(Some(GroupBy::Session)),
        Some(_) => Err(ApiError::bad_request_static(USAGE_GROUP_INVALID)),
    }
}

fn tz_offset(params: &Params) -> Result<i32, ApiError> {
    match params.get("tzOffsetMinutes") {
        None => Ok(0),
        Some(value) => value
            .parse::<i32>()
            .ok()
            .filter(|offset| (-MAX_TZ_OFFSET_MINUTES..=MAX_TZ_OFFSET_MINUTES).contains(offset))
            .ok_or_else(|| ApiError::bad_request_static(USAGE_TZ_INVALID)),
    }
}

fn limit(params: &Params) -> Result<usize, ApiError> {
    match params.get("limit") {
        None => Ok(DEFAULT_RECORDS_LIMIT),
        Some(value) => value
            .parse::<usize>()
            .ok()
            .filter(|limit| (1..=MAX_RECORDS_LIMIT).contains(limit))
            .ok_or_else(|| ApiError::bad_request_static(USAGE_LIMIT_INVALID)),
    }
}

/// `<createdAtMs>:<id>`, split at the first `:`.
fn cursor(params: &Params) -> Result<Option<(u64, String)>, ApiError> {
    let Some(value) = params.get("cursor") else {
        return Ok(None);
    };
    value
        .split_once(':')
        .and_then(|(at_ms, id)| Some((at_ms.parse::<u64>().ok()?, id)))
        .filter(|(_, id)| !id.is_empty())
        .map(|(at_ms, id)| Some((at_ms, id.to_string())))
        .ok_or_else(|| ApiError::bad_request_static(USAGE_CURSOR_INVALID))
}

fn non_empty(params: &Params, name: &str) -> Option<String> {
    params.get(name).filter(|value| !value.is_empty()).cloned()
}

async fn store(state: &AppState) -> Arc<dyn HistoryStore> {
    let guard = state.daemon.read().await;
    guard.history.store()
}

fn store_error(error: HistoryError) -> Response {
    rejected(ApiError::service_unavailable(error.to_string()))
}

#[utoipa::path(get, path = "/api/usage/summary", tag = "usage",
    params(
        ("from" = Option<u64>, Query, description = "Epoch ms, inclusive; default 30 days before `to`"),
        ("to" = Option<u64>, Query, description = "Epoch ms, exclusive; default now"),
        ("agentId" = Option<String>, Query),
        ("sessionId" = Option<String>, Query),
        ("groupBy" = Option<String>, Query, description = "day, model, source, or session"),
        ("tzOffsetMinutes" = Option<i32>, Query, description = "-840 to 840, default 0; days are cut at this offset")
    ),
    responses(
        (status = 200, description = "Totals and optional groups for the range", body = UsageSummaryResponse),
        (status = 400, description = "An invalid query", body = ErrorBody),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 503, description = "The usage store cannot be read", body = ErrorBody)
    ))]
pub(super) async fn usage_summary(State(state): State<AppState>, request: Request) -> Response {
    if let Err(response) = authorize(&state, &request, true) {
        return response;
    }
    let params = match params(&request) {
        Ok(params) => params,
        Err(error) => return rejected(error),
    };
    let parsed = range(&params, now_millis()).and_then(|range| {
        let group = group_by(&params)?;
        let tz_offset_minutes = tz_offset(&params)?;
        Ok((range, group, tz_offset_minutes))
    });
    let ((from_ms, to_ms), group, tz_offset_minutes) = match parsed {
        Ok(parsed) => parsed,
        Err(error) => return rejected(error),
    };
    let summary = summarize(
        &*store(&state).await,
        SummaryQuery {
            from_ms,
            to_ms,
            agent_id: non_empty(&params, "agentId"),
            session_id: non_empty(&params, "sessionId"),
            group_by: group,
            tz_offset_minutes,
        },
    )
    .await;
    match summary {
        Ok(summary) => no_store(json_response(
            StatusCode::OK,
            &UsageSummaryResponse::new(
                (from_ms, to_ms),
                params
                    .get("groupBy")
                    .map(String::as_str)
                    .filter(|_| group.is_some()),
                tz_offset_minutes,
                &summary,
            ),
        )),
        Err(error) => store_error(error),
    }
}

#[utoipa::path(get, path = "/api/usage/records", tag = "usage",
    params(
        ("from" = Option<u64>, Query), ("to" = Option<u64>, Query),
        ("agentId" = Option<String>, Query),
        ("cursor" = Option<String>, Query, description = "`nextCursor` of the previous page"),
        ("limit" = Option<usize>, Query, description = "1-200, default 50")
    ),
    responses(
        (status = 200, description = "Calls, newest first", body = UsageRecordsEnvelope),
        (status = 400, description = "An invalid query", body = ErrorBody),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 503, description = "The usage store cannot be read", body = ErrorBody)
    ))]
pub(super) async fn usage_records(State(state): State<AppState>, request: Request) -> Response {
    if let Err(response) = authorize(&state, &request, true) {
        return response;
    }
    let params = match params(&request) {
        Ok(params) => params,
        Err(error) => return rejected(error),
    };
    let parsed = range(&params, now_millis()).and_then(|range| {
        let limit = limit(&params)?;
        let before = cursor(&params)?;
        Ok((range, limit, before))
    });
    let ((from_ms, to_ms), limit, before) = match parsed {
        Ok(parsed) => parsed,
        Err(error) => return rejected(error),
    };
    // One row past the page proves there is another page.
    let page = store(&state)
        .await
        .page_usage(&UsagePageQuery {
            from_ms,
            to_ms,
            agent_id: non_empty(&params, "agentId"),
            session_id: None,
            before,
            limit: limit + 1,
        })
        .await;
    let mut records = match page {
        Ok(records) => records,
        Err(error) => return store_error(error),
    };
    let more = records.len() > limit;
    records.truncate(limit);
    let next_cursor = more
        .then(|| records.last())
        .flatten()
        .map(|last| format!("{}:{}", last.created_at_ms, last.id));
    no_store(json_response(
        StatusCode::OK,
        &UsageRecordsEnvelope {
            records: records.iter().map(UsageRecordResponse::from).collect(),
            next_cursor,
        },
    ))
}

/// One CSV cell: text that starts with a formula character (`=`, `+`, `-`,
/// `@`, tab, or CR) gets a leading `'` so a spreadsheet reads it as text, and
/// text with a comma, quote, CR, or LF is quoted with its quotes doubled.
pub(super) fn csv_cell(value: &str) -> String {
    let guarded = value.starts_with(['=', '+', '-', '@', '\t', '\r']);
    let mut text = String::with_capacity(value.len() + 1);
    if guarded {
        text.push('\'');
    }
    text.push_str(value);
    if text.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text
    }
}

fn iso(at_ms: u64) -> String {
    i64::try_from(at_ms)
        .ok()
        .and_then(DateTime::<Utc>::from_timestamp_millis)
        .map(|at| at.to_rfc3339_opts(SecondsFormat::Millis, true))
        .unwrap_or_default()
}

fn usd(micros: u64) -> String {
    format!("{}.{:06}", micros / 1_000_000, micros % 1_000_000)
}

fn csv_row(record: &UsageRecord) -> String {
    let optional = |value: &Option<String>| csv_cell(value.as_deref().unwrap_or_default());
    [
        csv_cell(&record.id),
        iso(record.created_at_ms),
        csv_cell(&record.agent_id),
        optional(&record.session_id),
        optional(&record.run_id),
        csv_cell(record.source.as_str()),
        csv_cell(&record.provider),
        csv_cell(&record.model),
        record.prompt_tokens.to_string(),
        record.completion_tokens.to_string(),
        record.cached_prompt_tokens.to_string(),
        record.reasoning_tokens.to_string(),
        record.total_tokens.to_string(),
        record.cost_micros.map(usd).unwrap_or_default(),
        csv_cell(record.pricing_source.as_str()),
        record.duration_ms.to_string(),
    ]
    .join(",")
}

/// The range as CSV, oldest first; `Ok(None)` when it holds more than
/// `max_rows` calls.
pub(super) async fn export_csv(
    store: &dyn HistoryStore,
    from_ms: u64,
    to_ms: u64,
    agent_id: Option<String>,
    max_rows: usize,
) -> Result<Option<String>, HistoryError> {
    let mut records: Vec<UsageRecord> = Vec::new();
    let mut before: Option<(u64, String)> = None;
    loop {
        let page = store
            .page_usage(&UsagePageQuery {
                from_ms,
                to_ms,
                agent_id: agent_id.clone(),
                session_id: None,
                before: before.clone(),
                limit: USAGE_SCAN_PAGE,
            })
            .await?;
        let full = page.len() >= USAGE_SCAN_PAGE;
        if let Some(last) = page.last() {
            before = Some((last.created_at_ms, last.id.clone()));
        }
        records.extend(page);
        if records.len() > max_rows {
            return Ok(None);
        }
        if !full {
            break;
        }
    }
    records.reverse();
    let mut csv = String::from(CSV_HEADER);
    csv.push('\n');
    for record in &records {
        csv.push_str(&csv_row(record));
        csv.push('\n');
    }
    Ok(Some(csv))
}

#[utoipa::path(get, path = "/api/usage/export.csv", tag = "usage",
    params(("from" = Option<u64>, Query), ("to" = Option<u64>, Query), ("agentId" = Option<String>, Query)),
    responses(
        (status = 200, description = "The calls of the range as CSV, oldest first", content_type = "text/csv", body = String),
        (status = 400, description = "An invalid query, or too many calls to export", body = ErrorBody),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 503, description = "The usage store cannot be read", body = ErrorBody)
    ))]
pub(super) async fn usage_export(State(state): State<AppState>, request: Request) -> Response {
    if let Err(response) = authorize(&state, &request, true) {
        return response;
    }
    let params = match params(&request) {
        Ok(params) => params,
        Err(error) => return rejected(error),
    };
    let (from_ms, to_ms) = match range(&params, now_millis()) {
        Ok(range) => range,
        Err(error) => return rejected(error),
    };
    let csv = export_csv(
        &*store(&state).await,
        from_ms,
        to_ms,
        non_empty(&params, "agentId"),
        MAX_CSV_ROWS,
    )
    .await;
    let csv = match csv {
        Ok(Some(csv)) => csv,
        Ok(None) => return rejected(ApiError::bad_request_static(USAGE_EXPORT_TOO_LARGE)),
        Err(error) => return store_error(error),
    };
    let filename = format!(
        "anima-usage-{}-{}.csv",
        day_key(from_ms, 0),
        day_key(to_ms - 1, 0)
    );
    let mut response = (StatusCode::OK, csv).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/csv; charset=utf-8"),
    );
    if let Ok(disposition) = HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
    {
        headers.insert(header::CONTENT_DISPOSITION, disposition);
    }
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    no_store(response)
}

fn pricing_envelope(overrides: &[PricingOverride]) -> PricingEnvelope {
    PricingEnvelope {
        overrides: overrides.iter().map(PricingOverrideBody::from).collect(),
        table_date: PRICING_TABLE_DATE.to_string(),
    }
}

#[utoipa::path(get, path = "/api/usage/pricing", tag = "usage",
    responses(
        (status = 200, description = "The owner's price overrides and the built-in table's date", body = PricingEnvelope),
        (status = 403, description = "Local owner required", body = ErrorBody)
    ))]
pub(super) async fn get_pricing(State(state): State<AppState>, request: Request) -> Response {
    if let Err(response) = authorize(&state, &request, true) {
        return response;
    }
    let envelope = pricing_envelope(&state.daemon.read().await.pricing_overrides);
    no_store(json_response(StatusCode::OK, &envelope))
}

#[utoipa::path(put, path = "/api/usage/pricing", tag = "usage",
    request_body = PricingPutRequest,
    responses(
        (status = 200, description = "The overrides replaced; calls recorded afterwards use them", body = PricingEnvelope),
        (status = 400, description = "An invalid list", body = ErrorBody),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 503, description = "The list could not be saved; the old one stays", body = ErrorBody)
    ))]
pub(super) async fn put_pricing(State(state): State<AppState>, request: Request) -> Response {
    if let Err(response) = authorize(&state, &request, false) {
        return response;
    }
    let input: PricingPutRequest = match body(&state, request).await {
        Ok(input) => input,
        Err(response) => return response,
    };
    let list = match validate_overrides(input.overrides.into_iter().map(Into::into).collect()) {
        Ok(list) => list,
        Err(message) => return rejected(ApiError::bad_request_static(message)),
    };
    // Keep the transaction through the save and any revert even if the HTTP
    // caller disconnects.
    let operation = tokio::spawn(async move {
        let transaction = state.agent_runs.control_plane_transaction().await;
        let (previous, persist) = {
            let mut guard = state.daemon.write().await;
            let previous = guard.set_pricing_overrides(list.clone());
            (previous, guard.control_plane_persist_request())
        };
        if let Err(error) = persist.save().await {
            state.daemon.write().await.set_pricing_overrides(previous);
            return rejected(ApiError::service_unavailable(error.to_string()));
        }
        drop(transaction);
        no_store(json_response(StatusCode::OK, &pricing_envelope(&list)))
    });
    match operation.await {
        Ok(response) => response,
        Err(_) => rejected(ApiError::service_unavailable(USAGE_TASK_FAILED)),
    }
}

/// The totals of one session for `GET session`; `None` when the store cannot
/// be read or the scan hit its cap (the figures would be partial).
pub(super) async fn session_totals(
    state: &AppState,
    agent_id: &str,
    session_id: &str,
) -> Option<UsageTotalsResponse> {
    let summary = summarize(
        &*store(state).await,
        SummaryQuery {
            from_ms: 0,
            to_ms: now_millis().saturating_add(1),
            agent_id: Some(agent_id.to_string()),
            session_id: Some(session_id.to_string()),
            group_by: None,
            tz_offset_minutes: 0,
        },
    )
    .await
    .ok()?;
    (!summary.truncated).then(|| UsageTotalsResponse::from(&summary.totals))
}
