# Rust Core Foundation — DatabaseAdapter + Step Log

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `DatabaseAdapter` trait to `anima-core` so any host can inject persistence, then implement `SqlxPostgresAdapter` in `anima-daemon` to checkpoint every tool call to Postgres — establishing "DB = truth" as the foundation of durable execution.

**Architecture:** `anima-core` defines `DatabaseAdapter` as a trait with zero DB dependencies — it's the interface any host implements (Postgres, in-memory, Elixir NIF, JS callback). `AgentRuntime` accepts an `Option<Arc<dyn DatabaseAdapter>>`, writes a `pending` step before each tool call and a `done`/`failed` step after. `anima-daemon` implements `SqlxPostgresAdapter` via sqlx 0.8, runs migrations on startup, and injects the adapter into each runtime. Explicit request retry keys now let retried runs reuse already-completed tool steps; full runtime replay and process-level resume are still follow-up work.

**Tech Stack:** Rust, axum 0.7, sqlx 0.8, PostgreSQL 14+, async-trait, serde_json, uuid 1.x

---

## File Structure

**New files:**
- `packages/animaos-rs/crates/anima-core/src/persistence.rs` — `DatabaseAdapter` trait, `Step`, `StepStatus`, `PersistenceError`
- `packages/animaos-rs/crates/anima-daemon/src/postgres.rs` — `SqlxPostgresAdapter` impl
- `packages/animaos-rs/crates/anima-daemon/migrations/20260412000000_step_log.sql` — schema

**Modified files:**
- `packages/animaos-rs/Cargo.toml` — sqlx workspace dep
- `packages/animaos-rs/crates/anima-core/Cargo.toml` — add serde_json, uuid
- `packages/animaos-rs/crates/anima-core/src/lib.rs` — `pub mod persistence` + re-exports
- `packages/animaos-rs/crates/anima-core/src/runtime.rs` — add `db` + `step_counter` fields; write steps at tool boundaries
- `packages/animaos-rs/crates/anima-daemon/Cargo.toml` — add sqlx with postgres + migrate features
- `packages/animaos-rs/crates/anima-daemon/src/lib.rs` — `pub mod postgres`
- `packages/animaos-rs/crates/anima-daemon/src/state.rs` — expose `set_database()`
- `packages/animaos-rs/crates/anima-daemon/src/app.rs` — connect Postgres on startup, inject adapter

---

## Task 1: DatabaseAdapter trait in anima-core

**Files:**
- Create: `packages/animaos-rs/crates/anima-core/src/persistence.rs`
- Modify: `packages/animaos-rs/crates/anima-core/Cargo.toml`
- Modify: `packages/animaos-rs/crates/anima-core/src/lib.rs`

- [ ] **Step 1: Add serde_json and uuid to anima-core**

In `packages/animaos-rs/crates/anima-core/Cargo.toml`, replace `[dependencies]` with:

```toml
[dependencies]
async-trait = "0.1"
futures = "0.3"
serde_json = "1"
uuid = { version = "1", features = ["v4"] }
```

- [ ] **Step 2: Write failing test for DatabaseAdapter**

Create `packages/animaos-rs/crates/anima-core/src/persistence.rs`:

