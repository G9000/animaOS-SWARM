use std::collections::{BTreeMap, HashMap};
use std::str;
use std::sync::{Arc, Mutex};

use anima_memory::{
    Memory, MemorySearchOptions, MemorySearchResult, MemoryType, NewMemory, RecentMemoryOptions,
};

use super::Response;
use crate::json::{escape_json, JsonParser, JsonValue};
use crate::state::DaemonState;

pub(crate) fn handle_create_memory(body: Vec<u8>, state: &Arc<Mutex<DaemonState>>) -> Response {
    let body = match str::from_utf8(&body) {
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

    let agent_id = match required_string(&object, "agentId") {
        Ok(value) => value,
        Err(message) => return Response::error("HTTP/1.1 400 Bad Request", message),
    };
    let agent_name = match required_string(&object, "agentName") {
        Ok(value) => value,
        Err(message) => return Response::error("HTTP/1.1 400 Bad Request", message),
    };
    let memory_type = match required_memory_type(&object) {
        Ok(value) => value,
        Err(message) => return Response::error("HTTP/1.1 400 Bad Request", message),
    };
    let content = match required_string(&object, "content") {
        Ok(value) => value,
        Err(message) => return Response::error("HTTP/1.1 400 Bad Request", message),
    };
    let importance = match required_importance(&object) {
        Ok(value) => value,
        Err(message) => return Response::error("HTTP/1.1 400 Bad Request", message),
    };
    let tags = match optional_string_array(&object, "tags") {
        Ok(value) => value,
        Err(message) => return Response::error("HTTP/1.1 400 Bad Request", message),
    };

    let memory = {
        let memory = {
            let guard = state
                .lock()
                .expect("daemon state mutex should not be poisoned");
            Arc::clone(&guard.memory)
        };
        let mut memory_guard = memory.lock().expect("memory mutex should not be poisoned");
        match memory_guard.add(NewMemory {
            agent_id,
            agent_name,
            memory_type,
            content,
            importance,
            tags,
        }) {
            Ok(memory) => memory,
            Err(error) => {
                return Response::error("HTTP/1.1 400 Bad Request", error.message());
            }
        }
    };

    Response::json("HTTP/1.1 201 Created", memory_json(&memory))
}

pub(crate) fn handle_search_memories(
    query: HashMap<String, String>,
    state: &Arc<Mutex<DaemonState>>,
) -> Response {
    let Some(search_query) = query.get("q").filter(|value| !value.is_empty()).cloned() else {
        return Response::error("HTTP/1.1 400 Bad Request", "q query parameter is required");
    };

    let memory_type = match query.get("type") {
        None => None,
        Some(value) => match MemoryType::parse(value) {
            Ok(memory_type) => Some(memory_type),
            Err(_) => {
                return Response::error(
                    "HTTP/1.1 400 Bad Request",
                    "type must be a valid memory type",
                )
            }
        },
    };
    let limit = match parse_optional_usize(query.get("limit").map(String::as_str)) {
        Ok(value) => value,
        Err(message) => return Response::error("HTTP/1.1 400 Bad Request", message),
    };
    let min_importance = match parse_optional_f64(query.get("minImportance").map(String::as_str)) {
        Ok(value) => value,
        Err(message) => return Response::error("HTTP/1.1 400 Bad Request", message),
    };

    let results = {
        let memory = {
            let guard = state
                .lock()
                .expect("daemon state mutex should not be poisoned");
            Arc::clone(&guard.memory)
        };
        let memory_guard = memory.lock().expect("memory mutex should not be poisoned");
        memory_guard.search(
            &search_query,
            MemorySearchOptions {
                agent_id: query.get("agentId").cloned(),
                agent_name: query.get("agentName").cloned(),
                memory_type,
                limit,
                min_importance,
            },
        )
    };

    Response::json(
        "HTTP/1.1 200 OK",
        format!("{{\"results\":[{}]}}", join_search_results(&results)),
    )
}

pub(crate) fn handle_recent_memories(
    query: HashMap<String, String>,
    state: &Arc<Mutex<DaemonState>>,
) -> Response {
    let limit = match parse_optional_usize(query.get("limit").map(String::as_str)) {
        Ok(value) => value,
        Err(message) => return Response::error("HTTP/1.1 400 Bad Request", message),
    };

    let memories = {
        let memory = {
            let guard = state
                .lock()
                .expect("daemon state mutex should not be poisoned");
            Arc::clone(&guard.memory)
        };
        let memory_guard = memory.lock().expect("memory mutex should not be poisoned");
        memory_guard.get_recent(RecentMemoryOptions {
            agent_id: query.get("agentId").cloned(),
            agent_name: query.get("agentName").cloned(),
            limit,
        })
    };

    Response::json(
        "HTTP/1.1 200 OK",
        format!("{{\"memories\":[{}]}}", join_memories(&memories)),
    )
}

fn required_string(
    object: &BTreeMap<String, JsonValue>,
    key: &'static str,
) -> Result<String, &'static str> {
    match object.get(key) {
        Some(JsonValue::String(value)) if !value.is_empty() => Ok(value.clone()),
        _ => Err(match key {
            "agentId" => "agentId is required",
            "agentName" => "agentName is required",
            "content" => "content is required",
            _ => "required string field is missing",
        }),
    }
}

