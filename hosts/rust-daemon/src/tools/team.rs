use super::ToolExecutionContext;
use anima_core::{AgentState, Content, DataValue, Message, TaskResult, ToolCall};
use futures::future::BoxFuture;

pub(super) fn send_message(
    context: ToolExecutionContext,
    agent: AgentState,
    _message: Message,
    call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    Box::pin(async move {
        let (Some(coordinator), Some(route)) = (context.team, context.peer_route) else {
            return TaskResult::error("Peer communication requires a workspace agent run", 0);
        };
        let arg = |key: &str| match call.args.get(key) {
            Some(DataValue::String(value)) if !value.is_empty() => Some(value.as_str()),
            _ => None,
        };
        let Some(message) = arg("message") else {
            return TaskResult::error("message is required", 0);
        };
        let target = match coordinator
            .resolve_peer(arg("to_agent_id"), arg("to_agent_name"))
            .await
        {
            Ok(target) => target,
            Err(error) => return TaskResult::error(error, 0),
        };
        match coordinator.send_peer(agent.id, target.id.clone(), message.into(), route).await {
            Ok(result) => TaskResult::success(Content { text: serde_json::json!({"fromAgentId": target.id, "fromAgentName": target.name, "status": result.result.status, "result": result.result.data, "error": result.result.error}).to_string(), ..Content::default() }, 0),
            Err(error) => TaskResult::error(format!("Peer request failed: {}", error.message()), 0),
        }
    })
}

pub(super) fn broadcast_message(
    context: ToolExecutionContext,
    agent: AgentState,
    _message: Message,
    call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    Box::pin(async move {
        let (Some(coordinator), Some(route)) = (context.team, context.peer_route) else {
            return TaskResult::error("Peer communication requires a workspace agent run", 0);
        };
        let Some(DataValue::String(message)) = call.args.get("message") else {
            return TaskResult::error("message is required", 0);
        };
        if message.is_empty() {
            return TaskResult::error("message is required", 0);
        }
        let mut deliveries = vec![];
        for target in coordinator
            .peer_ids()
            .await
            .into_iter()
            .filter(|id| !route.participants().contains(id))
        {
            let delivery = match coordinator
                .send_peer(
                    agent.id.clone(),
                    target.clone(),
                    message.clone(),
                    route.clone(),
                )
                .await
            {
                Ok(result) => {
                    serde_json::json!({"agentId": target, "status": result.result.status, "result": result.result.data, "error": result.result.error})
                }
                Err(error) => {
                    serde_json::json!({"agentId": target, "status": "error", "error": error.message()})
                }
            };
            deliveries.push(delivery);
        }
        TaskResult::success(
            Content {
                text: serde_json::json!({"deliveries": deliveries}).to_string(),
                ..Content::default()
            },
            0,
        )
    })
}

pub(super) fn list_workspace_agents(
    context: ToolExecutionContext,
    _agent: AgentState,
    _message: Message,
    _call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    Box::pin(async move {
        let Some(coordinator) = context.team else {
            return TaskResult::error(
                "Workspace roster is unavailable in this execution context",
                0,
            );
        };
        TaskResult::success(
            Content {
                text: coordinator.team_roster().await,
                attachments: None,
                metadata: None,
            },
            0,
        )
    })
}

pub(super) fn delegate_to_agent(
    context: ToolExecutionContext,
    agent: AgentState,
    _message: Message,
    call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    Box::pin(async move {
        if !context.can_delegate {
            return TaskResult::error(
                "Only the workspace manager can delegate; recursive delegation is not allowed",
                0,
            );
        }
        let Some(coordinator) = context.team else {
            return TaskResult::error("Delegation is unavailable in this execution context", 0);
        };
        let arg = |key: &str| match call.args.get(key) {
            Some(DataValue::String(value)) if !value.trim().is_empty() => {
                Some(value.trim().to_owned())
            }
            _ => None,
        };
        let (Some(target), Some(task)) = (arg("agent_id"), arg("task")) else {
            return TaskResult::error("agent_id and task are required", 0);
        };
        match coordinator.delegate(&agent, target, task).await {
            Ok(text) => TaskResult::success(
                Content {
                    text,
                    attachments: None,
                    metadata: None,
                },
                0,
            ),
            Err(error) => TaskResult::error(error, 0),
        }
    })
}
