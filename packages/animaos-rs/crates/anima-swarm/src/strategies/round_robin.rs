use std::collections::BTreeMap;
use std::time::Instant;

use anima_core::{Content, DataValue, TaskResult, TaskStatus};

use crate::coordinator::CoordinatorAgentRef;
use crate::coordinator::{CoordinatorDispatchContext, CoordinatorFuture};

pub fn round_robin_strategy(
    ctx: CoordinatorDispatchContext,
) -> CoordinatorFuture<TaskResult<Content>> {
    Box::pin(async move {
        let start = Instant::now();
        let all_configs = std::iter::once(ctx.manager_config().clone())
            .chain(ctx.worker_configs().iter().cloned())
            .collect::<Vec<_>>();

        let mut agents = Vec::with_capacity(all_configs.len());
        for config in all_configs {
            let agent = match ctx.spawn_agent(config.clone()).await {
                Ok(agent) => agent,
                Err(error) => return TaskResult::error(error, start.elapsed().as_millis()),
            };

            agents.push(RoundRobinAgent {
                name: config.name,
                agent,
            });
        }

        if agents.is_empty() {
            return TaskResult::error(
                "Round-robin strategy requires at least one agent",
                start.elapsed().as_millis(),
            );
        }

        let mut history = Vec::new();
        let mut last_result = None;

        for turn in 0..ctx.max_turns() {
            let agent = &agents[turn % agents.len()];
            let prompt = if turn == 0 {
                ctx.task().to_string()
            } else {
                build_follow_up_prompt(ctx.task(), &history)
            };

            let result = agent.agent.run(prompt).await;
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

            history.push(HistoryEntry {
                speaker: agent.name.clone(),
                content: response_text,
            });
            last_result = Some(result);
        }

        let content = build_result_content(&history);
        let duration_ms = start.elapsed().as_millis();
        let final_status = last_result
            .as_ref()
            .map(|result| result.status)
            .unwrap_or(TaskStatus::Success);
        let final_error = last_result.as_ref().and_then(|result| result.error.clone());

        TaskResult {
            status: final_status,
            data: Some(content),
            error: final_error,
            duration_ms,
        }
    })
}

struct RoundRobinAgent {
    name: String,
    agent: CoordinatorAgentRef,
}

struct HistoryEntry {
    speaker: String,
    content: String,
}

fn build_follow_up_prompt(task: &str, history: &[HistoryEntry]) -> String {
    let history_str = if history.is_empty() {
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
    };

    format!("Continue working on this task: {task}\n\nIt's your turn to contribute.{history_str}")
}

fn build_result_content(history: &[HistoryEntry]) -> Content {
    let text = history
        .iter()
        .map(|entry| format!("[{}]: {}", entry.speaker, entry.content))
        .collect::<Vec<_>>()
        .join("\n\n");

    let mut metadata = BTreeMap::new();
    metadata.insert(
        "history".into(),
        DataValue::Array(
            history
                .iter()
                .map(|entry| {
                    let mut record = BTreeMap::new();
                    record.insert("speaker".into(), DataValue::String(entry.speaker.clone()));
                    record.insert("content".into(), DataValue::String(entry.content.clone()));
                    DataValue::Object(record)
                })
                .collect(),
        ),
    );

    Content {
        text,
        attachments: None,
        metadata: Some(metadata),
    }
}
