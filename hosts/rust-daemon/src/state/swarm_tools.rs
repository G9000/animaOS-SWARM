use std::sync::Arc;

use anima_core::{AgentState, Content, DataValue, Message, TaskResult, ToolCall};
use anima_swarm::coordinator::{CoordinatorBatchDelegateFn, CoordinatorDelegateFn};
use anima_swarm::SwarmDelegation;

use crate::tools::ToolExecutionContext;

pub(super) async fn execute_swarm_tool(
    delegate_task: Option<Arc<CoordinatorDelegateFn>>,
    delegate_tasks: Option<Arc<CoordinatorBatchDelegateFn>>,
    tool_context: ToolExecutionContext,
    agent: AgentState,
    user_message: Message,
    tool_call: ToolCall,
) -> TaskResult<Content> {
    match tool_call.name.as_str() {
        "delegate_task" => {
            let Some(delegate_task) = delegate_task else {
                return TaskResult::error("delegate_task is unavailable", 0);
            };

            let Some(worker_name) = string_arg(&tool_call, "worker_name") else {
                return TaskResult::error("delegate_task worker_name must be a string", 0);
            };
            let Some(task) = string_arg(&tool_call, "task") else {
                return TaskResult::error("delegate_task task must be a string", 0);
            };

            delegate_task(worker_name, task).await
        }
        "delegate_tasks" => {
            let Some(delegate_tasks) = delegate_tasks else {
                return TaskResult::error("delegate_tasks is unavailable", 0);
            };

            let Some(delegations) = delegation_args(&tool_call, "delegations") else {
                return TaskResult::error(
                    "delegate_tasks delegations must be a non-empty array of objects",
                    0,
                );
            };

            delegate_tasks(delegations).await
        }
        "choose_speaker" => {
            let Some(delegate_task) = delegate_task else {
                return TaskResult::error("choose_speaker is unavailable", 0);
            };

            let Some(agent_name) = string_arg(&tool_call, "agent_name") else {
                return TaskResult::error("choose_speaker agent_name must be a string", 0);
            };
            let instruction = string_arg(&tool_call, "instruction").unwrap_or_default();

            delegate_task(agent_name, instruction).await
        }
        _ => tool_context.execute_tool(agent, user_message, tool_call).await,
    }
}

fn string_arg(tool_call: &ToolCall, key: &str) -> Option<String> {
    match tool_call.args.get(key) {
        Some(DataValue::String(value)) if !value.is_empty() => Some(value.clone()),
        _ => None,
    }
}

fn delegation_args(tool_call: &ToolCall, key: &str) -> Option<Vec<SwarmDelegation>> {
    let DataValue::Array(values) = tool_call.args.get(key)? else {
        return None;
    };

    let mut delegations = Vec::with_capacity(values.len());
    for value in values {
        let DataValue::Object(entry) = value else {
            return None;
        };
        let Some(DataValue::String(worker_name)) = entry.get("worker_name") else {
            return None;
        };
        let Some(DataValue::String(task)) = entry.get("task") else {
            return None;
        };
        if worker_name.is_empty() || task.is_empty() {
            return None;
        }

        delegations.push(SwarmDelegation {
            worker_name: worker_name.clone(),
            task: task.clone(),
        });
    }

    if delegations.is_empty() {
        return None;
    }

    Some(delegations)
}