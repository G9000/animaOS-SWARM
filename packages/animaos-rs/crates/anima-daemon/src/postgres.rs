use anima_core::persistence::{
    DatabaseAdapter, PersistenceError, PersistenceResult, Step, StepStatus,
};
use async_trait::async_trait;
use sqlx::PgPool;

pub struct SqlxPostgresAdapter {
    pool: PgPool,
}

impl SqlxPostgresAdapter {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl DatabaseAdapter for SqlxPostgresAdapter {
    async fn write_step(&self, step: &Step) -> PersistenceResult<()> {
        let status = step.status.as_str();
        sqlx::query(
            r#"
            INSERT INTO step_log (id, agent_id, step_index, idempotency_key, type, status, input, output)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (agent_id, step_index)
            DO UPDATE SET
                status = CASE
                    WHEN step_log.status IN ('done', 'failed') THEN step_log.status
                    ELSE EXCLUDED.status
                END,
                output = COALESCE(EXCLUDED.output, step_log.output)
            "#,
        )
        .bind(&step.id)
        .bind(&step.agent_id)
        .bind(step.step_index)
        .bind(&step.idempotency_key)
        .bind(&step.step_type)
        .bind(status)
        .bind(&step.input)
        .bind(&step.output)
        .execute(&self.pool)
        .await
        .map_err(|e| PersistenceError::Write(e.to_string()))?;
        Ok(())
    }

    async fn get_step_by_idempotency_key(
        &self,
        agent_id: &str,
        key: &str,
    ) -> PersistenceResult<Option<Step>> {
        let row = sqlx::query(
            "SELECT id, agent_id, step_index, idempotency_key, type, status, input, output \
             FROM step_log WHERE agent_id = $1 AND idempotency_key = $2",
        )
        .bind(agent_id)
        .bind(key)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| PersistenceError::Query(e.to_string()))?;
        Ok(row.map(|r| row_to_step(&r)))
    }

    async fn list_agent_steps(&self, agent_id: &str) -> PersistenceResult<Vec<Step>> {
        let rows = sqlx::query(
            "SELECT id, agent_id, step_index, idempotency_key, type, status, input, output \
             FROM step_log WHERE agent_id = $1 ORDER BY step_index ASC",
        )
        .bind(agent_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| PersistenceError::Query(e.to_string()))?;
        Ok(rows.iter().map(row_to_step).collect())
    }
}

fn row_to_step(row: &sqlx::postgres::PgRow) -> Step {
    use sqlx::Row;
    let status_str: &str = row.get("status");
    let status = match status_str {
        "done" => StepStatus::Done,
        "failed" => StepStatus::Failed,
        _ => StepStatus::Pending,
    };
    Step {
        id: row.get("id"),
        agent_id: row.get("agent_id"),
        step_index: row.get("step_index"),
        idempotency_key: row.get("idempotency_key"),
        step_type: row.get("type"),
        status,
        input: row.get("input"),
        output: row.get("output"),
    }
}
