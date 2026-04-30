use std::collections::BTreeMap;

use anima_core::{
    AgentConfig, Content, DataValue, Message, MessageRole, ModelAdapter, ModelGenerateRequest,
    TaskResult,
};
use futures::executor::block_on;

use super::DeterministicModelAdapter;

#[test]
fn deterministic_adapter_aggregates_trailing_tool_messages() {
    let adapter = DeterministicModelAdapter;
    let request = ModelGenerateRequest {
        system: "You are helpful".into(),
        messages: vec![
            message("msg-1", "room-1", MessageRole::User, "search both"),
            message(
                "msg-2",
                "room-1",
                MessageRole::Assistant,
                "searching memories",
            ),
            tool_message(
                "msg-3",
                "room-1",
                "alpha result",
                TaskResult::success(content("alpha result"), 1),
            ),
            tool_message(
                "msg-4",
                "room-1",
                "beta result",
                TaskResult::success(content("beta result"), 2),
            ),
        ],
        temperature: None,
        max_tokens: None,
    };

    let response =
        block_on(adapter.generate(&config_with_memory_search(), &request)).expect("adapter should generate");

    assert!(
        response.content.text.contains("alpha result"),
        "response should include the first tool result: {}",
        response.content.text
    );
    assert!(
        response.content.text.contains("beta result"),
        "response should include the second tool result: {}",
        response.content.text
    );
}

#[test]
fn deterministic_adapter_issues_todo_write_and_read_calls() {
    let adapter = DeterministicModelAdapter;

    let write_response = block_on(adapter.generate(
        &config_with_tools(&["todo_write", "todo_read"]),
        &ModelGenerateRequest {
            system: "You are helpful".into(),
            messages: vec![message("msg-1", "room-1", MessageRole::User, "plan release patch")],
            temperature: None,
            max_tokens: None,
        },
    ))
    .expect("adapter should generate todo write call");

    let tool_call = write_response
        .tool_calls
        .as_ref()
        .and_then(|calls| calls.first())
        .expect("todo write should produce a tool call");
    assert_eq!(tool_call.name, "todo_write");
    assert!(matches!(tool_call.args.get("todos"), Some(DataValue::Array(values)) if values.len() == 3));

    let read_response = block_on(adapter.generate(
        &config_with_tools(&["todo_read"]),
        &ModelGenerateRequest {
            system: "You are helpful".into(),
            messages: vec![message("msg-2", "room-1", MessageRole::User, "read todos")],
            temperature: None,
            max_tokens: None,
        },
    ))
    .expect("adapter should generate todo read call");

    let read_tool_call = read_response
        .tool_calls
        .as_ref()
        .and_then(|calls| calls.first())
        .expect("todo read should produce a tool call");
    assert_eq!(read_tool_call.name, "todo_read");
}

#[test]
fn deterministic_adapter_issues_file_tool_calls() {
    let adapter = DeterministicModelAdapter;

    let write_response = block_on(adapter.generate(
        &config_with_tools(&["write_file", "read_file", "list_dir"]),
        &ModelGenerateRequest {
            system: "You are helpful".into(),
            messages: vec![message(
                "msg-1",
                "room-1",
                MessageRole::User,
                "write file release patch",
            )],
            temperature: None,
            max_tokens: None,
        },
    ))
    .expect("adapter should generate write_file call");

    let write_tool_call = write_response
        .tool_calls
        .as_ref()
        .and_then(|calls| calls.first())
        .expect("write_file should produce a tool call");
    assert_eq!(write_tool_call.name, "write_file");
    assert_eq!(
        write_tool_call.args.get("file_path"),
        Some(&DataValue::String("notes/release-patch.txt".into()))
    );

    let read_response = block_on(adapter.generate(
        &config_with_tools(&["read_file"]),
        &ModelGenerateRequest {
            system: "You are helpful".into(),
            messages: vec![message(
                "msg-2",
                "room-1",
                MessageRole::User,
                "read file release patch",
            )],
            temperature: None,
            max_tokens: None,
        },
    ))
    .expect("adapter should generate read_file call");

    let read_tool_call = read_response
        .tool_calls
        .as_ref()
        .and_then(|calls| calls.first())
        .expect("read_file should produce a tool call");
    assert_eq!(read_tool_call.name, "read_file");

    let list_response = block_on(adapter.generate(
        &config_with_tools(&["list_dir"]),
        &ModelGenerateRequest {
            system: "You are helpful".into(),
            messages: vec![message("msg-3", "room-1", MessageRole::User, "list notes")],
            temperature: None,
            max_tokens: None,
        },
    ))
    .expect("adapter should generate list_dir call");

    let list_tool_call = list_response
        .tool_calls
        .as_ref()
        .and_then(|calls| calls.first())
        .expect("list_dir should produce a tool call");
    assert_eq!(list_tool_call.name, "list_dir");
    assert_eq!(
        list_tool_call.args.get("path"),
        Some(&DataValue::String("notes".into()))
    );
}

fn config_with_memory_search() -> AgentConfig {
    config_with_tools(&["memory_search"])
}

fn config_with_tools(tool_names: &[&str]) -> AgentConfig {
    AgentConfig {
        name: "reviewer".into(),
        model: "gpt-5.4".into(),
        bio: None,
        lore: None,
        knowledge: None,
        topics: None,
        adjectives: None,
        style: None,
        provider: Some("openai".into()),
        system: None,
        tools: Some(
            tool_names
                .iter()
                .map(|tool_name| anima_core::ToolDescriptor {
                    name: (*tool_name).into(),
                    description: format!("Tool {tool_name}"),
                    parameters: BTreeMap::new(),
                    examples: None,
                })
                .collect(),
        ),
        plugins: None,
        settings: None,
    }
}

fn message(id: &str, room_id: &str, role: MessageRole, text: &str) -> Message {
    Message {
        id: id.into(),
        agent_id: "agent-1".into(),
        room_id: room_id.into(),
        content: content(text),
        role,
        created_at: 1,
    }
}

fn tool_message(id: &str, room_id: &str, text: &str, result: TaskResult<Content>) -> Message {
    let mut metadata = BTreeMap::new();
    metadata.insert("toolCallId".into(), DataValue::String(id.into()));
    metadata.insert("taskResult".into(), task_result_data_value(&result));

    Message {
        id: id.into(),
        agent_id: "agent-1".into(),
        room_id: room_id.into(),
        content: Content {
            text: text.into(),
            attachments: None,
            metadata: Some(metadata),
        },
        role: MessageRole::Tool,
        created_at: 1,
    }
}

fn content(text: &str) -> Content {
    Content {
        text: text.into(),
        attachments: None,
        metadata: None,
    }
}

fn task_result_data_value(result: &TaskResult<Content>) -> DataValue {
    let mut value = BTreeMap::new();
    value.insert(
        "status".into(),
        DataValue::String(result.status.as_str().to_string()),
    );
    value.insert(
        "data".into(),
        match result.data.as_ref() {
            Some(content) => {
                let mut content_value = BTreeMap::new();
                content_value.insert("text".into(), DataValue::String(content.text.clone()));
                content_value.insert("attachments".into(), DataValue::Null);
                content_value.insert("metadata".into(), DataValue::Null);
                DataValue::Object(content_value)
            }
            None => DataValue::Null,
        },
    );
    value.insert(
        "error".into(),
        result
            .error
            .as_ref()
            .map(|error| DataValue::String(error.clone()))
            .unwrap_or(DataValue::Null),
    );
    value.insert(
        "durationMs".into(),
        DataValue::Number(result.duration_ms as f64),
    );
    DataValue::Object(value)
}