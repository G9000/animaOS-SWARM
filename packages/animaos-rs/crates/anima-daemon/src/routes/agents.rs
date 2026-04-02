use std::future::ready;
use std::sync::{Arc, Mutex};

use anima_core::{AgentConfig, AgentRuntimeSnapshot, AgentSettings};
use anima_memory::Memory;

use super::api::{
    data_value_json, optional_string_array_json, optional_string_json, parse_agent_config,
    parse_content, plugins_json, task_result_json, token_usage_json, tools_json,
};
use super::Response;
use crate::json::{escape_json, JsonParser};
use crate::state::DaemonState;

pub(crate) fn handle_create_agent(body: Vec<u8>, state: &Arc<Mutex<DaemonState>>) -> Response {
    let body = match std::str::from_utf8(&body) {
        Ok(body) => body,
        Err(_) => {
            return Response::error(
                "HTTP/1.1 400 Bad Request",
                "request body must be valid UTF-8",
            )
        }
    };

    let object = match JsonParser::new(body).parse_object() {
        Ok(object) => object,
        Err(_) => {
            return Response::error(
                "HTTP/1.1 400 Bad Request",
                "request body must be valid JSON",
            )
        }
    };

    let config = match parse_agent_config(&object) {
        Ok(config) => config,
        Err(message) => return Response::error("HTTP/1.1 400 Bad Request", message),
    };

    let snapshot = {
        let mut guard = state
            .lock()
            .expect("daemon state mutex should not be poisoned");
        match guard.create_agent(config) {
            Ok(snapshot) => snapshot,
            Err(message) => {
                return Response::json(
                    "HTTP/1.1 400 Bad Request",
                    format!("{{\"error\":\"{}\"}}", escape_json(&message)),
                )
            }
        }
    };

    Response::json(
        "HTTP/1.1 201 Created",
        format!("{{\"agent\":{}}}", runtime_snapshot_json(&snapshot)),
    )
}

pub(crate) fn handle_list_agents(state: &Arc<Mutex<DaemonState>>) -> Response {
    let snapshots = {
        let guard = state
            .lock()
            .expect("daemon state mutex should not be poisoned");
        guard.list_agents()
    };

    Response::json(
        "HTTP/1.1 200 OK",
        format!(
            "{{\"agents\":[{}]}}",
            snapshots
                .iter()
                .map(runtime_snapshot_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
    )
}

pub(crate) fn handle_get_agent(agent_id: &str, state: &Arc<Mutex<DaemonState>>) -> Response {
    let snapshot = {
        let guard = state
            .lock()
            .expect("daemon state mutex should not be poisoned");
        guard.get_agent(agent_id)
    };

    match snapshot {
        Some(snapshot) => Response::json(
            "HTTP/1.1 200 OK",
            format!("{{\"agent\":{}}}", runtime_snapshot_json(&snapshot)),
        ),
        None => Response::error("HTTP/1.1 404 Not Found", "not found"),
    }
}

pub(crate) fn handle_recent_agent_memories(
    agent_id: &str,
    query: std::collections::HashMap<String, String>,
    state: &Arc<Mutex<DaemonState>>,
) -> Response {
    let limit = match parse_optional_usize(query.get("limit").map(String::as_str)) {
        Ok(value) => value,
        Err(message) => return Response::error("HTTP/1.1 400 Bad Request", message),
    };

    let memories = {
        let guard = state
            .lock()
            .expect("daemon state mutex should not be poisoned");
        guard.recent_memories_for_agent(agent_id, limit)
    };

    match memories {
        Some(memories) => Response::json(
            "HTTP/1.1 200 OK",
            format!("{{\"memories\":[{}]}}", join_memories(&memories)),
        ),
        None => Response::error("HTTP/1.1 404 Not Found", "not found"),
    }
}

pub(crate) async fn handle_run_agent(
    agent_id: &str,
    body: Vec<u8>,
    state: &Arc<Mutex<DaemonState>>,
) -> Response {
    let body = match std::str::from_utf8(&body) {
        Ok(body) => body,
        Err(_) => {
            return Response::error(
                "HTTP/1.1 400 Bad Request",
                "request body must be valid UTF-8",
            )
        }
    };

    let object = match JsonParser::new(body).parse_object() {
        Ok(object) => object,
        Err(_) => {
            return Response::error(
                "HTTP/1.1 400 Bad Request",
                "request body must be valid JSON",
            )
        }
    };

    let content = match parse_content(&object) {
        Ok(content) => content,
        Err(message) => return Response::error("HTTP/1.1 400 Bad Request", message),
    };

    let Some((mut runtime, tool_context)) = ({
        let mut guard = state
            .lock()
            .expect("daemon state mutex should not be poisoned");
        guard.take_agent_runtime(agent_id)
    }) else {
        return Response::error("HTTP/1.1 404 Not Found", "not found");
    };

    let result = runtime
        .run_with_tools(content, |agent, user_message, tool_call| {
            ready(tool_context.execute_tool(agent, user_message, tool_call))
        })
        .await;
    let snapshot = {
        let mut guard = state
            .lock()
            .expect("daemon state mutex should not be poisoned");
        guard.complete_agent_run(runtime, &result)
    };

    Response::json(
        "HTTP/1.1 200 OK",
        format!(
            "{{\"agent\":{},\"result\":{}}}",
            runtime_snapshot_json(&snapshot),
            task_result_json(Some(&result))
        ),
    )
}

fn parse_optional_usize(value: Option<&str>) -> Result<Option<usize>, &'static str> {
    value
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| "limit must be an integer")
        })
        .transpose()
}