```rust
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::Value;

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

#[derive(Debug, Clone)]
pub struct Step {
    pub id: String,
    pub agent_id: String,
    pub step_index: i32,
    pub idempotency_key: String,
    pub step_type: String,
    pub status: StepStatus,
    pub input: Option<Value>,
    pub output: Option<Value>,
}

#[derive(Debug)]
pub enum PersistenceError {
    Connection(String),
    Write(String),
    Query(String),
}

impl std::fmt::Display for PersistenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PersistenceError::Connection(m) => write!(f, "connection: {m}"),
            PersistenceError::Write(m) => write!(f, "write: {m}"),
            PersistenceError::Query(m) => write!(f, "query: {m}"),
        }
    }
}

pub type PersistenceResult<T> = Result<T, PersistenceError>;

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

// --- tests ---

#[cfg(test)]
pub struct InMemoryAdapter {
    pub steps: Arc<Mutex<Vec<Step>>>,
}

#[cfg(test)]
impl InMemoryAdapter {
    pub fn new() -> Self {
        Self { steps: Arc::new(Mutex::new(Vec::new())) }
    }
}

#[cfg(test)]
#[async_trait]
impl DatabaseAdapter for InMemoryAdapter {
    async fn write_step(&self, step: &Step) -> PersistenceResult<()> {
        let mut steps = self.steps.lock().unwrap();
        // upsert by (agent_id, step_index)
        if let Some(existing) = steps
            .iter_mut()
            .find(|s| s.agent_id == step.agent_id && s.step_index == step.step_index)
        {
            existing.status = step.status.clone();
            if step.output.is_some() {
                existing.output = step.output.clone();
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
        let steps = self.steps.lock().unwrap();
        Ok(steps
            .iter()
            .find(|s| s.agent_id == agent_id && s.idempotency_key == key)
            .cloned())
    }

    async fn list_agent_steps(&self, agent_id: &str) -> PersistenceResult<Vec<Step>> {
        let steps = self.steps.lock().unwrap();
        Ok(steps.iter().filter(|s| s.agent_id == agent_id).cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn write_and_retrieve_step() {
        let adapter = InMemoryAdapter::new();
        let step = Step {
            id: "test-id".to_string(),
            agent_id: "agent-1".to_string(),
            step_index: 0,
            idempotency_key: "agent-1:tool-call-abc".to_string(),
            step_type: "tool".to_string(),
            status: StepStatus::Pending,
            input: Some(serde_json::json!({ "name": "bash", "args": {} })),
            output: None,
        };

        adapter.write_step(&step).await.unwrap();

        let found = adapter
            .get_step_by_idempotency_key("agent-1", "agent-1:tool-call-abc")
            .await
            .unwrap();

        assert!(found.is_some());
        assert_eq!(found.unwrap().status, StepStatus::Pending);
    }

    #[tokio::test]
    async fn write_step_upserts_status() {
        let adapter = InMemoryAdapter::new();
        let pending = Step {
            id: "id-1".to_string(),
            agent_id: "agent-1".to_string(),
            step_index: 0,
            idempotency_key: "agent-1:tool-call-abc".to_string(),
            step_type: "tool".to_string(),
            status: StepStatus::Pending,
            input: Some(serde_json::json!({})),
            output: None,
        };
        adapter.write_step(&pending).await.unwrap();

        let done = Step {
            id: "id-1".to_string(),
            agent_id: "agent-1".to_string(),
            step_index: 0,
            idempotency_key: "agent-1:tool-call-abc".to_string(),
            step_type: "tool".to_string(),
            status: StepStatus::Done,
            input: None,
            output: Some(serde_json::json!({ "result": "ok" })),
        };
        adapter.write_step(&done).await.unwrap();

        let steps = adapter.list_agent_steps("agent-1").await.unwrap();
        assert_eq!(steps.len(), 1, "upsert must not create duplicate rows");
        assert_eq!(steps[0].status, StepStatus::Done);
        assert!(steps[0].output.is_some());
    }
}
```

- [ ] **Step 3: Run tests to verify they fail (module not exported yet)**

```bash
cd packages/animaos-rs && cargo test -p anima-core persistence 2>&1 | head -20
```

Expected: compile error — `persistence` module not found.

- [ ] **Step 4: Export persistence from lib.rs**

In `packages/animaos-rs/crates/anima-core/src/lib.rs`, add:

```rust
pub mod persistence;

pub use persistence::{DatabaseAdapter, PersistenceError, PersistenceResult, Step, StepStatus};
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
cd packages/animaos-rs && cargo test -p anima-core persistence
```

Expected: `test persistence::tests::write_and_retrieve_step ... ok` and `test persistence::tests::write_step_upserts_status ... ok`.

- [ ] **Step 6: Commit**

```bash
git add packages/animaos-rs/crates/anima-core/
git commit -m "feat(anima-core): add DatabaseAdapter trait and Step types"
```

---

## Task 2: Wire DatabaseAdapter into AgentRuntime

**Files:**
- Modify: `packages/animaos-rs/crates/anima-core/src/runtime.rs`

- [ ] **Step 1: Write failing test for step checkpointing**

In `packages/animaos-rs/crates/anima-core/src/runtime.rs`, inside the existing `#[cfg(test)]` block, add a test that verifies steps are written at tool boundaries:

```rust
#[tokio::test]
async fn agent_writes_pending_and_done_steps_to_db() {
    use crate::persistence::{InMemoryAdapter, StepStatus};
    use std::sync::Arc;

    let adapter = Arc::new(InMemoryAdapter::new());
    let recorded_steps = Arc::clone(&adapter.steps);

    let config = AgentConfig {
        name: "test-agent".to_string(),
        description: "".to_string(),
        system_prompt: "You are a test agent.".to_string(),
        tools: Some(vec!["echo".to_string()]),
        model: None,
        temperature: None,
        max_tokens: None,
        max_iterations: None,
    };

    let model = Arc::new(StubModelAdapter::new(vec![
        // First response: call the echo tool
        ModelGenerateResponse {
            content: Content { text: "".to_string(), attachments: None, metadata: None },
            stop_reason: ModelStopReason::ToolUse,
            tool_calls: Some(vec![ToolCall {
                id: "call-001".to_string(),
                name: "echo".to_string(),
                args: [("text".to_string(), DataValue::String("hello".to_string()))]
                    .into_iter()
                    .collect(),
            }]),
            input_tokens: 10,
            output_tokens: 5,
        },
        // Second response: final answer
        ModelGenerateResponse {
            content: Content {
                text: "Done.".to_string(),
                attachments: None,
                metadata: None,
            },
            stop_reason: ModelStopReason::EndTurn,
            tool_calls: None,
            input_tokens: 15,
            output_tokens: 3,
        },
    ]));

    let mut runtime = AgentRuntime::new(config, model);
    runtime.set_database(Arc::clone(&adapter) as Arc<dyn DatabaseAdapter>);
    runtime.init();

    runtime
        .run_with_tools(
            Content { text: "run the echo tool".to_string(), attachments: None, metadata: None },
            |_, _, tool_call| async move {
                TaskResult::ok(
                    Content {
                        text: format!("echoed: {}", tool_call.name),
                        attachments: None,
                        metadata: None,
                    },
                    0,
                )
            },
        )
        .await;

    let steps = recorded_steps.lock().unwrap();
    assert!(!steps.is_empty(), "at least one step must be written");

    let tool_step = steps.iter().find(|s| s.step_type == "tool").expect("tool step must exist");
    assert_eq!(tool_step.status, StepStatus::Done, "step must be done after successful tool call");
    assert!(tool_step.input.is_some(), "input must be recorded");
    assert!(tool_step.output.is_some(), "output must be recorded");
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cd packages/animaos-rs && cargo test -p anima-core agent_writes_pending_and_done_steps_to_db 2>&1 | head -30
```

Expected: compile error — `set_database` method not found on `AgentRuntime`.

- [ ] **Step 3: Add db and step_counter fields to AgentRuntime**

In `packages/animaos-rs/crates/anima-core/src/runtime.rs`:

Add to imports at top of file:
```rust
use crate::persistence::{DatabaseAdapter, Step, StepStatus};
use uuid::Uuid;
```

Add two fields to `AgentRuntime` struct (after `model_adapter` field):
```rust
pub struct AgentRuntime {
    state: AgentState,
    messages: Vec<Message>,
    last_task: Option<TaskResult<Content>>,
    events: Vec<EngineEvent>,
    event_listener: Option<Arc<dyn Fn(EngineEvent) + Send + Sync>>,
    providers: Vec<Arc<dyn Provider>>,
    evaluators: Vec<Arc<dyn Evaluator>>,
    model_adapter: Arc<dyn ModelAdapter>,
    db: Option<Arc<dyn DatabaseAdapter>>,  // add this
    step_counter: u64,                      // add this
}
```

Initialize them in `AgentRuntime::new()`:
```rust
Self {
    state: AgentState { ... },
    messages: Vec::new(),
    last_task: None,
    events: Vec::new(),
    event_listener: None,
    providers: Vec::new(),
    evaluators: Vec::new(),
    model_adapter,
    db: None,           // add this
    step_counter: 0,    // add this
}
```

Add `set_database` method in the `impl AgentRuntime` block (after `set_event_listener`):
```rust
pub fn set_database(&mut self, db: Arc<dyn DatabaseAdapter>) {
    self.db = Some(db);
}
```

- [ ] **Step 4: Write pending steps before tool execution and done/failed steps after**

In `run_with_tools`, locate the block that processes tool calls (around the `for tool_call in &tool_calls` loop before `join_all`). Replace that section with:

