use std::collections::BTreeMap;

use anima_core::{AgentConfig, DataValue};

pub(super) struct DeterministicToolDispatch {
    pub(super) name: &'static str,
    pub(super) response_text: &'static str,
    pub(super) id_prefix: &'static str,
    pub(super) args: BTreeMap<String, DataValue>,
}

pub(super) fn deterministic_tool_dispatch(
    config: &AgentConfig,
    input: &str,
) -> Option<DeterministicToolDispatch> {
    maybe_memory_add_dispatch(config, input)
        .or_else(|| maybe_recent_memories_dispatch(config, input))
        .or_else(|| maybe_todo_write_dispatch(config, input))
        .or_else(|| maybe_todo_read_dispatch(config, input))
        .or_else(|| maybe_write_file_dispatch(config, input))
        .or_else(|| maybe_read_file_dispatch(config, input))
        .or_else(|| maybe_list_dir_dispatch(config, input))
        .or_else(|| maybe_memory_search_dispatch(config, input))
}

fn has_tool(config: &AgentConfig, tool_name: &str) -> bool {
    config
        .tools
        .as_ref()
        .is_some_and(|tools| tools.iter().any(|tool| tool.name == tool_name))
}

fn maybe_memory_add_dispatch(
    config: &AgentConfig,
    input: &str,
) -> Option<DeterministicToolDispatch> {
    if !has_tool(config, "memory_add") {
        return None;
    }

    let content = parse_prefixed_topic(input, &["remember "])?;
    let mut args = BTreeMap::new();
    args.insert("content".into(), DataValue::String(content));
    args.insert("type".into(), DataValue::String("fact".into()));
    args.insert("importance".into(), DataValue::Number(0.8));

    Some(DeterministicToolDispatch {
        name: "memory_add",
        response_text: "storing memory",
        id_prefix: "tool-call-add-",
        args,
    })
}

fn maybe_recent_memories_dispatch(
    config: &AgentConfig,
    input: &str,
) -> Option<DeterministicToolDispatch> {
    if !has_tool(config, "recent_memories") {
        return None;
    }

    let limit = parse_recent_limit(input)?;
    let mut args = BTreeMap::new();
    args.insert("limit".into(), DataValue::Number(limit as f64));

    Some(DeterministicToolDispatch {
        name: "recent_memories",
        response_text: "loading recent memories",
        id_prefix: "tool-call-recent-",
        args,
    })
}

fn maybe_todo_write_dispatch(
    config: &AgentConfig,
    input: &str,
) -> Option<DeterministicToolDispatch> {
    if !has_tool(config, "todo_write") {
        return None;
    }

    let topic = parse_todo_write_topic(input)?;
    let mut args = BTreeMap::new();
    args.insert(
        "todos".into(),
        DataValue::Array(vec![
            todo_item_data_value(
                &format!("Inspect {topic}"),
                "completed",
                &format!("Inspecting {topic}"),
            ),
            todo_item_data_value(
                &format!("Implement {topic}"),
                "in_progress",
                &format!("Implementing {topic}"),
            ),
            todo_item_data_value(
                &format!("Validate {topic}"),
                "pending",
                &format!("Validating {topic}"),
            ),
        ]),
    );

    Some(DeterministicToolDispatch {
        name: "todo_write",
        response_text: "writing todos",
        id_prefix: "tool-call-todo-write-",
        args,
    })
}

fn maybe_todo_read_dispatch(
    config: &AgentConfig,
    input: &str,
) -> Option<DeterministicToolDispatch> {
    if !has_tool(config, "todo_read") || !should_read_todos(input) {
        return None;
    }

    Some(DeterministicToolDispatch {
        name: "todo_read",
        response_text: "reading todos",
        id_prefix: "tool-call-todo-read-",
        args: BTreeMap::new(),
    })
}

fn maybe_write_file_dispatch(
    config: &AgentConfig,
    input: &str,
) -> Option<DeterministicToolDispatch> {
    if !has_tool(config, "write_file") {
        return None;
    }

    let topic = parse_write_file_topic(input)?;
    let mut args = BTreeMap::new();
    args.insert("file_path".into(), DataValue::String(topic_file_path(&topic)));
    args.insert(
        "content".into(),
        DataValue::String(format!("notes for {topic}")),
    );

    Some(DeterministicToolDispatch {
        name: "write_file",
        response_text: "writing file",
        id_prefix: "tool-call-write-file-",
        args,
    })
}