fn runtime_snapshot_json(snapshot: &AgentRuntimeSnapshot) -> String {
    format!(
        "{{\"state\":{},\"messageCount\":{},\"eventCount\":{},\"lastTask\":{}}}",
        agent_state_json(&snapshot.state),
        snapshot.message_count,
        snapshot.event_count,
        task_result_json(snapshot.last_task.as_ref())
    )
}

fn agent_state_json(state: &anima_core::AgentState) -> String {
    format!(
        "{{\"id\":\"{}\",\"name\":\"{}\",\"status\":\"{}\",\"config\":{},\"createdAt\":{},\"tokenUsage\":{}}}",
        escape_json(&state.id),
        escape_json(&state.name),
        state.status.as_str(),
        agent_config_json(&state.config),
        state.created_at,
        token_usage_json(&state.token_usage)
    )
}

fn agent_config_json(config: &AgentConfig) -> String {
    format!(
        "{{\"name\":\"{}\",\"model\":\"{}\",\"bio\":{},\"lore\":{},\"knowledge\":{},\"topics\":{},\"adjectives\":{},\"style\":{},\"provider\":{},\"system\":{},\"tools\":{},\"plugins\":{},\"settings\":{}}}",
        escape_json(&config.name),
        escape_json(&config.model),
        optional_string_json(config.bio.as_deref()),
        optional_string_json(config.lore.as_deref()),
        optional_string_array_json(config.knowledge.as_deref()),
        optional_string_array_json(config.topics.as_deref()),
        optional_string_array_json(config.adjectives.as_deref()),
        optional_string_json(config.style.as_deref()),
        optional_string_json(config.provider.as_deref()),
        optional_string_json(config.system.as_deref()),
        tools_json(config.tools.as_deref()),
        plugins_json(config.plugins.as_deref()),
        settings_json(config.settings.as_ref())
    )
}

fn settings_json(settings: Option<&AgentSettings>) -> String {
    let Some(settings) = settings else {
        return "null".to_string();
    };

    let mut fields = Vec::new();
    if let Some(value) = settings.temperature {
        fields.push(format!("\"temperature\":{value}"));
    }
    if let Some(value) = settings.max_tokens {
        fields.push(format!("\"maxTokens\":{value}"));
    }
    if let Some(value) = settings.timeout {
        fields.push(format!("\"timeout\":{value}"));
    }
    if let Some(value) = settings.max_retries {
        fields.push(format!("\"maxRetries\":{value}"));
    }
    for (key, value) in &settings.additional {
        fields.push(format!(
            "\"{}\":{}",
            escape_json(key),
            data_value_json(value)
        ));
    }

    format!("{{{}}}", fields.join(","))
}

