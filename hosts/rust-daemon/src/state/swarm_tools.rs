use std::sync::Arc;

use anima_core::{AgentState, Content, DataValue, Message, TaskResult, ToolCall};
use anima_swarm::coordinator::{
    CoordinatorBatchDelegateFn, CoordinatorBroadcastFn, CoordinatorDelegateFn,
    CoordinatorParticipant, CoordinatorParticipantsFn, CoordinatorSendFn,
};
use anima_swarm::SwarmDelegation;

use crate::tools::ToolExecutionContext;

pub(super) async fn execute_swarm_tool(
    send: Arc<CoordinatorSendFn>,
    broadcast: Arc<CoordinatorBroadcastFn>,
    participants: Arc<CoordinatorParticipantsFn>,
    delegate_task: Option<Arc<CoordinatorDelegateFn>>,
    delegate_tasks: Option<Arc<CoordinatorBatchDelegateFn>>,
    tool_context: ToolExecutionContext,
    agent: AgentState,
    user_message: Message,
    tool_call: ToolCall,
) -> TaskResult<Content> {
    match tool_call.name.as_str() {
        "send_message" => {
            let Some(message) = string_arg(&tool_call, "message") else {
                return TaskResult::error("send_message message must be a string", 0);
            };
            let recipient = match resolve_message_recipient(&tool_call, participants).await {
                Ok(recipient) => recipient,
                Err(error) => return TaskResult::error(error, 0),
            };

            match send(recipient.agent_id.clone(), text_content(message)).await {
                Ok(()) => TaskResult::success(
                    text_content(format!("sent message to {}", recipient.display_name)),
                    0,
                ),
                Err(error) => TaskResult::error(error, 0),
            }
        }
        "broadcast_message" => {
            let Some(message) = string_arg(&tool_call, "message") else {
                return TaskResult::error("broadcast_message message must be a string", 0);
            };

            match broadcast(text_content(message)).await {
                Ok(()) => TaskResult::success(text_content("broadcast message sent to swarm"), 0),
                Err(error) => TaskResult::error(error, 0),
            }
        }
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
        _ => {
            tool_context
                .execute_tool(agent, user_message, tool_call)
                .await
        }
    }
}

struct MessageRecipient {
    agent_id: String,
    display_name: String,
}

