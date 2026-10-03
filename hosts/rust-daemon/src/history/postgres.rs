//! Postgres history store (spec §13.1) over the tables of migration
//! `20260923000000_history_store.sql`.

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use sqlx::{PgPool, Row};

use super::{
    message_ordinal, role_name, search_tokens, searchable_text, to_i64, ApprovalPageQuery,
    HistoryError, HistoryMessage, HistoryStore, MessagePageQuery,
};
use crate::approvals::ApprovalRequest;
use crate::runs::RunRecord;
use crate::schedules::ScheduleFireRecord;

const UPSERT_MESSAGE: &str = "
INSERT INTO history_messages (id, agent_id, session_id, role, text, hidden, created_at_ms, ordinal, record)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
ON CONFLICT (id) DO UPDATE SET
    agent_id = EXCLUDED.agent_id,
    session_id = EXCLUDED.session_id,
    role = EXCLUDED.role,
    text = EXCLUDED.text,
    hidden = EXCLUDED.hidden,
    created_at_ms = EXCLUDED.created_at_ms,
    ordinal = EXCLUDED.ordinal,
    record = EXCLUDED.record";

const UPSERT_RUN: &str = "
INSERT INTO history_runs (id, agent_id, session_id, status, created_at_ms, finished_at_ms, record)
VALUES ($1, $2, $3, $4, $5, $6, $7)
ON CONFLICT (id) DO UPDATE SET
    agent_id = EXCLUDED.agent_id,
    session_id = EXCLUDED.session_id,
    status = EXCLUDED.status,
    created_at_ms = EXCLUDED.created_at_ms,
    finished_at_ms = EXCLUDED.finished_at_ms,
    record = EXCLUDED.record";

const UPSERT_APPROVAL: &str = "
INSERT INTO history_approvals (id, agent_id, session_id, created_at_ms, record)
VALUES ($1, $2, $3, $4, $5)
ON CONFLICT (id) DO UPDATE SET
    agent_id = EXCLUDED.agent_id,
    session_id = EXCLUDED.session_id,
    created_at_ms = EXCLUDED.created_at_ms,
    record = EXCLUDED.record";