fn required_memory_type(object: &BTreeMap<String, JsonValue>) -> Result<MemoryType, &'static str> {
    let Some(JsonValue::String(value)) = object.get("type") else {
        return Err("type is required");
    };
    MemoryType::parse(value)
        .map_err(|_| "type must be one of fact, observation, task_result, reflection")
}

fn required_importance(object: &BTreeMap<String, JsonValue>) -> Result<f64, &'static str> {
    let Some(JsonValue::Number(value)) = object.get("importance") else {
        return Err("importance is required");
    };
    if !(0.0..=1.0).contains(value) {
        return Err("importance must be between 0 and 1");
    }
    Ok(*value)
}

fn optional_string_array(
    object: &BTreeMap<String, JsonValue>,
    key: &'static str,
) -> Result<Option<Vec<String>>, &'static str> {
    let Some(value) = object.get(key) else {
        return Ok(None);
    };
    match value {
        JsonValue::Null => Ok(None),
        JsonValue::Array(values) => values
            .iter()
            .map(|value| match value {
                JsonValue::String(value) => Ok(value.clone()),
                _ => Err("tags must be an array of strings"),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        _ => Err("tags must be an array of strings"),
    }
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

fn parse_optional_f64(value: Option<&str>) -> Result<Option<f64>, &'static str> {
    value
        .map(|value| {
            let parsed = value
                .parse::<f64>()
                .map_err(|_| "minImportance must be a number")?;
            if parsed.is_finite() && (0.0..=1.0).contains(&parsed) {
                Ok(parsed)
            } else {
                Err("minImportance must be between 0 and 1")
            }
        })
        .transpose()
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
        tags_json(memory.tags.as_deref())
    )
}

fn search_result_json(result: &MemorySearchResult) -> String {
    format!(
        "{{\"id\":\"{}\",\"agentId\":\"{}\",\"agentName\":\"{}\",\"type\":\"{}\",\"content\":\"{}\",\"importance\":{},\"createdAt\":{},\"tags\":{},\"score\":{}}}",
        escape_json(&result.id),
        escape_json(&result.agent_id),
        escape_json(&result.agent_name),
        result.memory_type.as_str(),
        escape_json(&result.content),
        result.importance,
        result.created_at,
        tags_json(result.tags.as_deref()),
        result.score
    )
}

fn tags_json(tags: Option<&[String]>) -> String {
    match tags {
        None => "null".to_string(),
        Some(tags) => format!(
            "[{}]",
            tags.iter()
                .map(|tag| format!("\"{}\"", escape_json(tag)))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

fn join_memories(memories: &[Memory]) -> String {
    memories
        .iter()
        .map(memory_json)
        .collect::<Vec<_>>()
        .join(",")
}

fn join_search_results(results: &[MemorySearchResult]) -> String {
    results
        .iter()
        .map(search_result_json)
        .collect::<Vec<_>>()
        .join(",")
}