async fn resolve_message_recipient(
    tool_call: &ToolCall,
    participants: Arc<CoordinatorParticipantsFn>,
) -> Result<MessageRecipient, String> {
    if let Some(to_agent_id) = string_arg(tool_call, "to_agent_id") {
        return Ok(MessageRecipient {
            agent_id: to_agent_id.clone(),
            display_name: to_agent_id,
        });
    }

    let Some(to_agent_name) = string_arg(tool_call, "to_agent_name") else {
        return Err("send_message requires to_agent_id or to_agent_name".into());
    };
    let participants = participants().await?;
    let matches = participants
        .iter()
        .filter(|participant| participant.agent_name == to_agent_name)
        .collect::<Vec<_>>();

    match matches.as_slice() {
        [participant] => Ok(MessageRecipient {
            agent_id: participant.agent_id.clone(),
            display_name: format!("{} ({})", participant.agent_name, participant.agent_id),
        }),
        [] => Err(format!(
            "No live swarm agent named \"{}\". Available: {}",
            to_agent_name,
            available_participants(&participants)
        )),
        _ => Err(format!(
            "Multiple live swarm agents named \"{}\". Use to_agent_id. Matches: {}",
            to_agent_name,
            matches
                .iter()
                .map(|participant| participant.agent_id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn available_participants(participants: &[CoordinatorParticipant]) -> String {
    if participants.is_empty() {
        return "none".into();
    }

    participants
        .iter()
        .map(|participant| format!("{} ({})", participant.agent_name, participant.agent_id))
        .collect::<Vec<_>>()
        .join(", ")
}

fn text_content(text: impl Into<String>) -> Content {
    Content {
        text: text.into(),
        attachments: None,
        metadata: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use anima_core::{AgentConfig, AgentState, AgentStatus, MessageRole, TaskStatus};
    use anima_memory::MemoryManager;
    use tokio::sync::RwLock as AsyncRwLock;

    use crate::memory_embeddings::MemoryEmbeddingRuntime;
    use crate::tools::{
        new_shared_process_manager_with_limit, ToolRegistry, DEFAULT_MAX_BACKGROUND_PROCESSES,
    };

    #[tokio::test]
    async fn send_message_uses_coordinator_send_hook() {
        let sent = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        let send: Arc<CoordinatorSendFn> = Arc::new({
            let sent = Arc::clone(&sent);
            move |to_agent_id, content| {
                let sent = Arc::clone(&sent);
                Box::pin(async move {
                    sent.lock()
                        .expect("sent messages mutex should not be poisoned")
                        .push((to_agent_id, content.text));
                    Ok(())
                })
            }
        });
        let broadcast = noop_broadcast();
        let participants = noop_participants();

        let result = execute_swarm_tool(
            send,
            broadcast,
            participants,
            None,
            None,
            tool_context(),
            agent_state(),
            user_message(),
            tool_call(
                "send_message",
                BTreeMap::from([
                    ("to_agent_id".into(), DataValue::String("worker-1".into())),
                    ("message".into(), DataValue::String("hello".into())),
                ]),
            ),
        )
        .await;

        assert_eq!(result.status, TaskStatus::Success);
        assert_eq!(
            result.data.as_ref().map(|content| content.text.as_str()),
            Some("sent message to worker-1")
        );
        assert_eq!(
            sent.lock()
                .expect("sent messages mutex should not be poisoned")
                .as_slice(),
            &[("worker-1".into(), "hello".into())]
        );
    }

    #[tokio::test]
    async fn send_message_resolves_configured_agent_name() {
        let sent = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        let send: Arc<CoordinatorSendFn> = Arc::new({
            let sent = Arc::clone(&sent);
            move |to_agent_id, content| {
                let sent = Arc::clone(&sent);
                Box::pin(async move {
                    sent.lock()
                        .expect("sent messages mutex should not be poisoned")
                        .push((to_agent_id, content.text));
                    Ok(())
                })
            }
        });
        let participants: Arc<CoordinatorParticipantsFn> = Arc::new(|| {
            Box::pin(async {
                Ok(vec![CoordinatorParticipant {
                    agent_id: "worker-a-2".into(),
                    agent_name: "worker-a".into(),
                }])
            })
        });

        let result = execute_swarm_tool(
            send,
            noop_broadcast(),
            participants,
            None,
            None,
            tool_context(),
            agent_state(),
            user_message(),
            tool_call(
                "send_message",
                BTreeMap::from([
                    ("to_agent_name".into(), DataValue::String("worker-a".into())),
                    ("message".into(), DataValue::String("named hello".into())),
                ]),
            ),
        )
        .await;

        assert_eq!(result.status, TaskStatus::Success);
        assert_eq!(
            result.data.as_ref().map(|content| content.text.as_str()),
            Some("sent message to worker-a (worker-a-2)")
        );
        assert_eq!(
            sent.lock()
                .expect("sent messages mutex should not be poisoned")
                .as_slice(),
            &[("worker-a-2".into(), "named hello".into())]
        );
    }

    #[tokio::test]
    async fn broadcast_message_uses_coordinator_broadcast_hook() {
        let broadcasted = Arc::new(Mutex::new(Vec::<String>::new()));
        let send = noop_send();
        let broadcast: Arc<CoordinatorBroadcastFn> = Arc::new({
            let broadcasted = Arc::clone(&broadcasted);
            move |content| {
                let broadcasted = Arc::clone(&broadcasted);
                Box::pin(async move {
                    broadcasted
                        .lock()
                        .expect("broadcast messages mutex should not be poisoned")
                        .push(content.text);
                    Ok(())
                })
            }
        });

        let result = execute_swarm_tool(
            send,
            broadcast,
            noop_participants(),
            None,
            None,
            tool_context(),
            agent_state(),
            user_message(),
            tool_call(
                "broadcast_message",
                BTreeMap::from([("message".into(), DataValue::String("team update".into()))]),
            ),
        )
        .await;

        assert_eq!(result.status, TaskStatus::Success);
        assert_eq!(
            result.data.as_ref().map(|content| content.text.as_str()),
            Some("broadcast message sent to swarm")
        );
        assert_eq!(
            broadcasted
                .lock()
                .expect("broadcast messages mutex should not be poisoned")
                .as_slice(),
            &["team update".to_string()]
        );
    }

    fn noop_send() -> Arc<CoordinatorSendFn> {
        Arc::new(|_to_agent_id, _content| Box::pin(async { Ok(()) }))
    }

    fn noop_broadcast() -> Arc<CoordinatorBroadcastFn> {
        Arc::new(|_content| Box::pin(async { Ok(()) }))
    }

    fn noop_participants() -> Arc<CoordinatorParticipantsFn> {
        Arc::new(|| Box::pin(async { Ok(Vec::new()) }))
    }

    fn tool_context() -> ToolExecutionContext {
        ToolExecutionContext::new(
            Arc::new(AsyncRwLock::new(MemoryManager::new())),
            Arc::new(AsyncRwLock::new(MemoryEmbeddingRuntime::disabled())),
            None,
            ToolRegistry::new(),
            new_shared_process_manager_with_limit(DEFAULT_MAX_BACKGROUND_PROCESSES),
        )
    }

    fn agent_state() -> AgentState {
        AgentState {
            id: "agent-1".into(),
            name: "manager".into(),
            status: AgentStatus::Running,
            config: agent_config("manager"),
            created_at: 1,
            token_usage: Default::default(),
        }
    }

    fn user_message() -> Message {
        Message {
            id: "message-1".into(),
            agent_id: "agent-1".into(),
            room_id: "room-1".into(),
            content: text_content("run"),
            role: MessageRole::User,
            created_at: 1,
        }
    }

    fn tool_call(name: &str, args: BTreeMap<String, DataValue>) -> ToolCall {
        ToolCall {
            id: format!("{name}-call"),
            name: name.into(),
            args,
        }
    }

    fn agent_config(name: &str) -> AgentConfig {
        AgentConfig {
            name: name.into(),
            model: "deterministic".into(),
            bio: None,
            lore: None,
            knowledge: None,
            topics: None,
            adjectives: None,
            style: None,
            provider: None,
            system: None,
            tools: None,
            plugins: None,
            settings: None,
        }
    }
}