```rust
// Assign a step index to each tool call before parallel execution
let step_map: std::collections::HashMap<String, i32> = tool_calls
    .iter()
    .map(|tc| {
        let idx = self.step_counter as i32;
        self.step_counter += 1;
        (tc.id.clone(), idx)
    })
    .collect();

for tool_call in &tool_calls {
    self.record_event(
        EventType::ToolBefore,
        tool_before_event_data(tool_call),
    );
}

// Write pending steps before execution (one per tool call)
if let Some(db) = self.db.clone() {
    for tool_call in &tool_calls {
        let step_index = *step_map.get(&tool_call.id).expect("assigned above");
        let step = Step {
            id: Uuid::new_v4().to_string(),
            agent_id: self.state.id.clone(),
            step_index,
            idempotency_key: format!("{}:{}", self.state.id, tool_call.id),
            step_type: "tool".to_string(),
            status: StepStatus::Pending,
            input: Some(serde_json::json!({
                "name": tool_call.name,
                "args": data_value_to_json(&DataValue::Object(tool_call.args.clone())),
            })),
            output: None,
        };
        let _ = db.write_step(&step).await;
    }
}

let tool_results =
    join_all(tool_calls.iter().cloned().map(|tool_call| {
        let tool_started = now_millis();
        let state = self.state.clone();
        let user_message = user_message.clone();
        let future =
            execute_tool(state, user_message, tool_call.clone());
        async move {
            let tool_result = future.await;
            let tool_duration =
                now_millis().saturating_sub(tool_started);
            (tool_call, tool_result, tool_duration)
        }
    }))
    .await;

// Write done/failed steps after execution
if let Some(db) = self.db.clone() {
    for (tool_call, tool_result, _) in &tool_results {
        let step_index = *step_map.get(&tool_call.id).expect("assigned above");
        let status = if tool_result.error.is_none() {
            StepStatus::Done
        } else {
            StepStatus::Failed
        };
        let step = Step {
            id: Uuid::new_v4().to_string(),
            agent_id: self.state.id.clone(),
            step_index,
            idempotency_key: format!("{}:{}", self.state.id, tool_call.id),
            step_type: "tool".to_string(),
            status,
            input: None,
            output: Some(serde_json::json!({
                "status": tool_result.status.as_str(),
                "data": tool_result.data.as_ref().map(|c| &c.text),
                "error": tool_result.error,
            })),
        };
        let _ = db.write_step(&step).await;
    }
}

for (tool_call, tool_result, tool_duration) in tool_results {
    self.record_event(
        EventType::ToolAfter,
        tool_after_event_data(
            &tool_call.name,
            tool_result.status.as_str(),
            tool_duration,
            &tool_result,
        ),
    );
    let tool_message = self.record_message_in_room(
        room_id.clone(),
        MessageRole::Tool,
        content_from_tool_result(&tool_call, tool_result),
    );
    conversation.push(tool_message);
}
```

You will also need a `data_value_to_json` helper in runtime.rs. Add this private function at the bottom of the file (before the `#[cfg(test)]` block):

```rust
fn data_value_to_json(value: &DataValue) -> serde_json::Value {
    match value {
        DataValue::Null => serde_json::Value::Null,
        DataValue::Bool(v) => serde_json::json!(v),
        DataValue::Number(v) => serde_json::json!(v),
        DataValue::String(v) => serde_json::json!(v),
        DataValue::Array(vs) => {
            serde_json::Value::Array(vs.iter().map(data_value_to_json).collect())
        }
        DataValue::Object(map) => serde_json::Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), data_value_to_json(v)))
                .collect(),
        ),
    }
}
```

- [ ] **Step 5: Run test to verify it passes**

```bash
cd packages/animaos-rs && cargo test -p anima-core agent_writes_pending_and_done_steps_to_db
```

Expected: `test runtime::tests::agent_writes_pending_and_done_steps_to_db ... ok`.

- [ ] **Step 6: Run all anima-core tests to check for regressions**

