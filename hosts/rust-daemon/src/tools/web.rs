use std::collections::BTreeMap;
use std::time::Duration;

use anima_core::{AgentState, Content, DataValue, Message, TaskResult, ToolCall};
use futures::future::BoxFuture;
use reqwest::header::{ACCEPT, CONTENT_TYPE, USER_AGENT};
use serde_json::{json, Value};

use super::ToolExecutionContext;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ExaSearchResultItem {
    pub(super) title: String,
    pub(super) url: String,
    pub(super) excerpt: String,
}

pub(super) fn execute_web_fetch(
    _context: ToolExecutionContext,
    _agent: AgentState,
    _user_message: Message,
    tool_call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    Box::pin(async move {
        let url = match tool_call.args.get("url") {
            Some(DataValue::String(value)) if !value.trim().is_empty() => value.trim().to_string(),
            _ => return TaskResult::error("web_fetch url must be a non-empty string", 0),
        };

        let max_length = match tool_call.args.get("max_length") {
            Some(DataValue::Number(value))
                if value.is_finite() && *value >= 1.0 && value.fract() == 0.0 =>
            {
                *value as usize
            }
            Some(DataValue::Number(_)) | Some(_) => {
                return TaskResult::error("web_fetch max_length must be a positive integer", 0);
            }
            None => 10_000,
        };

        match fetch_web_text(&url, max_length).await {
            Ok((text, content_type)) => {
                let mut metadata = BTreeMap::new();
                metadata.insert("url".into(), DataValue::String(url));
                if !content_type.is_empty() {
                    metadata.insert("contentType".into(), DataValue::String(content_type));
                }

                TaskResult::success(
                    Content {
                        text,
                        attachments: None,
                        metadata: Some(metadata),
                    },
                    0,
                )
            }
            Err(error) => TaskResult::error(error, 0),
        }
    })
}

pub(super) fn execute_exa_search(
    _context: ToolExecutionContext,
    _agent: AgentState,
    _user_message: Message,
    tool_call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    Box::pin(async move {
        let query = match tool_call.args.get("query") {
            Some(DataValue::String(value)) if !value.trim().is_empty() => value.trim().to_string(),
            _ => return TaskResult::error("exa_search query must be a non-empty string", 0),
        };

        let num_results = match tool_call.args.get("num_results") {
            Some(DataValue::Number(value))
                if value.is_finite() && *value >= 1.0 && value.fract() == 0.0 =>
            {
                *value as usize
            }
            Some(DataValue::Number(_)) | Some(_) => {
                return TaskResult::error("exa_search num_results must be a positive integer", 0);
            }
            None => 5,
        };

        let include_text = match tool_call.args.get("include_text") {
            Some(DataValue::Bool(value)) => *value,
            Some(_) => return TaskResult::error("exa_search include_text must be a boolean", 0),
            None => false,
        };

        let max_characters = match tool_call.args.get("max_characters") {
            Some(DataValue::Number(value))
                if value.is_finite() && *value >= 1.0 && value.fract() == 0.0 =>
            {
                *value as usize
            }
            Some(DataValue::Number(_)) | Some(_) => {
                return TaskResult::error(
                    "exa_search max_characters must be a positive integer",
                    0,
                );
            }
            None => 2_000,
        };

        let api_key = match first_non_empty_env_value(&["EXA_API_KEY", "EXA_KEY", "EXA_TOKEN"]) {
            Some(value) => value,
            None => {
                return TaskResult::error(
                    "EXA_API_KEY is not configured for daemon-backed exa_search",
                    0,
                )
            }
        };

        match search_exa(&api_key, &query, num_results, include_text, max_characters).await {
            Ok(results) => {
                let mut metadata = BTreeMap::new();
                metadata.insert("provider".into(), DataValue::String("exa".into()));
                metadata.insert("query".into(), DataValue::String(query));
                metadata.insert("resultCount".into(), DataValue::Number(results.len() as f64));
                metadata.insert(
                    "urls".into(),
                    DataValue::Array(
                        results
                            .iter()
                            .map(|result| DataValue::String(result.url.clone()))
                            .collect(),
                    ),
                );

                TaskResult::success(
                    Content {
                        text: format_exa_results(&results),
                        attachments: None,
                        metadata: Some(metadata),
                    },
                    0,
                )
            }
            Err(error) => TaskResult::error(error, 0),
        }
    })
}

async fn fetch_web_text(url: &str, max_length: usize) -> Result<(String, String), String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|error| format!("web_fetch client init failed: {error}"))?;

    let response = client
        .get(url)
        .header(USER_AGENT, "animaOS-SWARM/0.1")
        .header(ACCEPT, "text/html,application/json,text/plain,*/*")
        .send()
        .await
        .map_err(|error| format!("web_fetch request failed: {error}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let reason = status.canonical_reason().unwrap_or("request failed");
        return Err(format!("HTTP {}: {}", status.as_u16(), reason));
    }

    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();

    let mut text = if content_type.contains("application/json") {
        let json = response
            .json::<serde_json::Value>()
            .await
            .map_err(|error| format!("web_fetch json parse failed: {error}"))?;
        serde_json::to_string_pretty(&json)
            .map_err(|error| format!("web_fetch json formatting failed: {error}"))?
    } else {
        response
            .text()
            .await
            .map_err(|error| format!("web_fetch body read failed: {error}"))?
    };

    if content_type.contains("text/html") {
        text = strip_html_text(&text);
    }

    if text.chars().count() > max_length {
        text = format!("{}\n...[truncated]", text.chars().take(max_length).collect::<String>());
    }

    Ok((text, content_type))
}

