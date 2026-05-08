use std::collections::BTreeMap;
use std::time::Instant;

use anima_core::{Content, DataValue, TaskResult, TaskStatus};
use futures::future::try_join_all;

use crate::coordinator::CoordinatorAgentRef;
use crate::coordinator::{CoordinatorDispatchContext, CoordinatorFuture};
use crate::strategies::elapsed_ms;

pub fn round_robin_strategy(
    ctx: CoordinatorDispatchContext,
) -> CoordinatorFuture<TaskResult<Content>> {
    Box::pin(async move {
        let start = Instant::now();
        let max_turns = ctx.max_turns();
        if max_turns == 0 {
            return TaskResult::error(
                "Round-robin strategy requires at least one turn",
                elapsed_ms(start),
            );
        }

        let all_configs = std::iter::once(ctx.manager_config().clone())
            .chain(ctx.worker_configs().iter().cloned())
            .collect::<Vec<_>>();

        let agents = match try_join_all(all_configs.into_iter().map(|config| {
            let agent_name = config.name.clone();
            let ctx = ctx.clone();
            async move {
                ctx.spawn_agent(config).await.map(|agent| RoundRobinAgent {
                    name: agent_name,
                    agent,
                })
            }
        }))
        .await
        {
            Ok(agents) => agents,
            Err(error) => return TaskResult::error(error, elapsed_ms(start)),
        };

        let mut history = Vec::new();
        let mut turn_errors: Vec<TurnError> = Vec::new();

        for turn in 0..max_turns {
            let agent = &agents[turn % agents.len()];
            let prompt = if turn == 0 {
                ctx.task().to_string()
            } else {
                build_follow_up_prompt(ctx.task(), &history)
            };

            let result = agent
                .agent
                .run_content(ctx.scoped_text_content(
                    format!("round-robin:turn:{turn}:agent:{}", agent.name),
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
                let error = result.error.as_deref().unwrap_or("unknown error");
                turn_errors.push(TurnError {
                    turn,
                    speaker: agent.name.clone(),
                    error: error.to_string(),
                });
                format!("Error: {error}")
            };

            history.push(HistoryEntry {
                speaker: agent.name.clone(),
                content: response_text,
            });
        }

        let content = build_result_content(&history, &turn_errors);
        let duration_ms = elapsed_ms(start);

        if turn_errors.is_empty() {
            TaskResult::success(content, duration_ms)
        } else {
            let summary = turn_errors
                .iter()
                .map(|entry| format!("[{}] {}", entry.speaker, entry.error))
                .collect::<Vec<_>>()
                .join("; ");
            TaskResult {
                status: TaskStatus::Error,
                data: Some(content),
                error: Some(summary),
                duration_ms,
            }
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

struct TurnError {
    turn: usize,
    speaker: String,
    error: String,
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

fn build_result_content(history: &[HistoryEntry], errors: &[TurnError]) -> Content {
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
    if !errors.is_empty() {
        metadata.insert(
            "errors".into(),
            DataValue::Array(
                errors
                    .iter()
                    .map(|entry| {
                        let mut record = BTreeMap::new();
                        record.insert("turn".into(), DataValue::Number(entry.turn as f64));
                        record.insert(
                            "speaker".into(),
                            DataValue::String(entry.speaker.clone()),
                        );
                        record.insert("error".into(), DataValue::String(entry.error.clone()));
                        DataValue::Object(record)
                    })
                    .collect(),
            ),
        );
    }

    Content {
        text,
        attachments: None,
        metadata: Some(metadata),
    }
}