fn maybe_read_file_dispatch(
    config: &AgentConfig,
    input: &str,
) -> Option<DeterministicToolDispatch> {
    if !has_tool(config, "read_file") {
        return None;
    }

    let topic = parse_read_file_topic(input)?;
    let mut args = BTreeMap::new();
    args.insert("file_path".into(), DataValue::String(topic_file_path(&topic)));

    Some(DeterministicToolDispatch {
        name: "read_file",
        response_text: "reading file",
        id_prefix: "tool-call-read-file-",
        args,
    })
}

fn maybe_list_dir_dispatch(
    config: &AgentConfig,
    input: &str,
) -> Option<DeterministicToolDispatch> {
    if !has_tool(config, "list_dir") || !should_list_notes_dir(input) {
        return None;
    }

    let mut args = BTreeMap::new();
    args.insert("path".into(), DataValue::String("notes".into()));

    Some(DeterministicToolDispatch {
        name: "list_dir",
        response_text: "listing directory",
        id_prefix: "tool-call-list-dir-",
        args,
    })
}

fn maybe_memory_search_dispatch(
    config: &AgentConfig,
    input: &str,
) -> Option<DeterministicToolDispatch> {
    if !has_tool(config, "memory_search") {
        return None;
    }

    let mut args = BTreeMap::new();
    args.insert("query".into(), DataValue::String(input.to_string()));

    Some(DeterministicToolDispatch {
        name: "memory_search",
        response_text: "searching memories",
        id_prefix: "tool-call-",
        args,
    })
}

fn parse_recent_limit(input: &str) -> Option<u64> {
    let trimmed = input.trim();
    if !trimmed.starts_with("recent") {
        return None;
    }

    let suffix = trimmed["recent".len()..].trim();
    if suffix.is_empty() {
        return Some(3);
    }

    suffix.parse::<u64>().ok().filter(|value| *value > 0)
}

fn parse_prefixed_topic(input: &str, prefixes: &[&str]) -> Option<String> {
    prefixes
        .iter()
        .find_map(|prefix| input.strip_prefix(prefix))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn matches_normalized_input(input: &str, candidates: &[&str]) -> bool {
    let normalized = input.trim().to_ascii_lowercase();
    candidates
        .iter()
        .any(|candidate| normalized == candidate.to_ascii_lowercase())
}

fn parse_todo_write_topic(input: &str) -> Option<String> {
    parse_prefixed_topic(input, &["plan ", "todo ", "track "])
}

fn should_read_todos(input: &str) -> bool {
    matches_normalized_input(input, &["read todos", "todo read", "show todos", "list todos"])
}

fn parse_write_file_topic(input: &str) -> Option<String> {
    parse_prefixed_topic(input, &["write file "])
}

fn parse_read_file_topic(input: &str) -> Option<String> {
    parse_prefixed_topic(input, &["read file "])
}

fn should_list_notes_dir(input: &str) -> bool {
    matches_normalized_input(input, &["list notes", "show notes", "list dir notes"])
}

fn topic_file_path(topic: &str) -> String {
    format!("notes/{}.txt", slugify_topic(topic))
}

fn slugify_topic(topic: &str) -> String {
    let mut slug = String::new();
    let mut previous_was_separator = false;

    for character in topic.chars().flat_map(|character| character.to_lowercase()) {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
            previous_was_separator = false;
        } else if !previous_was_separator {
            slug.push('-');
            previous_was_separator = true;
        }
    }

    slug.trim_matches('-').to_string()
}

fn todo_item_data_value(content: &str, status: &str, active_form: &str) -> DataValue {
    DataValue::Object(BTreeMap::from([
        ("content".into(), DataValue::String(content.to_string())),
        ("status".into(), DataValue::String(status.to_string())),
        (
            "activeForm".into(),
            DataValue::String(active_form.to_string()),
        ),
    ]))
}