```bash
cd packages/animaos-rs && cargo test -p anima-core
```

Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
git add packages/animaos-rs/crates/anima-core/src/runtime.rs
git commit -m "feat(anima-core): wire DatabaseAdapter into AgentRuntime, checkpoint tool calls"
```

---

## Task 3: SQL migration for step_log

**Files:**
- Create: `packages/animaos-rs/crates/anima-daemon/migrations/20260412000000_step_log.sql`

- [ ] **Step 1: Create migrations directory and migration file**

```bash
mkdir -p packages/animaos-rs/crates/anima-daemon/migrations
```

Create `packages/animaos-rs/crates/anima-daemon/migrations/20260412000000_step_log.sql`:

```sql
CREATE TABLE IF NOT EXISTS step_log (
    id              TEXT PRIMARY KEY,
    agent_id        TEXT NOT NULL,
    step_index      INTEGER NOT NULL,
    idempotency_key TEXT NOT NULL,
    type            TEXT NOT NULL,
    status          TEXT NOT NULL CHECK (status IN ('pending', 'done', 'failed')),
    input           JSONB,
    output          JSONB,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (agent_id, step_index)
);

CREATE INDEX IF NOT EXISTS step_log_agent_idx ON step_log (agent_id);
CREATE INDEX IF NOT EXISTS step_log_idem_idx  ON step_log (agent_id, idempotency_key);
```

- [ ] **Step 2: Verify migration file is valid SQL (dry-run)**

```bash
psql $DATABASE_URL -f packages/animaos-rs/crates/anima-daemon/migrations/20260412000000_step_log.sql
```

Expected: `CREATE TABLE`, `CREATE INDEX`, `CREATE INDEX` — no errors.

If `DATABASE_URL` is not set, create a local test database first:
```bash
createdb animaos_dev
export DATABASE_URL=postgres://localhost/animaos_dev
```

- [ ] **Step 3: Commit**

```bash
git add packages/animaos-rs/crates/anima-daemon/migrations/
git commit -m "feat(anima-daemon): add step_log migration"
```

---

## Task 4: SqlxPostgresAdapter in anima-daemon

**Files:**
- Modify: `packages/animaos-rs/Cargo.toml`
- Modify: `packages/animaos-rs/crates/anima-daemon/Cargo.toml`
- Create: `packages/animaos-rs/crates/anima-daemon/src/postgres.rs`
- Modify: `packages/animaos-rs/crates/anima-daemon/src/lib.rs`

- [ ] **Step 1: Add sqlx to workspace Cargo.toml**

In `packages/animaos-rs/Cargo.toml`, add to `[workspace.dependencies]` (create the section if it doesn't exist):

```toml
[workspace.dependencies]
sqlx = { version = "0.8", features = ["postgres", "runtime-tokio", "migrate", "json", "chrono"], default-features = false }
```

- [ ] **Step 2: Add sqlx to anima-daemon**

In `packages/animaos-rs/crates/anima-daemon/Cargo.toml`, add to `[dependencies]`:

```toml
sqlx = { workspace = true }
```

- [ ] **Step 3: Write failing tests for SqlxPostgresAdapter**

Create `packages/animaos-rs/crates/anima-daemon/src/postgres.rs`:

```rust
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
            DO UPDATE SET status = EXCLUDED.status, output = EXCLUDED.output
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

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;

    // sqlx::test spins up a temp database, runs migrations in ./migrations/, and tears down after.
    // Requires DATABASE_URL=postgres://localhost/animaos_test (or similar) in env.

    #[sqlx::test(migrations = "./migrations")]
    async fn write_and_retrieve_step(pool: PgPool) {
        let adapter = SqlxPostgresAdapter::new(pool);

        let step = Step {
            id: "step-001".to_string(),
            agent_id: "agent-1".to_string(),
            step_index: 0,
            idempotency_key: "agent-1:call-001".to_string(),
            step_type: "tool".to_string(),
            status: StepStatus::Pending,
            input: Some(serde_json::json!({ "name": "bash", "args": {} })),
            output: None,
        };

        adapter.write_step(&step).await.unwrap();

        let found = adapter
            .get_step_by_idempotency_key("agent-1", "agent-1:call-001")
            .await
            .unwrap();

        assert!(found.is_some());
        assert_eq!(found.unwrap().status, StepStatus::Pending);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn write_step_transitions_to_done(pool: PgPool) {
        let adapter = SqlxPostgresAdapter::new(pool);

        let pending = Step {
            id: "step-002".to_string(),
            agent_id: "agent-1".to_string(),
            step_index: 0,
            idempotency_key: "agent-1:call-002".to_string(),
            step_type: "tool".to_string(),
            status: StepStatus::Pending,
            input: Some(serde_json::json!({})),
            output: None,
        };
        adapter.write_step(&pending).await.unwrap();

        let done = Step {
            id: "step-002".to_string(),
            agent_id: "agent-1".to_string(),
            step_index: 0,
            idempotency_key: "agent-1:call-002".to_string(),
            step_type: "tool".to_string(),
            status: StepStatus::Done,
            input: None,
            output: Some(serde_json::json!({ "result": "exit 0" })),
        };
        adapter.write_step(&done).await.unwrap();

        let steps = adapter.list_agent_steps("agent-1").await.unwrap();
        assert_eq!(steps.len(), 1, "upsert must not create a second row");
        assert_eq!(steps[0].status, StepStatus::Done);
        assert!(steps[0].output.is_some());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn list_agent_steps_ordered_by_index(pool: PgPool) {
        let adapter = SqlxPostgresAdapter::new(pool);

        for i in 0..3_i32 {
            adapter
                .write_step(&Step {
                    id: format!("step-{i}"),
                    agent_id: "agent-1".to_string(),
                    step_index: i,
                    idempotency_key: format!("agent-1:call-{i}"),
                    step_type: "tool".to_string(),
                    status: StepStatus::Done,
                    input: Some(serde_json::json!({})),
                    output: Some(serde_json::json!({})),
                })
                .await
                .unwrap();
        }

        let steps = adapter.list_agent_steps("agent-1").await.unwrap();
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0].step_index, 0);
        assert_eq!(steps[1].step_index, 1);
        assert_eq!(steps[2].step_index, 2);
    }
}
```

- [ ] **Step 4: Run tests to verify they fail (no compilation yet)**

```bash
cd packages/animaos-rs && cargo test -p anima-daemon postgres 2>&1 | head -20
```

Expected: compile error — `postgres` module not exported.

- [ ] **Step 5: Export postgres module from lib.rs**

In `packages/animaos-rs/crates/anima-daemon/src/lib.rs`, add:

```rust
pub mod postgres;
```

- [ ] **Step 6: Run the Postgres tests**

First ensure a test database exists:
```bash
createdb animaos_test 2>/dev/null || true
export DATABASE_URL=postgres://localhost/animaos_test
```

Then run:
```bash
cd packages/animaos-rs && cargo test -p anima-daemon postgres
```

Expected: all three tests pass.

- [ ] **Step 7: Commit**

```bash
git add packages/animaos-rs/Cargo.toml \
        packages/animaos-rs/crates/anima-daemon/Cargo.toml \
        packages/animaos-rs/crates/anima-daemon/src/postgres.rs \
        packages/animaos-rs/crates/anima-daemon/src/lib.rs
