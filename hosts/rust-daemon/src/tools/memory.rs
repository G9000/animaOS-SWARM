use std::collections::BTreeMap;

use anima_core::{AgentState, Content, DataValue, Message, TaskResult, ToolCall};
use anima_memory::{MemorySearchOptions, MemoryType, NewMemory};
use futures::future::BoxFuture;

use super::ToolExecutionContext;

pub(super) fn execute_memory_search(
    context: ToolExecutionContext,
    agent: AgentState,
    _user_message: Message,
    tool_call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    Box::pin(async move {
        let query = match tool_call.args.get("query") {
            Some(DataValue::String(value)) if !value.is_empty() => value.clone(),
            _ => {
                return TaskResult::error("memory_search query must be a non-empty string", 0);
            }
        };

        let limit = match tool_call.args.get("limit") {
            Some(DataValue::Number(value))
                if value.is_finite() && *value >= 1.0 && value.fract() == 0.0 =>
            {
                *value as usize
            }
            Some(DataValue::Number(_)) | Some(_) => {
                return TaskResult::error("memory_search limit must be a positive integer", 0);
            }
            None => 3,
        };

        let results = context
            .memory
            .read()
            .await
            .search(
                &query,
                MemorySearchOptions {
                    agent_id: Some(agent.id.clone()),
                    limit: Some(limit),
                    ..MemorySearchOptions::default()
                },
            );

        let mut metadata = BTreeMap::new();
        metadata.insert("query".into(), DataValue::String(query));
        metadata.insert("matchCount".into(), DataValue::Number(results.len() as f64));

        let text = if results.is_empty() {
            "no memory matches".to_string()
        } else {
            results
                .into_iter()
                .map(|result| result.content)
                .collect::<Vec<_>>()
                .join("\n")
        };

        TaskResult::success(
            Content {
                text,
                attachments: None,
                metadata: Some(metadata),
            },
            0,
        )
    })
}

pub(super) fn execute_memory_add(
    context: ToolExecutionContext,
    agent: AgentState,
    _user_message: Message,
    tool_call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    Box::pin(async move {
        let content = match tool_call.args.get("content") {
            Some(DataValue::String(value)) if !value.trim().is_empty() => value.trim().to_string(),
            _ => return TaskResult::error("memory_add content must be a non-empty string", 0),
        };

        let memory_type = match tool_call.args.get("type") {
            None => MemoryType::Fact,
            Some(DataValue::String(value)) => match MemoryType::parse(value) {
                Ok(memory_type) => memory_type,
                Err(()) => {
                    return TaskResult::error(
                        "memory_add type must be one of fact, observation, task_result, reflection",
                        0,
                    )
                }
            },
            Some(_) => return TaskResult::error("memory_add type must be a string", 0),
        };

        let importance = match tool_call.args.get("importance") {
            None => 0.8,
            Some(DataValue::Number(value)) if value.is_finite() && (0.0..=1.0).contains(value) => {
                *value
            }
            Some(DataValue::Number(_)) | Some(_) => {
                return TaskResult::error("memory_add importance must be between 0 and 1", 0);
            }
        };

        let memory = match context
            .memory
            .write()
            .await
            .add(NewMemory {
                agent_id: agent.id.clone(),
                agent_name: agent.name.clone(),
                memory_type,
                content: content.clone(),
                importance,
                tags: Some(vec!["runtime".into(), "tool-memory-add".into()]),
            }) {
            Ok(memory) => memory,
            Err(error) => return TaskResult::error(error.message(), 0),
        };

        let mut metadata = BTreeMap::new();
        metadata.insert("memoryId".into(), DataValue::String(memory.id));
        metadata.insert(
            "memoryType".into(),
            DataValue::String(memory.memory_type.as_str().to_string()),
        );

        TaskResult::success(
            Content {
                text: format!("stored memory: {content}"),
                attachments: None,
                metadata: Some(metadata),
            },
            0,
        )
    })
}

pub(super) fn execute_recent_memories(
    context: ToolExecutionContext,
    agent: AgentState,
    _user_message: Message,
    tool_call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    Box::pin(async move {
        let limit = match tool_call.args.get("limit") {
            Some(DataValue::Number(value))
                if value.is_finite() && *value >= 1.0 && value.fract() == 0.0 =>
            {
                *value as usize
            }
            Some(DataValue::Number(_)) | Some(_) => {
                return TaskResult::error("recent_memories limit must be a positive integer", 0);
            }
            None => 3,
        };

        let memories = context
            .memory
            .read()
            .await
            .get_recent(anima_memory::RecentMemoryOptions {
                agent_id: Some(agent.id.clone()),
                agent_name: None,
                limit: Some(limit),
            });

        let mut metadata = BTreeMap::new();
        metadata.insert("limit".into(), DataValue::Number(limit as f64));
        metadata.insert(
            "matchCount".into(),
            DataValue::Number(memories.len() as f64),
        );

        let text = if memories.is_empty() {
            "no recent memories".to_string()
        } else {
            memories
                .into_iter()
                .map(|memory| memory.content)
                .collect::<Vec<_>>()
                .join("\n")
        };

        TaskResult::success(
            Content {
                text,
                attachments: None,
                metadata: Some(metadata),
            },
            0,
        )
    })
}
