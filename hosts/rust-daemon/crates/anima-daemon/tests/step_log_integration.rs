use anima_core::persistence::{DatabaseAdapter, Step, StepStatus};
use anima_daemon::postgres::SqlxPostgresAdapter;
use sqlx::PgPool;

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
async fn write_step_upserts_to_done(pool: PgPool) {
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
    assert_eq!(steps.len(), 1, "upsert must not create duplicate row");
    assert_eq!(steps[0].status, StepStatus::Done);
    assert!(steps[0].output.is_some());
}

#[sqlx::test(migrations = "./migrations")]
async fn list_steps_ordered_by_index(pool: PgPool) {
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
