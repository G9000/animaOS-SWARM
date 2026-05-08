use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anima_core::TaskStatus;
use anima_core::{Content, DataValue, LockRecover, TaskResult, ToolDescriptor};
use futures::future::try_join_all;

use crate::coordinator::{CoordinatorDelegateFn, CoordinatorDispatchContext, CoordinatorFuture};
use crate::strategies::elapsed_ms;

#[derive(Clone)]
struct HistoryEntry {
    speaker: String,
    content: String,
}

pub fn dynamic_strategy(ctx: CoordinatorDispatchContext) -> CoordinatorFuture<TaskResult<Content>> {
    Box::pin(async move {
        let start = Instant::now();

        let worker_names = ctx
            .worker_configs()
            .iter()
            .map(|config| config.name.clone())
            .collect::<Vec<_>>();
        let available_agents = worker_names
            .iter()
            .map(|name| format!("\"{name}\""))
            .collect::<Vec<_>>()
            .join(", ");

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
            Ok(workers) => workers.into_iter().collect::<BTreeMap<_, _>>(),
            Err(error) => return TaskResult::error(error, elapsed_ms(start)),
        };

        let worker_refs = Arc::new(worker_refs);
        let chat_history = Arc::new(Mutex::new(Vec::<HistoryEntry>::new()));

        let choose_speaker: Arc<CoordinatorDelegateFn> = {
            let ctx = ctx.clone();
            let worker_refs = Arc::clone(&worker_refs);
            let chat_history = Arc::clone(&chat_history);
            let available_agents = available_agents.clone();
            Arc::new(move |agent_name: String, instruction: String| {
                let ctx = ctx.clone();
                let worker_refs = Arc::clone(&worker_refs);
                let chat_history = Arc::clone(&chat_history);
                let available_agents = available_agents.clone();
                Box::pin(async move {
                    if agent_name == "DONE" {
                        return TaskResult::success(
                            Content {
                                text: "DONE".into(),
                                attachments: None,
                                metadata: None,
                            },
                            0,
                        );
                    }

                    let Some(worker) = worker_refs.get(&agent_name) else {
                        return TaskResult::error(
                            format!(
                                "Agent \"{agent_name}\" not found. Available: {}",
                                available_agents
                            ),
                            0,
                        );
                    };

                    let history_text = {
                        let history = chat_history
                            .lock_recover();
                        if history.is_empty() {
                            String::new()
                        } else {
                            format!(
                                "\n\nConversation so far:\n{}",
                                history
                                    .iter()
                                    .map(|entry| format!("[{}]: {}", entry.speaker, entry.content))
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            )
                        }
                    };

                    let prompt = format!("{instruction}{history_text}");
                    let result = worker
                        .run_content(ctx.scoped_text_content(
                            format!("dynamic:speaker:{agent_name}:{prompt}"),
                            prompt,
                        ))
                        .await;
                    let response_text = if result.status == TaskStatus::Success {
                        result
                            .data
                            .as_ref()
                            .map(|content| content.text.clone())
                            .unwrap_or_default()
                    } else {
                        format!(
                            "Error: {}",
                            result.error.as_deref().unwrap_or("unknown error")
                        )
                    };

                    chat_history
                        .lock_recover()
                        .push(HistoryEntry {
                            speaker: agent_name,
                            content: response_text,
                        });

                    result
                })
            })
        };

        let mut manager_config = ctx.manager_config().clone();
        // The dynamic manager spends each tool turn on a `choose_speaker` call,
        // so cap its tool-iteration budget at the swarm's `max_turns` setting.
        let max_turns = ctx.max_turns().max(1);
        let mut settings = manager_config.settings.take().unwrap_or_default();
        settings.max_tool_iterations = Some(max_turns);
        manager_config.settings = Some(settings);
        let choose_speaker_tool = ToolDescriptor {
            name: "choose_speaker".into(),
            description: format!(
                "Choose which agent speaks next. Available agents: {}. Set agent_name to \"DONE\" to end the conversation.",
                worker_names
                    .iter()
                    .map(|name| format!("\"{name}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
                    parameters_schema: choose_speaker_parameters(),
            examples: None,
        };

        let mut tools = manager_config.tools.take().unwrap_or_default();
        tools.push(choose_speaker_tool);
        manager_config.tools = Some(tools);
        manager_config.system = Some(dynamic_system_prompt(
            manager_config.system.take(),
            &worker_names,
        ));

        let manager = match ctx
            .spawn_manager(manager_config, choose_speaker, None)
            .await
        {
            Ok(manager) => manager,
            Err(error) => return TaskResult::error(error, elapsed_ms(start)),
        };

        let result = manager
            .run_content(ctx.scoped_task_content("dynamic:manager"))
            .await;
        let duration_ms = elapsed_ms(start);

        TaskResult {
            status: result.status,
            data: result.data,
            error: result.error,
            duration_ms,
        }
    })
}

fn dynamic_system_prompt(existing: Option<String>, worker_names: &[String]) -> String {
    let mut prompt = existing.unwrap_or_default();
    if !prompt.is_empty() {
        prompt.push_str("\n\n");
    }
    prompt.push_str(
        "You are a dynamic orchestrator agent. You have worker agents available to choose from.\n",
    );
    prompt.push_str("Available agents: ");
    prompt.push_str(
        &worker_names
            .iter()
            .map(|name| format!("\"{name}\""))
            .collect::<Vec<_>>()
            .join(", "),
    );
    prompt.push_str(".\nUse the choose_speaker tool to select which agent speaks next. Set agent_name to \"DONE\" when you are ready to finish and provide your final synthesis.");
    prompt
}

fn choose_speaker_parameters() -> BTreeMap<String, DataValue> {
    let mut agent_name = BTreeMap::new();
    agent_name.insert("type".into(), DataValue::String("string".into()));
    agent_name.insert(
        "description".into(),
        DataValue::String("Name of the agent to speak next, or DONE to finish".into()),
    );

    let mut instruction = BTreeMap::new();
    instruction.insert("type".into(), DataValue::String("string".into()));
    instruction.insert(
        "description".into(),
        DataValue::String("What you want this agent to address".into()),
    );

    let mut properties = BTreeMap::new();
    properties.insert("agent_name".into(), DataValue::Object(agent_name));
    properties.insert("instruction".into(), DataValue::Object(instruction));

    let mut required = Vec::new();
    required.push(DataValue::String("agent_name".into()));

    let mut params = BTreeMap::new();
    params.insert("type".into(), DataValue::String("object".into()));
    params.insert("properties".into(), DataValue::Object(properties));
    params.insert("required".into(), DataValue::Array(required));
    params
}