async fn search_exa(
    api_key: &str,
    query: &str,
    num_results: usize,
    include_text: bool,
    max_characters: usize,
) -> Result<Vec<ExaSearchResultItem>, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| format!("exa_search client init failed: {error}"))?;

    let mut contents = serde_json::Map::new();
    contents.insert(
        "highlights".into(),
        json!({
            "maxCharacters": max_characters,
            "query": query,
        }),
    );
    if include_text {
        contents.insert("text".into(), Value::Bool(true));
    }

    let response = client
        .post("https://api.exa.ai/search")
        .header("x-api-key", api_key)
        .header(CONTENT_TYPE, "application/json")
        .json(&json!({
            "query": query,
            "numResults": num_results,
            "contents": Value::Object(contents),
        }))
        .send()
        .await
        .map_err(|error| format!("exa_search request failed: {error}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let reason = status.canonical_reason().unwrap_or("request failed");
        return Err(format!("HTTP {}: {}", status.as_u16(), reason));
    }

    let payload = response
        .json::<Value>()
        .await
        .map_err(|error| format!("exa_search response parse failed: {error}"))?;

    parse_exa_results(&payload, include_text, max_characters)
}

pub(super) fn parse_exa_results(
    payload: &Value,
    include_text: bool,
    max_characters: usize,
) -> Result<Vec<ExaSearchResultItem>, String> {
    let Some(results) = payload.get("results").and_then(Value::as_array) else {
        return Err("exa_search response missing results array".into());
    };

    Ok(results
        .iter()
        .map(|result| {
            let title = result
                .get("title")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("Untitled result")
                .to_string();
            let url = result
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();

            let mut excerpts = match result.get("highlights") {
                Some(Value::Array(values)) => values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToString::to_string)
                    .collect::<Vec<_>>(),
                Some(Value::String(value)) if !value.trim().is_empty() => {
                    vec![value.trim().to_string()]
                }
                _ => Vec::new(),
            };

            if excerpts.is_empty() && include_text {
                if let Some(text) = result.get("text").and_then(Value::as_str) {
                    let snippet = truncate_chars(text.trim(), max_characters);
                    if !snippet.is_empty() {
                        excerpts.push(snippet);
                    }
                }
            }

            let excerpt = if excerpts.is_empty() {
                "no excerpt".to_string()
            } else {
                excerpts.join(" ")
            };

            ExaSearchResultItem {
                title,
                url,
                excerpt,
            }
        })
        .collect())
}

fn format_exa_results(results: &[ExaSearchResultItem]) -> String {
    if results.is_empty() {
        return "no exa search results".to_string();
    }

    results
        .iter()
        .enumerate()
        .map(|(index, result)| {
            format!(
                "{}. {}\nURL: {}\nExcerpt: {}",
                index + 1,
                result.title,
                result.url,
                result.excerpt
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn truncate_chars(input: &str, max_characters: usize) -> String {
    if input.chars().count() <= max_characters {
        return input.to_string();
    }

    format!("{}...", input.chars().take(max_characters).collect::<String>())
}

fn first_non_empty_env_value(names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .and_then(|value| (!value.trim().is_empty()).then_some(value))
    })
}

pub(super) fn strip_html_text(input: &str) -> String {
    let without_scripts = strip_html_block(input, "script");
    let without_styles = strip_html_block(&without_scripts, "style");
    let mut output = String::with_capacity(without_styles.len());
    let mut inside_tag = false;
    let mut pending_space = false;

    for ch in without_styles.chars() {
        match ch {
            '<' => {
                inside_tag = true;
                pending_space = true;
            }
            '>' => {
                inside_tag = false;
                pending_space = true;
            }
            _ if inside_tag => {}
            _ if ch.is_whitespace() => pending_space = true,
            _ => {
                if pending_space && !output.is_empty() {
                    output.push(' ');
                }
                output.push(ch);
                pending_space = false;
            }
        }
    }

    output.trim().to_string()
}

fn strip_html_block(input: &str, tag_name: &str) -> String {
    let lowercase = input.to_ascii_lowercase();
    let open_tag = format!("<{tag_name}");
    let close_tag = format!("</{tag_name}>");
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;

    while let Some(relative_start) = lowercase[cursor..].find(&open_tag) {
        let start = cursor + relative_start;
        output.push_str(&input[cursor..start]);

        let Some(relative_end) = lowercase[start..].find(&close_tag) else {
            return output;
        };

        cursor = start + relative_end + close_tag.len();
    }

    output.push_str(&input[cursor..]);
    output
}