const PAGE_APPROVALS: &str = "
SELECT record FROM history_approvals
WHERE ($1::text IS NULL OR agent_id = $1) AND created_at_ms >= $2
  AND ($3::bigint IS NULL OR created_at_ms < $3 OR (created_at_ms = $3 AND id COLLATE \"C\" < $4))
ORDER BY created_at_ms DESC, id COLLATE \"C\" DESC
LIMIT $5";

const UPSERT_SCHEDULE_RUN: &str = "
INSERT INTO history_schedule_runs (id, schedule_id, agent_id, fired_at_ms, record)
VALUES ($1, $2, $3, $4, $5)
ON CONFLICT (id) DO UPDATE SET
    schedule_id = EXCLUDED.schedule_id,
    agent_id = EXCLUDED.agent_id,
    fired_at_ms = EXCLUDED.fired_at_ms,
    record = EXCLUDED.record";

const PAGE_SCHEDULE_RUNS: &str = "
SELECT record FROM history_schedule_runs
WHERE agent_id = $1 AND schedule_id = $2
ORDER BY fired_at_ms DESC, id COLLATE \"C\" DESC
LIMIT $3";

impl From<sqlx::Error> for HistoryError {
    fn from(error: sqlx::Error) -> Self {
        Self::new(error)
    }
}

pub(crate) struct PostgresHistoryStore {
    pool: PgPool,
}

impl PostgresHistoryStore {
    pub(crate) fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// A prefix query in which every word must match: `deploy:* & build:*`.
pub(crate) fn prefix_tsquery(tokens: &[String]) -> String {
    tokens
        .iter()
        .map(|token| format!("{token}:*"))
        .collect::<Vec<_>>()
        .join(" & ")
}

fn decode_row(row: &sqlx::postgres::PgRow) -> Result<HistoryMessage, HistoryError> {
    let record: serde_json::Value = row.try_get("record")?;
    Ok(HistoryMessage {
        agent_id: row.try_get("agent_id")?,
        session_id: row.try_get("session_id")?,
        hidden: row.try_get("hidden")?,
        message: serde_json::from_value(record)?,
    })
}

#[async_trait]
impl HistoryStore for PostgresHistoryStore {
    fn label(&self) -> &'static str {
        "postgres"
    }

    async fn upsert_messages(&self, messages: &[HistoryMessage]) -> Result<(), HistoryError> {
        if messages.is_empty() {
            return Ok(());
        }
        let mut transaction = self.pool.begin().await?;
        for row in messages {
            sqlx::query(UPSERT_MESSAGE)
                .bind(&row.message.id)
                .bind(&row.agent_id)
                .bind(&row.session_id)
                .bind(role_name(row.message.role))
                // Indexed (the generated `search` tsvector); a check-in
                // prompt's scheduler suffix is stripped so it can't match
                // every search (review fix, M2 fix round 1), and the text is
                // capped to MAX_INDEXED_TEXT_BYTES so one huge message can't
                // fail tsvector generation, which errors past ~1 MB of
                // distinct words (final fix wave item B). `record` keeps the
                // full message.
                .bind(searchable_text(&row.message))
                .bind(row.hidden)
                .bind(to_i64(row.message.created_at_ms)?)
                .bind(to_i64(message_ordinal(&row.message.id))?)
                .bind(serde_json::to_value(&row.message)?)
                .execute(&mut *transaction)
                .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    async fn upsert_runs(&self, runs: &[RunRecord]) -> Result<(), HistoryError> {
        if runs.is_empty() {
            return Ok(());
        }
        let mut transaction = self.pool.begin().await?;
        for run in runs {
            let status = serde_json::to_value(run.status)?
                .as_str()
                .unwrap_or("unknown")
                .to_string();
            sqlx::query(UPSERT_RUN)
                .bind(&run.id)
                .bind(&run.agent_id)
                .bind(&run.session_id)
                .bind(status)
                .bind(to_i64(run.created_at_ms)?)
                .bind(run.finished_at_ms.map(to_i64).transpose()?)
                .bind(serde_json::to_value(run)?)
                .execute(&mut *transaction)
                .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    async fn upsert_approvals(&self, approvals: &[ApprovalRequest]) -> Result<(), HistoryError> {
        if approvals.is_empty() {
            return Ok(());
        }
        let mut transaction = self.pool.begin().await?;
        for approval in approvals {
            sqlx::query(UPSERT_APPROVAL)
                .bind(&approval.id)
                .bind(&approval.agent_id)
                .bind(&approval.session_id)
                .bind(to_i64(approval.created_at_ms)?)
                .bind(serde_json::to_value(approval)?)
                .execute(&mut *transaction)
                .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    async fn get_approval(
        &self,
        approval_id: &str,
    ) -> Result<Option<ApprovalRequest>, HistoryError> {
        let row = sqlx::query("SELECT record FROM history_approvals WHERE id = $1")
            .bind(approval_id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|row| -> Result<ApprovalRequest, HistoryError> {
            let record: serde_json::Value = row.try_get("record")?;
            Ok(serde_json::from_value(record)?)
        })
        .transpose()
    }

    async fn page_approvals(
        &self,
        query: &ApprovalPageQuery,
    ) -> Result<Vec<ApprovalRequest>, HistoryError> {
        let (before_at, before_id) = match &query.before {
            Some((at_ms, id)) => (Some(to_i64(*at_ms)?), id.clone()),
            None => (None, String::new()),
        };
        let rows = sqlx::query(PAGE_APPROVALS)
            .bind(query.agent_id.as_deref())
            .bind(to_i64(query.since_ms)?)
            .bind(before_at)
            .bind(before_id)
            .bind(i64::try_from(query.limit).unwrap_or(i64::MAX))
            .fetch_all(&self.pool)
            .await?;
        rows.iter()
            .map(|row| -> Result<ApprovalRequest, HistoryError> {
                let record: serde_json::Value = row.try_get("record")?;
                Ok(serde_json::from_value(record)?)
            })
            .collect()
    }

    async fn upsert_schedule_runs(&self, fires: &[ScheduleFireRecord]) -> Result<(), HistoryError> {
        if fires.is_empty() {
            return Ok(());
        }
        let mut transaction = self.pool.begin().await?;
        for fire in fires {
            sqlx::query(UPSERT_SCHEDULE_RUN)
                .bind(&fire.id)
                .bind(&fire.schedule_id)
                .bind(&fire.agent_id)
                .bind(to_i64(fire.fired_at_ms)?)
                .bind(serde_json::to_value(fire)?)
                .execute(&mut *transaction)
                .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    async fn page_schedule_runs(
        &self,
        agent_id: &str,
        schedule_id: &str,
        limit: usize,
    ) -> Result<Vec<ScheduleFireRecord>, HistoryError> {
        let rows = sqlx::query(PAGE_SCHEDULE_RUNS)
            .bind(agent_id)
            .bind(schedule_id)
            .bind(i64::try_from(limit).unwrap_or(i64::MAX))
            .fetch_all(&self.pool)
            .await?;
        rows.iter()
            .map(|row| -> Result<ScheduleFireRecord, HistoryError> {
                let record: serde_json::Value = row.try_get("record")?;
                Ok(serde_json::from_value(record)?)
            })
            .collect()
    }

    async fn existing_message_ids(&self, ids: &[String]) -> Result<HashSet<String>, HistoryError> {
        if ids.is_empty() {
            return Ok(HashSet::new());
        }
        let rows = sqlx::query("SELECT id FROM history_messages WHERE id = ANY($1)")
            .bind(ids.to_vec())
            .fetch_all(&self.pool)
            .await?;
        rows.iter()
            .map(|row| row.try_get::<String, _>("id").map_err(HistoryError::from))
            .collect()
    }

    async fn get_message(
        &self,
        agent_id: &str,
        session_id: &str,
        message_id: &str,
    ) -> Result<Option<HistoryMessage>, HistoryError> {
        let row = sqlx::query(
            "SELECT agent_id, session_id, hidden, record FROM history_messages
             WHERE id = $1 AND agent_id = $2 AND session_id = $3",
        )
        .bind(message_id)
        .bind(agent_id)
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(decode_row).transpose()
    }

    async fn get_run(&self, run_id: &str) -> Result<Option<RunRecord>, HistoryError> {
        let row = sqlx::query("SELECT record FROM history_runs WHERE id = $1")
            .bind(run_id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|row| -> Result<RunRecord, HistoryError> {
            let record: serde_json::Value = row.try_get("record")?;
            Ok(serde_json::from_value(record)?)
        })
        .transpose()
    }

    async fn page_messages(
        &self,
        query: &MessagePageQuery,
    ) -> Result<Vec<HistoryMessage>, HistoryError> {
        let limit = i64::try_from(query.limit).unwrap_or(i64::MAX);
        let rows = match &query.before {
            None => {
                sqlx::query(
                    "SELECT agent_id, session_id, hidden, record FROM history_messages
                     WHERE agent_id = $1 AND session_id = $2 AND ($3 OR NOT hidden)
                     ORDER BY created_at_ms DESC, ordinal DESC, id DESC
                     LIMIT $4",
                )
                .bind(&query.agent_id)
                .bind(&query.session_id)
                .bind(query.include_hidden)
                .bind(limit)
                .fetch_all(&self.pool)
                .await?
            }
            Some(before) => {
                sqlx::query(
                    "SELECT agent_id, session_id, hidden, record FROM history_messages
                     WHERE agent_id = $1 AND session_id = $2 AND ($3 OR NOT hidden)
                       AND (created_at_ms, ordinal, id) < ($5, $6, $7)
                     ORDER BY created_at_ms DESC, ordinal DESC, id DESC
                     LIMIT $4",
                )
                .bind(&query.agent_id)
                .bind(&query.session_id)
                .bind(query.include_hidden)
                .bind(limit)
                .bind(to_i64(before.created_at_ms)?)
                .bind(to_i64(before.ordinal)?)
                .bind(&before.id)
                .fetch_all(&self.pool)
                .await?
            }
        };
        rows.iter().map(decode_row).collect()
    }

    async fn visible_message_counts(
        &self,
        agent_id: &str,
        session_ids: &[String],
    ) -> Result<HashMap<String, usize>, HistoryError> {
        if session_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let rows = sqlx::query(
            "SELECT session_id, COUNT(*) AS total FROM history_messages
             WHERE agent_id = $1 AND NOT hidden AND session_id = ANY($2)
             GROUP BY session_id",
        )
        .bind(agent_id)
        .bind(session_ids.to_vec())
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| -> Result<(String, usize), HistoryError> {
                let total: i64 = row.try_get("total")?;
                Ok((
                    row.try_get::<String, _>("session_id")?,
                    usize::try_from(total).unwrap_or(0),
                ))
            })
            .collect()
    }

    async fn search_messages(
        &self,
        agent_ids: &[String],
        query: &str,
        limit: usize,
    ) -> Result<Vec<HistoryMessage>, HistoryError> {
        let tokens = search_tokens(query);
        if tokens.is_empty() || agent_ids.is_empty() {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(
            "SELECT agent_id, session_id, hidden, record FROM history_messages
             WHERE search @@ to_tsquery('simple', $1) AND NOT hidden AND agent_id = ANY($2)
             ORDER BY created_at_ms DESC, ordinal DESC, id DESC
             LIMIT $3",
        )
        .bind(prefix_tsquery(&tokens))
        .bind(agent_ids.to_vec())
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(decode_row).collect()
    }

    async fn search_sessions(
        &self,
        agent_ids: &[String],
        query: &str,
        limit: usize,
    ) -> Result<Vec<HistoryMessage>, HistoryError> {
        let tokens = search_tokens(query);
        if tokens.is_empty() || agent_ids.is_empty() {
            return Ok(Vec::new());
        }
        // `DISTINCT ON` picks the newest row per (agent, session) before the
        // session limit applies (Controller ruling, M2 pre-flight audit).
        let rows = sqlx::query(
            "SELECT agent_id, session_id, hidden, record FROM (
                 SELECT DISTINCT ON (agent_id, session_id)
                        agent_id, session_id, hidden, record, created_at_ms, ordinal, id
                 FROM history_messages
                 WHERE search @@ to_tsquery('simple', $1) AND NOT hidden AND agent_id = ANY($2)
                 ORDER BY agent_id, session_id, created_at_ms DESC, ordinal DESC, id DESC
             ) ranked
             ORDER BY created_at_ms DESC, ordinal DESC, id DESC
             LIMIT $3",
        )
        .bind(prefix_tsquery(&tokens))
        .bind(agent_ids.to_vec())
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(decode_row).collect()
    }

    async fn delete_session(&self, agent_id: &str, session_id: &str) -> Result<(), HistoryError> {
        let mut transaction = self.pool.begin().await?;
        for table in [
            "history_messages",
            "history_runs",
            "history_attachments",
            "history_approvals",
        ] {
            sqlx::query(&format!(
                "DELETE FROM {table} WHERE agent_id = $1 AND session_id = $2"
            ))
            .bind(agent_id)
            .bind(session_id)
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    async fn delete_agent(&self, agent_id: &str) -> Result<(), HistoryError> {
        let mut transaction = self.pool.begin().await?;
        // Usage rows stay (spec §3.3); approvals and automation fires go with their agent.
        for table in [
            "history_messages",
            "history_runs",
            "history_attachments",
            "history_approvals",
            "history_schedule_runs",
        ] {
            sqlx::query(&format!("DELETE FROM {table} WHERE agent_id = $1"))
                .bind(agent_id)
                .execute(&mut *transaction)
                .await?;
        }
        transaction.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::conformance::{
        assert_history_store_approval_conformance, assert_history_store_checkin_text_conformance,
        assert_history_store_conformance, assert_history_store_diacritics_conformance,
        assert_history_store_indexed_text_cap_conformance,
        assert_history_store_schedule_run_conformance,
        assert_history_store_session_search_conformance,
    };

    #[test]
    fn prefix_queries_join_every_word() {
        assert_eq!(
            prefix_tsquery(&["deploy".to_string(), "build".to_string()]),
            "deploy:* & build:*"
        );
    }

    #[ignore = "requires DATABASE_URL-backed Postgres"]
    #[sqlx::test(migrations = "./migrations")]
    async fn postgres_store_meets_the_conformance_suite(pool: PgPool) {
        let store = PostgresHistoryStore::new(pool);
        assert_history_store_conformance(&store).await;
        assert_history_store_session_search_conformance(&store).await;
        assert_history_store_checkin_text_conformance(&store).await;
        assert_history_store_indexed_text_cap_conformance(&store).await;
        assert_history_store_diacritics_conformance(&store).await;
        assert_history_store_approval_conformance(&store).await;
        assert_history_store_schedule_run_conformance(&store).await;
        assert_eq!(store.label(), "postgres");
    }
}
