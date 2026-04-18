use std::future::ready;
use std::sync::{Arc, Mutex};

use super::contracts::{
    AgentConfigRequest, AgentEnvelope, AgentRecentMemoriesQuery, AgentRunEnvelope,
    AgentRuntimeSnapshotResponse, AgentsEnvelope, DeleteResponse, MemoriesEnvelope,
    MemoryResponse, TaskRequest, TaskResultResponse,
};
use super::ApiError;
use crate::state::DaemonState;

pub(crate) fn handle_create_agent(
    body: Vec<u8>,
    state: &Arc<Mutex<DaemonState>>,
) -> Result<AgentEnvelope, ApiError> {
    let request: AgentConfigRequest = super::parse_json_body(body)?;
    let config = request.into_domain().map_err(ApiError::bad_request_static)?;

    let snapshot = {
        let mut guard = state
            .lock()
            .expect("daemon state mutex should not be poisoned");
        guard
            .create_agent(config)
            .map_err(|message| ApiError::bad_request(message))?
    };

    Ok(AgentEnvelope {
        agent: AgentRuntimeSnapshotResponse::from(&snapshot),
    })
}

pub(crate) fn handle_list_agents(
    state: &Arc<Mutex<DaemonState>>,
) -> Result<AgentsEnvelope, ApiError> {
    let snapshots = {
        let guard = state
            .lock()
            .expect("daemon state mutex should not be poisoned");
        guard.list_agents()
    };

    Ok(AgentsEnvelope {
        agents: snapshots
            .iter()
            .map(AgentRuntimeSnapshotResponse::from)
            .collect(),
    })
}

pub(crate) fn handle_get_agent(
    agent_id: &str,
    state: &Arc<Mutex<DaemonState>>,
) -> Result<AgentEnvelope, ApiError> {
    let snapshot = {
        let guard = state
            .lock()
            .expect("daemon state mutex should not be poisoned");
        guard.get_agent(agent_id)
    };

    match snapshot {
        Some(snapshot) => Ok(AgentEnvelope {
            agent: AgentRuntimeSnapshotResponse::from(&snapshot),
        }),
        None => Err(ApiError::not_found()),
    }
}

pub(crate) fn handle_delete_agent(
    agent_id: &str,
    state: &Arc<Mutex<DaemonState>>,
) -> Result<DeleteResponse, ApiError> {
    state
        .lock()
        .expect("daemon state mutex should not be poisoned")
        .remove_agent(agent_id);

    Ok(DeleteResponse { deleted: true })
}

pub(crate) fn handle_recent_agent_memories(
    agent_id: &str,
    query: AgentRecentMemoriesQuery,
    state: &Arc<Mutex<DaemonState>>,
) -> Result<MemoriesEnvelope, ApiError> {
    let memories = {
        let guard = state
            .lock()
            .expect("daemon state mutex should not be poisoned");
        guard.recent_memories_for_agent(agent_id, query.limit)
    };

    match memories {
        Some(memories) => Ok(MemoriesEnvelope {
            memories: memories.iter().map(MemoryResponse::from).collect(),
        }),
        None => Err(ApiError::not_found()),
    }
}

pub(crate) async fn handle_run_agent(
    agent_id: &str,
    body: Vec<u8>,
    state: &Arc<Mutex<DaemonState>>,
) -> Result<AgentRunEnvelope, ApiError> {
    let request: TaskRequest = super::parse_json_body(body)?;
    let content = request.into_domain().map_err(ApiError::bad_request_static)?;

    let Some((mut runtime, tool_context)) = ({
        let mut guard = state
            .lock()
            .expect("daemon state mutex should not be poisoned");
        guard.take_agent_runtime(agent_id)
    }) else {
        return Err(ApiError::not_found());
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

    Ok(AgentRunEnvelope {
        agent: AgentRuntimeSnapshotResponse::from(&snapshot),
        result: TaskResultResponse::from(&result),
    })
}

#[cfg(test)]
mod tests {
    use super::handle_run_agent;
    use crate::state::DaemonState;
    use anima_core::{
        AgentConfig, AgentSettings, Content, ModelAdapter, ModelGenerateRequest,
        ModelGenerateResponse, ModelStopReason, TokenUsage,
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
        assert!(response.is_ok());
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
            settings: Some(AgentSettings::default()),
        }
    }
}