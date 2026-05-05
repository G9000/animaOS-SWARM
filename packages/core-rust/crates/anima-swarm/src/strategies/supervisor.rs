use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anima_core::{Content, DataValue, TaskResult, ToolDescriptor};
use futures::future::{join_all, try_join_all};
use tokio::sync::Semaphore;

use crate::coordinator::{
    CoordinatorBatchDelegateFn, CoordinatorDelegateFn, CoordinatorDispatchContext,
    CoordinatorFuture,
};
use crate::types::SwarmDelegation;

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
        let available_workers = worker_names
            .iter()
            .map(|name| format!("\"{name}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let max_parallel_delegations = ctx.max_parallel_delegations().max(1);
        let worker_refs = match try_join_all(ctx.worker_configs().iter().cloned().map(|config| {
            let worker_name = config.name.clone();
            let ctx = ctx.clone();
            async move {
                ctx.spawn_agent(config)
                    .await
                    .map(|worker| (worker_name, worker))
            }
        }))
        .await
        {
            Ok(workers) => workers.into_iter().collect::<HashMap<_, _>>(),
            Err(error) => return TaskResult::error(error, start.elapsed().as_millis()),
        };

        let worker_refs = Arc::new(worker_refs);
        let delegation_limit = Arc::new(Semaphore::new(max_parallel_delegations));
        let delegate_task: Arc<CoordinatorDelegateFn> = {
            let ctx = ctx.clone();
            let worker_refs = Arc::clone(&worker_refs);
            let available_workers = available_workers.clone();
            let delegation_limit = Arc::clone(&delegation_limit);
            Arc::new(move |worker_name: String, task: String| {
                let ctx = ctx.clone();
                let worker_refs = Arc::clone(&worker_refs);
                let available_workers = available_workers.clone();
                let delegation_limit = Arc::clone(&delegation_limit);
                Box::pin(async move {
                    let Some(worker) = worker_refs.get(&worker_name) else {
                        return TaskResult::error(
                            format!(
                                "Worker \"{worker_name}\" not found. Available: {}",
                                available_workers
                            ),
                            0,
                        );
                    };

                    let _permit = delegation_limit
                        .acquire_owned()
                        .await
                        .expect("delegation semaphore should not close");
                    worker
                        .run_content(ctx.scoped_text_content(
                            format!("supervisor:delegate:{worker_name}:{task}"),
                            task,
                        ))
                        .await
                })
            })
        };
        let delegate_tasks: Arc<CoordinatorBatchDelegateFn> =
            {
                let ctx = ctx.clone();
                let worker_refs = Arc::clone(&worker_refs);
                let available_workers = available_workers.clone();
                let delegation_limit = Arc::clone(&delegation_limit);
                Arc::new(move |delegations: Vec<SwarmDelegation>| {
                    let ctx = ctx.clone();
                    let worker_refs = Arc::clone(&worker_refs);
                    let available_workers = available_workers.clone();
                    let delegation_limit = Arc::clone(&delegation_limit);
                    Box::pin(async move {
                        if delegations.is_empty() {
                            return TaskResult::error(
                                "delegate_tasks requires at least one delegation",
                                0,
                            );
                        }

                        let results: Vec<(String, TaskResult<Content>)> =
                            join_all(delegations.into_iter().enumerate().map(
                                |(index, delegation)| {
                                    let ctx = ctx.clone();
                                    let worker_refs = Arc::clone(&worker_refs);
                                    let available_workers = available_workers.clone();
                                    let delegation_limit = Arc::clone(&delegation_limit);
                                    async move {
                                        let SwarmDelegation { worker_name, task } = delegation;
                                        let Some(worker) = worker_refs.get(&worker_name) else {
                                            let missing_worker = worker_name.clone();
                                            return (
                                                worker_name,
                                                TaskResult::error(
                                                    format!(
                                            "Worker \"{missing_worker}\" not found. Available: {}",
                                            available_workers
                                        ),
                                                    0,
                                                ),
                                            );
                                        };

                                        let _permit = delegation_limit
                                            .acquire_owned()
                                            .await
                                            .expect("delegation semaphore should not close");
                                        let result = worker
                                            .run_content(ctx.scoped_text_content(
                                                format!(
                                            "supervisor:delegate-batch:{index}:{worker_name}:{task}"
                                        ),
                                                task,
                                            ))
                                            .await;
                                        (worker_name, result)
                                    }
                                },
                            ))
                            .await;

                        let mut lines = Vec::with_capacity(results.len());
                        let mut any_error = false;
                        let mut metadata = BTreeMap::new();
                        metadata.insert(
                            "delegationCount".into(),
                            DataValue::Number(results.len() as f64),
                        );
                        metadata.insert(
                            "parallelLimit".into(),
                            DataValue::Number(max_parallel_delegations as f64),
                        );
                        metadata.insert(
                            "results".into(),
                            DataValue::Array(
                                results
                                    .iter()
                                    .map(|(worker_name, result): &(String, TaskResult<Content>)| {
                                        let mut entry = BTreeMap::new();
                                        entry.insert(
                                            "worker_name".into(),
                                            DataValue::String(worker_name.clone()),
                                        );
                                        entry.insert(
                                            "status".into(),
                                            DataValue::String(result.status.as_str().into()),
                                        );
                                        if let Some(content) = result.data.as_ref() {
                                            entry.insert(
                                                "text".into(),
                                                DataValue::String(content.text.clone()),
                                            );
                                        }
                                        if let Some(error) = result.error.as_ref() {
                                            entry.insert(
                                                "error".into(),
                                                DataValue::String(error.clone()),
                                            );
                                        }
                                        DataValue::Object(entry)
                                    })
                                    .collect(),
                            ),
                        );

                        for (worker_name, result) in results {
                            match result.status {
                                anima_core::TaskStatus::Success => {
                                    let text = result
                                        .data
                                        .as_ref()
                                        .map(|content| content.text.as_str())
                                        .unwrap_or_default();
                                    lines.push(format!("[{worker_name}] {text}"));
                                }
                                anima_core::TaskStatus::Error => {
                                    any_error = true;
                                    let error = result.error.as_deref().unwrap_or("unknown error");
                                    lines.push(format!("[{worker_name}] Error: {error}"));
                                }
                            }
                        }

                        let content = Content {
                            text: lines.join("\n"),
                            attachments: None,
                            metadata: Some(metadata),
                        };

                        if any_error {
                            TaskResult {
                                status: anima_core::TaskStatus::Error,
                                data: Some(content),
                                error: Some("one or more delegated subtasks failed".into()),
                                duration_ms: 0,
                            }
                        } else {
                            TaskResult::success(content, 0)
                        }
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
            parameters_schema: delegate_task_parameters(),
            examples: None,
        };
        let delegate_tasks_tool = ToolDescriptor {
            name: "delegate_tasks".into(),
            description: format!(
                "Delegate multiple independent subtasks concurrently. Available workers: {}. At most {} delegations run at once.",
                worker_names
                    .iter()
                    .map(|name| format!("\"{name}\""))
                    .collect::<Vec<_>>()
                    .join(", "),
                max_parallel_delegations,
            ),
            parameters_schema: delegate_tasks_parameters(),
            examples: None,
        };

        let mut tools = manager_config.tools.take().unwrap_or_default();
        tools.push(delegate_tool);
        tools.push(delegate_tasks_tool);
        manager_config.tools = Some(tools);
        manager_config.system = Some(supervisor_system_prompt(
            manager_config.system.take(),
            &worker_names,
            max_parallel_delegations,
        ));

        let manager = match ctx
            .spawn_manager(manager_config, delegate_task, Some(delegate_tasks))
            .await
        {
            Ok(manager) => manager,
            Err(error) => return TaskResult::error(error, start.elapsed().as_millis()),
        };

        let result = manager
            .run_content(ctx.scoped_task_content("supervisor:manager"))
            .await;
        let duration_ms = start.elapsed().as_millis();

        TaskResult {
            status: result.status,
            data: result.data,
            error: result.error,
            duration_ms,
        }
    })
}

fn supervisor_system_prompt(
    existing: Option<String>,
    worker_names: &[String],
    max_parallel_delegations: usize,
) -> String {
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
    prompt.push_str(&format!(
        ".\nUse the delegate_task tool for one subtask and delegate_tasks for multiple independent subtasks that should run concurrently. Keep total concurrent delegations at or below {max_parallel_delegations}.\n\
        \n\
        SYNTHESIS GUIDELINES:\n\
        When you synthesize worker results, your job is NOT to force consensus. Workers were chosen for distinct perspectives — preserve that.\n\
        - Surface disagreements explicitly. If two workers reach different conclusions, name both and explain the tension. Do not paper over conflicts.\n\
        - Quote or attribute strong, specific claims to the worker who made them, especially when their angle differs from peers.\n\
        - When the question genuinely has multiple defensible answers, present them as alternatives rather than averaging into a single blurred answer.\n\
        - Only converge on one answer when the workers themselves clearly agree, or when the task demands a single decision and you can justify the choice over the rejected alternatives.\n\
        - Treat dissent from a worker (especially a designated skeptic) as a signal worth keeping, not noise to smooth over."
    ));
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

fn delegate_tasks_parameters() -> BTreeMap<String, DataValue> {
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

    let mut delegation_properties = BTreeMap::new();
    delegation_properties.insert("worker_name".into(), DataValue::Object(worker_name));
    delegation_properties.insert("task".into(), DataValue::Object(task));

    let mut delegation_required = Vec::new();
    delegation_required.push(DataValue::String("worker_name".into()));
    delegation_required.push(DataValue::String("task".into()));

    let mut delegation_item = BTreeMap::new();
    delegation_item.insert("type".into(), DataValue::String("object".into()));
    delegation_item.insert(
        "properties".into(),
        DataValue::Object(delegation_properties),
    );
    delegation_item.insert("required".into(), DataValue::Array(delegation_required));

    let mut delegations = BTreeMap::new();
    delegations.insert("type".into(), DataValue::String("array".into()));
    delegations.insert("items".into(), DataValue::Object(delegation_item));
    delegations.insert(
        "description".into(),
        DataValue::String("Independent subtasks to delegate concurrently".into()),
    );

    let mut properties = BTreeMap::new();
    properties.insert("delegations".into(), DataValue::Object(delegations));

    let mut params = BTreeMap::new();
    params.insert("type".into(), DataValue::String("object".into()));
    params.insert("properties".into(), DataValue::Object(properties));
    params.insert(
        "required".into(),
        DataValue::Array(vec![DataValue::String("delegations".into())]),
    );
    params
}
