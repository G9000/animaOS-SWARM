use async_trait::async_trait;
use std::fmt;

// ---------------------------------------------------------------------------
// StepStatus
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum StepStatus {
    Pending,
    Done,
    Failed,
}

impl StepStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            StepStatus::Pending => "pending",
            StepStatus::Done => "done",
            StepStatus::Failed => "failed",
        }
    }
}

// ---------------------------------------------------------------------------
// Step
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Step {
    pub id: String,
    pub agent_id: String,
    pub step_index: i32,
    pub idempotency_key: String,
    pub step_type: String,
    pub status: StepStatus,
    pub input: Option<serde_json::Value>,
    pub output: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// PersistenceError
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum PersistenceError {
    Connection(String),
    Write(String),
    Query(String),
}

impl fmt::Display for PersistenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PersistenceError::Connection(msg) => write!(f, "Connection error: {}", msg),
            PersistenceError::Write(msg) => write!(f, "Write error: {}", msg),
            PersistenceError::Query(msg) => write!(f, "Query error: {}", msg),
        }
    }
}

pub type PersistenceResult<T> = Result<T, PersistenceError>;

// ---------------------------------------------------------------------------
// DatabaseAdapter trait
// ---------------------------------------------------------------------------

#[async_trait]
pub trait DatabaseAdapter: Send + Sync {
    async fn write_step(&self, step: &Step) -> PersistenceResult<()>;

    async fn get_step_by_idempotency_key(
        &self,
        agent_id: &str,
        key: &str,
    ) -> PersistenceResult<Option<Step>>;

    async fn list_agent_steps(&self, agent_id: &str) -> PersistenceResult<Vec<Step>>;
}

// ---------------------------------------------------------------------------
// InMemoryAdapter (test-only)
// ---------------------------------------------------------------------------

#[cfg(test)]
pub mod in_memory {
    use super::*;
    use std::sync::{Arc, Mutex};

    pub struct InMemoryAdapter {
        steps: Arc<Mutex<Vec<Step>>>,
    }

    impl InMemoryAdapter {
        pub fn new() -> Self {
            Self {
                steps: Arc::new(Mutex::new(Vec::new())),
            }
        }

        pub fn recorded_steps(&self) -> Vec<Step> {
            self.steps.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl DatabaseAdapter for InMemoryAdapter {
        async fn write_step(&self, step: &Step) -> PersistenceResult<()> {
            let mut steps = self
                .steps
                .lock()
                .map_err(|e| PersistenceError::Write(format!("Mutex poisoned: {}", e)))?;

            // Upsert by logical idempotency key first, then step index as a fallback.
            // preserve input, freeze terminal status+output
            if let Some(existing) = steps.iter_mut().find(|s| {
                s.agent_id == step.agent_id
                    && (s.idempotency_key == step.idempotency_key
                        || s.step_index == step.step_index)
            }) {
                if !matches!(existing.status, StepStatus::Done | StepStatus::Failed) {
                    existing.status = step.status.clone();
                    if step.input.is_some() {
                        existing.input = step.input.clone();
                    }
                    if step.output.is_some() {
                        existing.output = step.output.clone();
                    }
                }
            } else {
                steps.push(step.clone());
            }

            Ok(())
        }

        async fn get_step_by_idempotency_key(
            &self,
            agent_id: &str,
            key: &str,
        ) -> PersistenceResult<Option<Step>> {
            let steps = self
                .steps
                .lock()
                .map_err(|e| PersistenceError::Query(format!("Mutex poisoned: {}", e)))?;

            let found = steps
                .iter()
                .find(|s| s.agent_id == agent_id && s.idempotency_key == key)
                .cloned();

            Ok(found)
        }

        async fn list_agent_steps(&self, agent_id: &str) -> PersistenceResult<Vec<Step>> {
            let steps = self
                .steps
                .lock()
                .map_err(|e| PersistenceError::Query(format!("Mutex poisoned: {}", e)))?;

            let mut result: Vec<Step> = steps
                .iter()
                .filter(|s| s.agent_id == agent_id)
                .cloned()
                .collect();

            result.sort_by_key(|s| s.step_index);
            Ok(result)
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::in_memory::InMemoryAdapter;
    use super::*;

    fn make_step(
        agent_id: &str,
        step_index: i32,
        idempotency_key: &str,
        status: StepStatus,
    ) -> Step {
        Step {
            id: uuid::Uuid::new_v4().to_string(),
            agent_id: agent_id.to_string(),
            step_index,
            idempotency_key: idempotency_key.to_string(),
            step_type: "tool".to_string(),
            status,
            input: Some(serde_json::json!({ "query": "hello" })),
            output: None,
        }
    }

    #[tokio::test]
    async fn write_and_retrieve_step() {
        let adapter = InMemoryAdapter::new();
        let step = make_step("agent-1", 0, "key-abc", StepStatus::Pending);

        adapter.write_step(&step).await.expect("write failed");

        let retrieved = adapter
            .get_step_by_idempotency_key("agent-1", "key-abc")
            .await
            .expect("query failed");

        assert!(retrieved.is_some(), "step should be found");
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.agent_id, "agent-1");
        assert_eq!(retrieved.idempotency_key, "key-abc");
        assert_eq!(retrieved.status, StepStatus::Pending);
    }

    #[tokio::test]
    async fn write_step_upserts_status() {
        let adapter = InMemoryAdapter::new();

        // Write initial step as Pending
        let step_pending = make_step("agent-2", 0, "key-xyz", StepStatus::Pending);
        adapter
            .write_step(&step_pending)
            .await
            .expect("initial write failed");

        // Upsert with Done status — same (agent_id, step_index)
        let step_done = Step {
            id: step_pending.id.clone(),
            agent_id: "agent-2".to_string(),
            step_index: 0,
            idempotency_key: "key-xyz".to_string(),
            step_type: "tool".to_string(),
            status: StepStatus::Done,
            input: step_pending.input.clone(),
            output: Some(serde_json::json!({ "result": "ok" })),
        };
        adapter
            .write_step(&step_done)
            .await
            .expect("upsert write failed");

        // List steps and confirm only one entry exists, with Done status
        let steps = adapter
            .list_agent_steps("agent-2")
            .await
            .expect("list failed");

        assert_eq!(steps.len(), 1, "upsert should keep exactly one entry");
        assert_eq!(steps[0].status, StepStatus::Done);
        assert!(steps[0].output.is_some());
    }

    #[tokio::test]
    async fn write_step_upserts_by_idempotency_key_across_retry_indices() {
        let adapter = InMemoryAdapter::new();

        let pending = make_step("agent-3", 0, "key-retry", StepStatus::Pending);
        adapter
            .write_step(&pending)
            .await
            .expect("initial write failed");

        let retried_done = Step {
            id: uuid::Uuid::new_v4().to_string(),
            agent_id: "agent-3".to_string(),
            step_index: 1,
            idempotency_key: "key-retry".to_string(),
            step_type: "tool".to_string(),
            status: StepStatus::Done,
            input: pending.input.clone(),
            output: Some(serde_json::json!({ "result": "cached" })),
        };
        adapter
            .write_step(&retried_done)
            .await
            .expect("retry write failed");

        let steps = adapter
            .list_agent_steps("agent-3")
            .await
            .expect("list failed");

        assert_eq!(steps.len(), 1, "retry should reuse the logical step row");
        assert_eq!(
            steps[0].step_index, 0,
            "original ordering should be preserved"
        );
        assert_eq!(steps[0].status, StepStatus::Done);
    }
}
