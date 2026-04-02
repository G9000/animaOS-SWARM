use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anima_core::{Content, DataValue, TaskResult, ToolDescriptor};

use crate::coordinator::{CoordinatorDelegateFn, CoordinatorDispatchContext, CoordinatorFuture};

pub fn supervisor_strategy(
    ctx: CoordinatorDispatchContext,
) -> CoordinatorFuture<TaskResult<Content>> {
    Box::pin(async move {
        let start = Instant::now();
        let worker_names = ctx
            .worker_configs()
            .iter()
            .map(|config| config.name.clone())
            .collect::<Vec<_>>();
        let mut worker_refs = HashMap::new();
        for config in ctx.worker_configs().iter().cloned() {
            let worker_name = config.name.clone();
            match ctx.spawn_agent(config).await {
                Ok(worker) => {
                    worker_refs.insert(worker_name, worker);
                }
                Err(error) => return TaskResult::error(error, start.elapsed().as_millis()),
            }
        }

        let worker_refs = Arc::new(worker_refs);
        let delegate_task: Arc<CoordinatorDelegateFn> = {
            let worker_refs = Arc::clone(&worker_refs);
            Arc::new(move |worker_name: String, task: String| {
                let worker_refs = Arc::clone(&worker_refs);
                Box::pin(async move {
                    let Some(worker) = worker_refs.get(&worker_name) else {
                        return TaskResult::error(
                            format!(
                                "Worker \"{worker_name}\" not found. Available: {}",
                                worker_refs.keys().cloned().collect::<Vec<_>>().join(", ")
                            ),
                            0,
                        );
                    };

                    worker.run(task).await
                })
            })
        };

        let mut manager_config = ctx.manager_config().clone();
        let delegate_tool = ToolDescriptor {
            name: "delegate_task".into(),
            description: format!(
                "Delegate a subtask to a worker agent. Available workers: {}",
                worker_names
                    .iter()
                    .map(|name| format!("\"{name}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            parameters: delegate_task_parameters(),
            examples: None,
        };

        let mut tools = manager_config.tools.take().unwrap_or_default();
        tools.push(delegate_tool);
        manager_config.tools = Some(tools);
        manager_config.system = Some(supervisor_system_prompt(
            manager_config.system.take(),
            &worker_names,
        ));

        let manager = match ctx.spawn_manager(manager_config, delegate_task).await {
            Ok(manager) => manager,
            Err(error) => return TaskResult::error(error, start.elapsed().as_millis()),
        };

        let result = manager.run(ctx.task().to_string()).await;
        let duration_ms = start.elapsed().as_millis();

        TaskResult {
            status: result.status,
            data: result.data,
            error: result.error,
            duration_ms,
        }
    })
}

fn supervisor_system_prompt(existing: Option<String>, worker_names: &[String]) -> String {
    let mut prompt = existing.unwrap_or_default();
    if !prompt.is_empty() {
        prompt.push_str("\n\n");
    }
    prompt.push_str(
        "You are a supervisor agent. You have worker agents available to delegate tasks to.\n",
    );
    prompt.push_str("Available workers: ");
    prompt.push_str(
        &worker_names
            .iter()
            .map(|name| format!("\"{name}\""))
            .collect::<Vec<_>>()
            .join(", "),
    );
    prompt.push_str(".\nUse the delegate_task tool to assign subtasks. Synthesize the results into a final answer.");
    prompt
}

fn delegate_task_parameters() -> BTreeMap<String, DataValue> {
    let mut required = Vec::new();
    required.push(DataValue::String("worker_name".into()));
    required.push(DataValue::String("task".into()));

    let mut worker_name = BTreeMap::new();
    worker_name.insert("type".into(), DataValue::String("string".into()));
    worker_name.insert(
        "description".into(),
        DataValue::String("Name of the worker to delegate to".into()),
    );

    let mut task = BTreeMap::new();
    task.insert("type".into(), DataValue::String("string".into()));
    task.insert(
        "description".into(),
        DataValue::String("The subtask to delegate".into()),
    );

    let mut properties = BTreeMap::new();
    properties.insert("worker_name".into(), DataValue::Object(worker_name));
    properties.insert("task".into(), DataValue::Object(task));

    let mut params = BTreeMap::new();
    params.insert("type".into(), DataValue::String("object".into()));
    params.insert("properties".into(), DataValue::Object(properties));
    params.insert("required".into(), DataValue::Array(required));
    params
}