fn join_memories(memories: &[Memory]) -> String {
    memories
        .iter()
        .map(memory_json)
        .collect::<Vec<_>>()
        .join(",")
}

fn memory_json(memory: &Memory) -> String {
    format!(
        "{{\"id\":\"{}\",\"agentId\":\"{}\",\"agentName\":\"{}\",\"type\":\"{}\",\"content\":\"{}\",\"importance\":{},\"createdAt\":{},\"tags\":{}}}",
        escape_json(&memory.id),
        escape_json(&memory.agent_id),
        escape_json(&memory.agent_name),
        memory.memory_type.as_str(),
        escape_json(&memory.content),
        memory.importance,
        memory.created_at,
        match memory.tags.as_deref() {
            None => "null".to_string(),
            Some(tags) => format!(
                "[{}]",
                tags.iter()
                    .map(|tag| format!("\"{}\"", escape_json(tag)))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        }
    )
}

#[cfg(test)]
mod tests {
    use super::handle_run_agent;
    use crate::state::DaemonState;
    use anima_core::{
        AgentConfig, Content, ModelAdapter, ModelGenerateRequest, ModelGenerateResponse,
        ModelStopReason, TokenUsage,
    };
    use async_trait::async_trait;
    use futures::executor::block_on;
    use futures::task::noop_waker;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll};

    struct PendingModelAdapter;

    struct PendingOnce<T> {
        value: Option<T>,
        pending: bool,
    }

    #[async_trait]
    impl ModelAdapter for PendingModelAdapter {
        fn provider(&self) -> &str {
            "pending"
        }

        async fn generate(
            &self,
            config: &AgentConfig,
            _request: &ModelGenerateRequest,
        ) -> Result<ModelGenerateResponse, String> {
            Ok(PendingOnce::new(ModelGenerateResponse {
                content: Content {
                    text: format!("{} handled task: pending", config.name),
                    attachments: None,
                    metadata: None,
                },
                tool_calls: None,
                usage: TokenUsage {
                    prompt_tokens: 1,
                    completion_tokens: 1,
                    total_tokens: 2,
                },
                stop_reason: ModelStopReason::End,
            })
            .await)
        }
    }

    impl<T: Unpin> Future for PendingOnce<T> {
        type Output = T;

        fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
            if self.pending {
                self.pending = false;
                context.waker().wake_by_ref();
                Poll::Pending
            } else {
                Poll::Ready(self.value.take().expect("pending-once value should exist"))
            }
        }
    }

    impl<T> PendingOnce<T> {
        fn new(value: T) -> Self {
            Self {
                value: Some(value),
                pending: true,
            }
        }
    }

    #[test]
    fn handle_run_agent_releases_state_lock_before_runtime_future_completes() {
        let state = Arc::new(Mutex::new(DaemonState::with_model_adapter(Arc::new(
            PendingModelAdapter,
        ))));
        let agent_id = {
            let mut guard = state
                .lock()
                .expect("daemon state mutex should not be poisoned");
            guard
                .create_agent(test_config("operator"))
                .expect("agent should be created")
                .state
                .id
        };
        let mut future = Box::pin(handle_run_agent(
            &agent_id,
            br#"{"text":"run pending task"}"#.to_vec(),
            &state,
        ));
        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);

        assert!(
            matches!(future.as_mut().poll(&mut context), Poll::Pending),
            "the first poll should suspend on the pending model adapter"
        );
        assert!(
            state.try_lock().is_ok(),
            "daemon state lock should be released while the runtime future is pending"
        );

        let response = block_on(future);
        assert_eq!(response.status_line, "HTTP/1.1 200 OK");
    }

    fn test_config(name: &str) -> AgentConfig {
        AgentConfig {
            name: name.into(),
            model: "gpt-5.4".into(),
            bio: None,
            lore: None,
            knowledge: None,
            topics: None,
            adjectives: None,
            style: None,
            provider: Some("openai".into()),
            system: None,
            tools: None,
            plugins: None,
            settings: None,
        }
    }
}