git commit -m "feat(anima-daemon): add SqlxPostgresAdapter with step_log persistence"
```

---

## Task 5: Wire Postgres into daemon startup

**Files:**
- Modify: `packages/animaos-rs/crates/anima-daemon/src/state.rs`
- Modify: `packages/animaos-rs/crates/anima-daemon/src/app.rs`

- [ ] **Step 1: Add set_database to DaemonState**

In `packages/animaos-rs/crates/anima-daemon/src/state.rs`:

Add to imports:
```rust
use anima_core::DatabaseAdapter;
use std::sync::Arc;
```

Add this method to `impl DaemonState`:
```rust
pub(crate) fn set_database(&mut self, db: Arc<dyn DatabaseAdapter>) {
    for runtime in self.agents.values_mut() {
        runtime.set_database(Arc::clone(&db));
    }
    // Store for use when new agents are created
    self.db = Some(db);
}
```

Add `db: Option<Arc<dyn DatabaseAdapter>>` field to `DaemonState`:
```rust
pub(crate) struct DaemonState {
    pub(crate) memory: Arc<Mutex<MemoryManager>>,
    pub(crate) agents: HashMap<String, AgentRuntime>,
    pub(crate) swarms: HashMap<String, SwarmCoordinator>,
    pub(crate) swarm_events: HashMap<String, EventFanout>,
    pub(crate) swarm_snapshots: HashMap<String, SwarmState>,
    pub(crate) model_adapter: Arc<dyn ModelAdapter>,
    pub(crate) tool_registry: ToolRegistry,
    pub(crate) event_fanout: EventFanout,
    pub(crate) db: Option<Arc<dyn DatabaseAdapter>>,  // add this
}
```

Initialize `db: None` in all `DaemonState` constructors (`new`, `with_events`, `with_model_adapter`, `with_model_adapter_and_events`).

In `create_agent`, inject the db into each new runtime:
```rust
pub(crate) fn create_agent(
    &mut self,
    config: AgentConfig,
) -> Result<AgentRuntimeSnapshot, String> {
    self.tool_registry.validate_tools(config.tools.as_deref())?;
    let mut runtime = AgentRuntime::new(config, Arc::clone(&self.model_adapter));
    // Inject database adapter if available
    if let Some(db) = &self.db {
        runtime.set_database(Arc::clone(db));
    }
    runtime.set_providers(default_providers(Arc::clone(&self.memory)));
    runtime.set_evaluators(default_evaluators(Arc::clone(&self.memory)));
    runtime.init();
    let agent_id = runtime.id().to_string();
    let snapshot = runtime.snapshot();
    self.agents.insert(agent_id, runtime);
    Ok(snapshot)
}
```

- [ ] **Step 2: Connect Postgres in serve()**

In `packages/animaos-rs/crates/anima-daemon/src/app.rs`, replace the `serve` function:

```rust
use crate::postgres::SqlxPostgresAdapter;
use sqlx::postgres::PgPoolOptions;

pub async fn serve(listener: TcpListener, config: DaemonConfig) -> io::Result<()> {
    let event_fanout = EventFanout::new(DEFAULT_EVENT_BUFFER);
    let state = Arc::new(Mutex::new(DaemonState::with_model_adapter_and_events(
        Arc::new(RuntimeModelAdapter::from_env()),
        event_fanout,
    )));

    // Connect Postgres if DATABASE_URL is set
    if let Ok(database_url) = std::env::var("DATABASE_URL") {
        match PgPoolOptions::new()
            .max_connections(10)
            .connect(&database_url)
            .await
        {
            Ok(pool) => {
                // Run migrations
                sqlx::migrate!("./migrations")
                    .run(&pool)
                    .await
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

                let adapter = Arc::new(SqlxPostgresAdapter::new(pool));
                state
                    .lock()
                    .expect("state mutex should not be poisoned")
                    .set_database(adapter);

                println!("anima-daemon: Postgres connected, migrations applied");
            }
            Err(e) => {
                eprintln!("anima-daemon: Postgres connection failed: {e} — running without persistence");
            }
        }
    } else {
        eprintln!("anima-daemon: DATABASE_URL not set — running without persistence");
    }

    serve_with_state(listener, state, config).await
}
```

- [ ] **Step 3: Build the daemon to verify it compiles**

```bash
cd packages/animaos-rs && cargo build -p anima-daemon
```

Expected: clean build with no errors.

- [ ] **Step 4: Smoke test — start daemon with Postgres**

```bash
export DATABASE_URL=postgres://localhost/animaos_dev
cd packages/animaos-rs && cargo run -p anima-daemon &
sleep 1
curl -s http://127.0.0.1:8080/health | jq .
```

Expected output: `{"status":"ok"}` and in daemon stdout: `anima-daemon: Postgres connected, migrations applied`.

- [ ] **Step 5: Verify step_log table exists**

```bash
psql $DATABASE_URL -c "\d step_log"
```

Expected: table description with columns `id`, `agent_id`, `step_index`, `idempotency_key`, `type`, `status`, `input`, `output`, `created_at`.

- [ ] **Step 6: Run all daemon tests**

```bash
cd packages/animaos-rs && cargo test -p anima-daemon
```

Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
git add packages/animaos-rs/crates/anima-daemon/src/state.rs \
        packages/animaos-rs/crates/anima-daemon/src/app.rs
git commit -m "feat(anima-daemon): connect Postgres on startup, inject SqlxPostgresAdapter into runtimes"
```

---

## Task 6: End-to-end integration test

**Files:**
- Create: `packages/animaos-rs/crates/anima-daemon/tests/step_log_integration.rs`

- [ ] **Step 1: Write the integration test**

Create `packages/animaos-rs/crates/anima-daemon/tests/step_log_integration.rs`:

```rust
//! Verifies that running an agent via the daemon HTTP API results in step_log rows in Postgres.
//! Requires DATABASE_URL to point to a test Postgres instance.

use std::sync::Arc;

use anima_core::{AgentConfig, DatabaseAdapter};
use anima_daemon::{app_with_state, DaemonConfig};
use anima_daemon::postgres::SqlxPostgresAdapter;
use anima_daemon::state::DaemonState;  // pub(crate) — expose for test with `pub` or use HTTP API
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::json;
use sqlx::PgPool;
use tower::ServiceExt;

// NOTE: This test uses sqlx::test to get an isolated database with migrations applied.
#[sqlx::test(migrations = "./migrations")]
async fn agent_run_writes_steps_to_step_log(pool: PgPool) {
    let adapter = Arc::new(SqlxPostgresAdapter::new(pool.clone()));

    // Build daemon with test state
    use anima_core::persistence::DatabaseAdapter as _;
    use std::sync::Mutex;
    use anima_daemon::events::EventFanout;

    // Use the test model adapter (DeterministicModelAdapter already in daemon)
    let event_fanout = EventFanout::new(128);
    let mut state = DaemonState::with_events(event_fanout);
    state.set_database(Arc::clone(&adapter) as Arc<dyn DatabaseAdapter>);

    let shared_state = Arc::new(Mutex::new(state));
    let app = app_with_state(shared_state.clone(), DaemonConfig::default());

    // Create an agent
    let create_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/agents")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "name": "test-agent",
                        "description": "integration test agent",
                        "systemPrompt": "You are a test agent. Use the echo tool.",
                        "tools": ["echo"],
                        "model": "test"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(create_resp.status(), StatusCode::CREATED);
    let body = axum::body::to_bytes(create_resp.into_body(), usize::MAX).await.unwrap();
    let agent: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let agent_id = agent["id"].as_str().unwrap().to_string();

    // Run the agent
    let run_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/agents/{agent_id}/run"))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "input": "call the echo tool" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(run_resp.status(), StatusCode::OK);

    // Verify step_log has entries for this agent
    let steps = adapter.list_agent_steps(&agent_id).await.unwrap();
    assert!(!steps.is_empty(), "at least one step must be written to step_log");

    let tool_steps: Vec<_> = steps.iter().filter(|s| s.step_type == "tool").collect();
    assert!(!tool_steps.is_empty(), "tool steps must be present");

    for step in &tool_steps {
        assert_ne!(
            step.status,
            anima_core::persistence::StepStatus::Pending,
            "no step should remain pending after a completed run"
        );
    }
}
```

- [ ] **Step 2: Run the integration test**

```bash
export DATABASE_URL=postgres://localhost/animaos_test
cd packages/animaos-rs && cargo test -p anima-daemon --test step_log_integration
```

Expected: `test agent_run_writes_steps_to_step_log ... ok`.

If `DaemonState` visibility needs adjustment, change `pub(crate) struct DaemonState` to `pub struct DaemonState` in `state.rs` and adjust `pub(crate)` fields accordingly. Only make fields `pub` that the test actually needs.

- [ ] **Step 3: Commit**

```bash
git add packages/animaos-rs/crates/anima-daemon/tests/
git commit -m "test(anima-daemon): integration test verifying step_log written during agent run"
```

---

## Self-Review

**Spec coverage check:**
- ✅ `DatabaseAdapter` trait — Task 1
- ✅ `Step` types with `pending`/`done`/`failed` — Task 1
- ✅ Trait injected into `AgentRuntime` — Task 2
- ✅ Write `pending` before side effects (tool call) — Task 2
- ✅ Write `done`/`failed` after side effects — Task 2
- ✅ `UNIQUE (agent_id, step_index)` enforced — Task 3 (migration)
- ✅ Idempotency key on each step — Task 2 + Task 3
- ✅ Postgres connected on daemon startup — Task 5
- ✅ Migrations run on startup — Task 5
- ✅ Host-agnostic (no Postgres in anima-core) — Tasks 1–2
- ✅ Explicit retry-key lookup for completed tool steps — implemented

**What this plan does NOT cover (follow-up plans):**
1. **Recovery/replay** — rehydrating an agent after daemon restart and resuming the in-flight run from persisted state. Explicit retry-key lookup exists, but full process-level replay still requires a richer agent lifecycle.
2. **WASM bindings** — `wasm-bindgen` exports for embedding anima-core in TypeScript. Separate plan.
3. **FFI exports** — C exports for Elixir/Python. Separate plan.
4. **TS package cleanup** — removing `apps/server`, `packages/core`, renaming `packages/sdk` → `packages/client`. Separate plan.
