use super::{
    filesystem::{
        edit::{
            edit_workspace_file_from_root, multi_edit_workspace_file_from_root,
            write_workspace_file_from_root,
        },
        execute_read_file,
        search::{
            compile_glob_matcher, glob_workspace_paths_from_root, grep_workspace_files_from_root,
            list_workspace_dir_from_root, read_workspace_file_from_root,
        },
        FileEditOperation,
    },
    new_shared_process_manager_with_limit,
    process::{
        background::{
            list_background_processes, new_shared_process_manager, read_background_process_output,
            set_background_process_limit, start_background_process_from_root,
            stop_background_process,
        },
        shell::execute_bash_command_from_root,
    },
    todo::{
        read_todo_list_from_root, todo_file_path_from_root, write_todo_list_from_root, TodoItem,
    },
    utility::{current_time_iso_utc, evaluate_expression},
    web::{parse_exa_results, strip_html_text},
    workspace_root_path, ToolExecutionContext, ToolRegistry, DEFAULT_MAX_BACKGROUND_PROCESSES,
};
use crate::memory_embeddings::MemoryEmbeddingRuntime;
use anima_core::{
    AgentConfig, AgentState, AgentStatus, Content, DataValue, Message, MessageRole, TaskStatus,
    ToolCall, ToolDescriptor,
};
use anima_memory::MemoryManager;
use chrono::DateTime;
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock as AsyncRwLock;

#[tokio::test]
async fn tool_execution_context_rejects_registered_but_unconfigured_write_tool() {
    let relative_path = format!(
        "target/denied-host-write-{}-{}.txt",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos()
    );
    let absolute_path = std::env::current_dir()
        .expect("current directory")
        .join(&relative_path);
    assert!(
        !absolute_path.exists(),
        "test output path must start absent"
    );
    let context = ToolExecutionContext::new(
        Arc::new(AsyncRwLock::new(MemoryManager::new())),
        Arc::new(AsyncRwLock::new(MemoryEmbeddingRuntime::disabled())),
        None,
        ToolRegistry::new(),
        new_shared_process_manager_with_limit(DEFAULT_MAX_BACKGROUND_PROCESSES),
        None,
    );
    let agent = AgentState {
        id: "agent-denied".into(),
        name: "denied".into(),
        status: AgentStatus::Running,
        config: AgentConfig {
            name: "denied".into(),
            model: "deterministic".into(),
            bio: None,
            lore: None,
            knowledge: None,
            topics: None,
            adjectives: None,
            style: None,
            provider: None,
            system: None,
            tools: None,
            plugins: None,
            settings: None,
        },
        created_at_ms: 1,
        token_usage: Default::default(),
    };
    let user_message = Message {
        id: "message-denied".into(),
        agent_id: agent.id.clone(),
        room_id: "room-denied".into(),
        content: Content {
            text: "write outside permissions".into(),
            ..Content::default()
        },
        role: MessageRole::User,
        created_at_ms: 1,
    };
    let result = context
        .execute_tool(
            agent,
            user_message,
            ToolCall {
                id: "write-denied".into(),
                name: "write_file".into(),
                args: BTreeMap::from([
                    ("file_path".into(), DataValue::String(relative_path.into())),
                    ("content".into(), DataValue::String("must not write".into())),
                ]),
            },
        )
        .await;
    let wrote_file = absolute_path.exists();
    if wrote_file {
        std::fs::remove_file(&absolute_path).expect("clean up unauthorized test write");
    }

    assert_eq!(result.status, TaskStatus::Error);
    assert_eq!(
        result.error.as_deref(),
        Some("tool 'write_file' is not configured for this agent")
    );
    assert!(
        !wrote_file,
        "host dispatch must not invoke the write handler"
    );
}

#[test]
fn registry_resolves_canonical_descriptors() {
    let registry = ToolRegistry::new();

    let descriptors = registry
        .resolve_descriptors(["read_file", "write_file", "bash"])
        .expect("canonical descriptors");

    assert_eq!(
        descriptors
            .iter()
            .map(|descriptor| descriptor.name.as_str())
            .collect::<Vec<_>>(),
        vec!["read_file", "write_file", "bash"]
    );
    assert!(descriptors
        .iter()
        .all(|descriptor| !descriptor.description.trim().is_empty()));
    assert_required_parameters(&descriptors[0], &["file_path"]);
    assert_required_parameters(&descriptors[1], &["file_path", "content"]);
    assert_required_parameters(&descriptors[2], &["command"]);
}

#[test]
fn registry_rejects_unknown_descriptor_without_partial_results() {
    let registry = ToolRegistry::new();

    let error = registry
        .resolve_descriptors(["read_file", "missing_tool", "bash"])
        .expect_err("unknown slug should reject the whole request");

    assert_eq!(error, "unknown tool 'missing_tool'");
}

#[test]
fn registry_descriptor_returns_a_canonical_clone() {
    let registry = ToolRegistry::new();
    let canonical = registry
        .descriptor("read_file")
        .expect("registered descriptor");
    let mut changed = canonical.clone();
    changed.description = "caller-owned mutation".into();

    assert_eq!(
        registry.descriptor("read_file"),
        Some(canonical),
        "mutating a returned descriptor must not alter the registry"
    );
    assert_ne!(registry.descriptor("read_file"), Some(changed));
    assert_eq!(registry.descriptor("missing_tool"), None);
}

#[test]
#[should_panic(expected = "duplicate tool registration 'read_file'")]
fn registry_rejects_duplicate_registration() {
    let mut registry = ToolRegistry::new();
    let duplicate = registry
        .descriptor("read_file")
        .expect("read_file descriptor");

    registry.register(duplicate, execute_read_file);
}

#[test]
fn registry_defines_every_registered_tool_schema() {
    let registry = ToolRegistry::new();
    let expectations = [
        ("memory_search", &["query"][..], &["limit"][..]),
        ("memory_add", &["content"][..], &["type", "importance"][..]),
        ("recent_memories", &[][..], &["limit"][..]),
        ("web_fetch", &["url"][..], &["max_length"][..]),
        (
            "exa_search",
            &["query"][..],
            &["num_results", "include_text", "max_characters"][..],
        ),
        ("get_current_time", &[][..], &[][..]),
        ("calculate", &["expression"][..], &[][..]),
        ("read_file", &["file_path"][..], &["offset", "limit"][..]),
        ("list_dir", &["path"][..], &[][..]),
        ("glob", &["pattern"][..], &["path"][..]),
        ("grep", &["pattern"][..], &["path", "include"][..]),
        ("write_file", &["file_path", "content"][..], &[][..]),
        (
            "edit_file",
            &["file_path", "old_string", "new_string"][..],
            &[][..],
        ),
        ("multi_edit", &["file_path", "edits"][..], &[][..]),
        ("todo_write", &["todos"][..], &[][..]),
        ("todo_read", &[][..], &[][..]),
        ("bash", &["command"][..], &["timeout", "cwd"][..]),
        ("bg_start", &["command"][..], &["cwd"][..]),
        ("bg_output", &["id"][..], &["all"][..]),
        ("bg_stop", &["id"][..], &[][..]),
        ("bg_list", &[][..], &[][..]),
        (
            "send_message",
            &["message"][..],
            &["to_agent_id", "to_agent_name"][..],
        ),
        ("broadcast_message", &["message"][..], &[][..]),
    ];

    assert_eq!(registry.tool_names().len(), expectations.len());
    for (name, required, optional) in expectations {
        let descriptor = registry.descriptor(name).expect("registered descriptor");
        assert!(
            !descriptor.description.trim().is_empty(),
            "{name} must have a model-facing description"
        );
        assert_object_parameters(&descriptor, required, optional);
    }
}

#[test]
fn registry_encodes_parameter_constraints() {
    let registry = ToolRegistry::new();

    for (tool, property) in [
        ("memory_search", "limit"),
        ("recent_memories", "limit"),
        ("web_fetch", "max_length"),
        ("exa_search", "num_results"),
        ("exa_search", "max_characters"),
        ("bash", "timeout"),
    ] {
        assert_property_schema(&registry, tool, property, "integer", Some(1.0), None);
    }
    for property in ["offset", "limit"] {
        assert_property_schema(&registry, "read_file", property, "integer", Some(0.0), None);
    }
    assert_property_schema(
        &registry,
        "memory_add",
        "importance",
        "number",
        Some(0.0),
        Some(1.0),
    );
    assert_string_enum(
        &registry,
        "memory_add",
        "type",
        &["fact", "observation", "task_result", "reflection"],
    );
    assert_property_schema(
        &registry,
        "exa_search",
        "include_text",
        "boolean",
        None,
        None,
    );
    assert_property_schema(&registry, "bg_output", "all", "boolean", None, None);
    assert_array_items(
        &registry,
        "multi_edit",
        "edits",
        &["old_string", "new_string"],
    );
    assert_array_items(
        &registry,
        "todo_write",
        "todos",
        &["content", "status", "activeForm"],
    );

    let todo_write = registry
        .descriptor("todo_write")
        .expect("todo_write descriptor");
    let todo_items = array_item_properties(&todo_write, "todos");
    assert_eq!(
        schema_strings(
            todo_items
                .get("status")
                .and_then(data_object)
                .and_then(|schema| schema.get("enum"))
                .expect("todo status enum")
        ),
        vec!["pending", "in_progress", "completed"]
    );
}

#[test]
fn registry_send_message_requires_a_recipient_alternative() {
    let registry = ToolRegistry::new();
    let descriptor = registry
        .descriptor("send_message")
        .expect("send_message descriptor");

    assert_required_parameters(&descriptor, &["message"]);
    let alternatives = descriptor
        .parameters_schema
        .get("anyOf")
        .and_then(data_array)
        .expect("send_message recipient alternatives");
    let recipient_requirements = alternatives
        .iter()
        .map(|alternative| {
            let schema = data_object(alternative).expect("recipient alternative schema");
            schema_strings(
                schema
                    .get("required")
                    .expect("recipient alternative required fields"),
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(
        recipient_requirements,
        vec![vec!["to_agent_id"], vec!["to_agent_name"]]
    );
}

#[test]
fn registry_marks_trim_rejected_strings_as_non_blank() {
    let registry = ToolRegistry::new();

    for (tool, property) in [
        ("memory_add", "content"),
        ("web_fetch", "url"),
        ("exa_search", "query"),
        ("calculate", "expression"),
        ("read_file", "file_path"),
        ("list_dir", "path"),
        ("glob", "pattern"),
        ("grep", "pattern"),
        ("write_file", "file_path"),
        ("edit_file", "file_path"),
        ("multi_edit", "file_path"),
        ("bash", "command"),
        ("bg_start", "command"),
        ("bg_output", "id"),
        ("bg_stop", "id"),
    ] {
        assert_non_blank_string(&parameter_schema(&registry, tool, property));
    }

    let todo_write = registry
        .descriptor("todo_write")
        .expect("todo_write descriptor");
    let todo_items = array_item_properties(&todo_write, "todos");
    for property in ["content", "activeForm"] {
        assert_non_blank_string(
            todo_items
                .get(property)
                .and_then(data_object)
                .expect("todo string schema"),
        );
    }
}

#[test]
fn registry_send_message_matches_empty_only_parser_semantics() {
    let registry = ToolRegistry::new();

    for property in ["message", "to_agent_id", "to_agent_name"] {
        assert_non_empty_only_string(&parameter_schema(&registry, "send_message", property));
    }
}

#[test]
fn registry_string_schemas_preserve_handler_empty_string_semantics() {
    let registry = ToolRegistry::new();

    for (tool, property) in [
        ("glob", "path"),
        ("grep", "path"),
        ("grep", "include"),
        ("write_file", "content"),
        ("edit_file", "old_string"),
        ("edit_file", "new_string"),
        ("bash", "cwd"),
        ("bg_start", "cwd"),
    ] {
        assert_unconstrained_string(&parameter_schema(&registry, tool, property));
    }

    for (tool, property) in [("memory_search", "query"), ("broadcast_message", "message")] {
        assert_non_empty_string(&parameter_schema(&registry, tool, property));
    }

    let multi_edit = registry
        .descriptor("multi_edit")
        .expect("multi_edit descriptor");
    let edit_items = array_item_properties(&multi_edit, "edits");
    for property in ["old_string", "new_string"] {
        assert_unconstrained_string(
            edit_items
                .get(property)
                .and_then(data_object)
                .expect("edit string schema"),
        );
    }
}

fn assert_required_parameters(descriptor: &ToolDescriptor, expected: &[&str]) {
    let Some(DataValue::Object(properties)) = descriptor.parameters_schema.get("properties") else {
        panic!("{} must define object properties", descriptor.name);
    };
    let Some(DataValue::Array(required)) = descriptor.parameters_schema.get("required") else {
        panic!("{} must define required parameters", descriptor.name);
    };

    assert_eq!(
        required,
        &expected
            .iter()
            .map(|name| DataValue::String((*name).into()))
            .collect::<Vec<_>>()
    );
    for name in expected {
        assert!(
            properties.contains_key(*name),
            "{} is missing required property {name}",
            descriptor.name
        );
    }
}

fn assert_object_parameters(descriptor: &ToolDescriptor, required: &[&str], optional: &[&str]) {
    assert_eq!(
        descriptor.parameters_schema.get("type"),
        Some(&DataValue::String("object".into()))
    );
    assert_required_parameters(descriptor, required);
    let properties = descriptor
        .parameters_schema
        .get("properties")
        .and_then(data_object)
        .expect("object properties");
    assert_eq!(properties.len(), required.len() + optional.len());
    for name in required.iter().chain(optional) {
        assert!(properties.contains_key(*name), "missing {name}");
    }
}

fn assert_property_schema(
    registry: &ToolRegistry,
    tool: &str,
    property: &str,
    expected_type: &str,
    minimum: Option<f64>,
    maximum: Option<f64>,
) {
    let descriptor = registry.descriptor(tool).expect("registered descriptor");
    let schema = descriptor
        .parameters_schema
        .get("properties")
        .and_then(data_object)
        .and_then(|properties| properties.get(property))
        .and_then(data_object)
        .expect("property schema");

    assert_eq!(
        schema.get("type"),
        Some(&DataValue::String(expected_type.into()))
    );
    assert_eq!(
        schema.get("minimum"),
        minimum.map(DataValue::Number).as_ref()
    );
    assert_eq!(
        schema.get("maximum"),
        maximum.map(DataValue::Number).as_ref()
    );
}

fn parameter_schema(
    registry: &ToolRegistry,
    tool: &str,
    property: &str,
) -> BTreeMap<String, DataValue> {
    registry
        .descriptor(tool)
        .expect("registered descriptor")
        .parameters_schema
        .get("properties")
        .and_then(data_object)
        .and_then(|properties| properties.get(property))
        .and_then(data_object)
        .cloned()
        .expect("property schema")
}

fn assert_non_blank_string(schema: &BTreeMap<String, DataValue>) {
    assert_non_empty_string(schema);
    assert_eq!(
        schema.get("pattern"),
        Some(&DataValue::String(r".*\S.*".into()))
    );
}

fn assert_non_empty_string(schema: &BTreeMap<String, DataValue>) {
    assert_eq!(
        schema.get("type"),
        Some(&DataValue::String("string".into()))
    );
    assert_eq!(schema.get("minLength"), Some(&DataValue::Number(1.0)));
}

fn assert_non_empty_only_string(schema: &BTreeMap<String, DataValue>) {
    assert_non_empty_string(schema);
    assert_eq!(schema.get("pattern"), None);
}

fn assert_unconstrained_string(schema: &BTreeMap<String, DataValue>) {
    assert_eq!(
        schema.get("type"),
        Some(&DataValue::String("string".into()))
    );
    assert_eq!(schema.get("minLength"), None);
    assert_eq!(schema.get("pattern"), None);
}

fn assert_string_enum(registry: &ToolRegistry, tool: &str, property: &str, expected: &[&str]) {
    let descriptor = registry.descriptor(tool).expect("registered descriptor");
    let schema = descriptor
        .parameters_schema
        .get("properties")
        .and_then(data_object)
        .and_then(|properties| properties.get(property))
        .and_then(data_object)
        .expect("enum property schema");

    assert_eq!(
        schema.get("type"),
        Some(&DataValue::String("string".into()))
    );
    assert_eq!(
        schema_strings(schema.get("enum").expect("enum values")),
        expected
    );
}

fn assert_array_items(registry: &ToolRegistry, tool: &str, property: &str, required: &[&str]) {
    let descriptor = registry.descriptor(tool).expect("registered descriptor");
    let properties = array_item_properties(&descriptor, property);
    let item_schema = descriptor
        .parameters_schema
        .get("properties")
        .and_then(data_object)
        .and_then(|properties| properties.get(property))
        .and_then(data_object)
        .and_then(|array| array.get("items"))
        .and_then(data_object)
        .expect("array item schema");

    assert_eq!(properties.len(), required.len());
    assert_eq!(
        schema_strings(item_schema.get("required").expect("item required fields")),
        required
    );
}

fn array_item_properties<'a>(
    descriptor: &'a ToolDescriptor,
    property: &str,
) -> &'a BTreeMap<String, DataValue> {
    descriptor
        .parameters_schema
        .get("properties")
        .and_then(data_object)
        .and_then(|properties| properties.get(property))
        .and_then(data_object)
        .and_then(|array| array.get("items"))
        .and_then(data_object)
        .and_then(|item| item.get("properties"))
        .and_then(data_object)
        .expect("array item properties")
}

fn schema_strings(value: &DataValue) -> Vec<&str> {
    data_array(value)
        .expect("schema string array")
        .iter()
        .map(|value| data_string(value).expect("schema string"))
        .collect()
}

fn data_object(value: &DataValue) -> Option<&BTreeMap<String, DataValue>> {
    match value {
        DataValue::Object(value) => Some(value),
        _ => None,
    }
}

fn data_array(value: &DataValue) -> Option<&[DataValue]> {
    match value {
        DataValue::Array(value) => Some(value),
        _ => None,
    }
}

fn data_string(value: &DataValue) -> Option<&str> {
    match value {
        DataValue::String(value) => Some(value),
        _ => None,
    }
}

#[test]
fn tool_registry_accepts_web_fetch_descriptor() {
    let registry = ToolRegistry::new();
    let tools = vec![ToolDescriptor {
        name: "web_fetch".into(),
        description: "Fetch a URL".into(),
        parameters_schema: BTreeMap::from([
            ("type".into(), DataValue::String("object".into())),
            (
                "properties".into(),
                DataValue::Object(BTreeMap::from([(
                    "url".into(),
                    DataValue::Object(BTreeMap::new()),
                )])),
            ),
        ]),
        examples: None,
    }];

    assert!(registry.validate_tools(Some(&tools)).is_ok());
    assert!(registry.lookup("web_fetch").is_some());
}

#[test]
fn strip_html_text_removes_tags_and_script_blocks() {
    let stripped = strip_html_text(
        r#"<html><body><script>alert('x')</script><h1>Hello</h1><p>world</p></body></html>"#,
    );

    assert_eq!(stripped, "Hello world");
}

#[test]
fn tool_registry_accepts_exa_search_descriptor() {
    let registry = ToolRegistry::new();
    let tools = vec![ToolDescriptor {
        name: "exa_search".into(),
        description: "Search Exa".into(),
        parameters_schema: BTreeMap::from([
            ("type".into(), DataValue::String("object".into())),
            (
                "properties".into(),
                DataValue::Object(BTreeMap::from([(
                    "query".into(),
                    DataValue::Object(BTreeMap::new()),
                )])),
            ),
        ]),
        examples: None,
    }];

    assert!(registry.validate_tools(Some(&tools)).is_ok());
    assert!(registry.lookup("exa_search").is_some());
}

#[test]
fn tool_registry_accepts_calculate_and_get_current_time_descriptors() {
    let registry = ToolRegistry::new();
    let tools = vec![
        ToolDescriptor {
            name: "calculate".into(),
            description: "Evaluate math".into(),
            parameters_schema: BTreeMap::new(),
            examples: None,
        },
        ToolDescriptor {
            name: "get_current_time".into(),
            description: "Current time".into(),
            parameters_schema: BTreeMap::new(),
            examples: None,
        },
    ];

    assert!(registry.validate_tools(Some(&tools)).is_ok());
    assert!(registry.lookup("calculate").is_some());
    assert!(registry.lookup("get_current_time").is_some());
}

#[test]
fn tool_registry_accepts_read_file_and_list_dir_descriptors() {
    let registry = ToolRegistry::new();
    let tools = vec![
        ToolDescriptor {
            name: "read_file".into(),
            description: "Read a file".into(),
            parameters_schema: BTreeMap::new(),
            examples: None,
        },
        ToolDescriptor {
            name: "list_dir".into(),
            description: "List a directory".into(),
            parameters_schema: BTreeMap::new(),
            examples: None,
        },
    ];

    assert!(registry.validate_tools(Some(&tools)).is_ok());
    assert!(registry.lookup("read_file").is_some());
    assert!(registry.lookup("list_dir").is_some());
}

#[test]
fn tool_registry_accepts_glob_and_grep_descriptors() {
    let registry = ToolRegistry::new();
    let tools = vec![
        ToolDescriptor {
            name: "glob".into(),
            description: "Find files".into(),
            parameters_schema: BTreeMap::new(),
            examples: None,
        },
        ToolDescriptor {
            name: "grep".into(),
            description: "Search files".into(),
            parameters_schema: BTreeMap::new(),
            examples: None,
        },
    ];

    assert!(registry.validate_tools(Some(&tools)).is_ok());
    assert!(registry.lookup("glob").is_some());
    assert!(registry.lookup("grep").is_some());
}

#[test]
fn tool_registry_accepts_write_edit_and_multi_edit_descriptors() {
    let registry = ToolRegistry::new();
    let tools = vec![
        ToolDescriptor {
            name: "write_file".into(),
            description: "Write a file".into(),
            parameters_schema: BTreeMap::new(),
            examples: None,
        },
        ToolDescriptor {
            name: "edit_file".into(),
            description: "Edit a file".into(),
            parameters_schema: BTreeMap::new(),
            examples: None,
        },
        ToolDescriptor {
            name: "multi_edit".into(),
            description: "Edit a file atomically".into(),
            parameters_schema: BTreeMap::new(),
            examples: None,
        },
    ];

    assert!(registry.validate_tools(Some(&tools)).is_ok());
    assert!(registry.lookup("write_file").is_some());
    assert!(registry.lookup("edit_file").is_some());
    assert!(registry.lookup("multi_edit").is_some());
}

#[test]
fn tool_registry_accepts_bash_and_background_descriptors() {
    let registry = ToolRegistry::new();
    let tools = vec![
        ToolDescriptor {
            name: "bash".into(),
            description: "Run a shell command".into(),
            parameters_schema: BTreeMap::new(),
            examples: None,
        },
        ToolDescriptor {
            name: "bg_start".into(),
            description: "Start background process".into(),
            parameters_schema: BTreeMap::new(),
            examples: None,
        },
        ToolDescriptor {
            name: "bg_output".into(),
            description: "Read background output".into(),
            parameters_schema: BTreeMap::new(),
            examples: None,
        },
        ToolDescriptor {
            name: "bg_stop".into(),
            description: "Stop background process".into(),
            parameters_schema: BTreeMap::new(),
            examples: None,
        },
        ToolDescriptor {
            name: "bg_list".into(),
            description: "List background processes".into(),
            parameters_schema: BTreeMap::new(),
            examples: None,
        },
    ];

    assert!(registry.validate_tools(Some(&tools)).is_ok());
    assert!(registry.lookup("bash").is_some());
    assert!(registry.lookup("bg_start").is_some());
    assert!(registry.lookup("bg_output").is_some());
    assert!(registry.lookup("bg_stop").is_some());
    assert!(registry.lookup("bg_list").is_some());
}

#[test]
fn tool_registry_accepts_swarm_messaging_descriptors() {
    let registry = ToolRegistry::new();
    let tools = vec![
        ToolDescriptor {
            name: "send_message".into(),
            description: "Send a swarm message".into(),
            parameters_schema: BTreeMap::new(),
            examples: None,
        },
        ToolDescriptor {
            name: "broadcast_message".into(),
            description: "Broadcast a swarm message".into(),
            parameters_schema: BTreeMap::new(),
            examples: None,
        },
    ];

    assert!(registry.validate_tools(Some(&tools)).is_ok());
    assert!(registry.lookup("send_message").is_some());
    assert!(registry.lookup("broadcast_message").is_some());
}

#[test]
fn tool_registry_accepts_todo_descriptors() {
    let registry = ToolRegistry::new();
    let tools = vec![
        ToolDescriptor {
            name: "todo_write".into(),
            description: "Write todos".into(),
            parameters_schema: BTreeMap::new(),
            examples: None,
        },
        ToolDescriptor {
            name: "todo_read".into(),
            description: "Read todos".into(),
            parameters_schema: BTreeMap::new(),
            examples: None,
        },
    ];

    assert!(registry.validate_tools(Some(&tools)).is_ok());
    assert!(registry.lookup("todo_write").is_some());
    assert!(registry.lookup("todo_read").is_some());
}

#[test]
fn parse_exa_results_uses_highlights_or_text() {
    let parsed = parse_exa_results(
        &json!({
            "results": [
                {
                    "title": "Operator One",
                    "url": "https://example.com/one",
                    "highlights": ["First highlight", "Second highlight"]
                },
                {
                    "title": "Operator Two",
                    "url": "https://example.com/two",
                    "text": "This is a long operator description"
                }
            ]
        }),
        true,
        10,
    )
    .expect("parsed results");

    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].excerpt, "First highlight Second highlight");
    assert_eq!(parsed[1].excerpt, "This is a ...");
}

#[test]
fn calculate_evaluates_math_expressions() {
    let result = evaluate_expression("2 + 2 * 3").expect("math result");

    assert_eq!(result, "8");
}

#[test]
fn current_time_returns_rfc3339() {
    let timestamp = current_time_iso_utc();

    assert!(DateTime::parse_from_rfc3339(&timestamp).is_ok());
    assert!(timestamp.ends_with('Z'));
}

#[test]
fn read_workspace_file_returns_numbered_slice() {
    let workspace = create_temp_workspace("read-file");
    let file_path = workspace.join("notes.txt");
    fs::write(&file_path, "alpha\nbeta\ngamma\n").expect("write file");

    let result =
        read_workspace_file_from_root(&workspace, "notes.txt", 1, 2).expect("read workspace file");

    assert_eq!(result, "     2| beta\n     3| gamma");

    fs::remove_dir_all(workspace).expect("remove workspace");
}

#[test]
fn list_workspace_dir_marks_files_and_directories() {
    let workspace = create_temp_workspace("list-dir");
    let nested = workspace.join("nested");
    fs::create_dir_all(&nested).expect("create nested dir");
    fs::write(workspace.join("notes.txt"), "hello").expect("write file");

    let result = list_workspace_dir_from_root(&workspace, ".").expect("list workspace dir");

    assert_eq!(result, "[dir]  nested\n[file] notes.txt");

    fs::remove_dir_all(workspace).expect("remove workspace");
}

#[test]
fn compile_glob_matcher_supports_double_star_prefix() {
    let matcher = compile_glob_matcher("**/*.ts").expect("glob matcher");

    assert!(matcher.is_match("nested/file.ts"));
    assert!(matcher.is_match("file.ts"));
}

#[test]
fn glob_workspace_paths_returns_workspace_relative_matches() {
    let workspace = create_temp_workspace("glob");
    fs::create_dir_all(workspace.join("src/nested")).expect("create nested dirs");
    fs::write(workspace.join("src/main.ts"), "export const a = 1;\n").expect("write main");
    fs::write(
        workspace.join("src/nested/util.ts"),
        "export const b = 2;\n",
    )
    .expect("write util");
    fs::write(workspace.join("README.md"), "hello\n").expect("write readme");

    let result =
        glob_workspace_paths_from_root(&workspace, "**/*.ts", "src").expect("glob workspace paths");

    assert_eq!(result, "src/main.ts\nsrc/nested/util.ts");

    fs::remove_dir_all(workspace).expect("remove workspace");
}

#[test]
fn grep_workspace_files_respects_include_glob() {
    let workspace = create_temp_workspace("grep");
    fs::create_dir_all(workspace.join("src")).expect("create src dir");
    fs::write(
        workspace.join("src/main.ts"),
        "const value = 1;\nconst target = value;\n",
    )
    .expect("write ts file");
    fs::write(workspace.join("src/main.md"), "target\n").expect("write md file");

    let result = grep_workspace_files_from_root(&workspace, "target", ".", Some("*.ts"))
        .expect("grep workspace files");

    assert_eq!(result, "src/main.ts:2:const target = value;");

    fs::remove_dir_all(workspace).expect("remove workspace");
}

#[test]
fn write_workspace_file_creates_parent_directories() {
    let workspace = create_temp_workspace("write-file");

    let result = write_workspace_file_from_root(&workspace, "nested/notes.txt", "hello world")
        .expect("write workspace file");

    assert_eq!(result, "Wrote 11 chars to nested/notes.txt");
    assert_eq!(
        fs::read_to_string(workspace.join("nested/notes.txt")).expect("read file"),
        "hello world"
    );

    fs::remove_dir_all(workspace).expect("remove workspace");
}

#[test]
fn edit_workspace_file_applies_over_escaped_match() {
    let workspace = create_temp_workspace("edit-file");
    let file_path = workspace.join("notes.txt");
    fs::write(&file_path, "alpha\nbeta\n").expect("write file");

    let result = edit_workspace_file_from_root(&workspace, "notes.txt", "alpha\\nbeta", "updated")
        .expect("edit workspace file");

    assert_eq!(result, "Edited notes.txt");
    assert_eq!(
        fs::read_to_string(&file_path).expect("read file"),
        "updated\n"
    );

    fs::remove_dir_all(workspace).expect("remove workspace");
}

#[test]
fn multi_edit_workspace_file_is_atomic_on_missing_match() {
    let workspace = create_temp_workspace("multi-edit");
    let file_path = workspace.join("notes.txt");
    fs::write(&file_path, "alpha\nbeta\n").expect("write file");

    let error = multi_edit_workspace_file_from_root(
        &workspace,
        "notes.txt",
        &[
            FileEditOperation {
                old_string: "alpha".into(),
                new_string: "first".into(),
            },
            FileEditOperation {
                old_string: "missing".into(),
                new_string: "second".into(),
            },
        ],
    )
    .expect_err("multi edit should fail");

    assert!(error.contains("Edit 2/2"));
    assert_eq!(
        fs::read_to_string(&file_path).expect("read file"),
        "alpha\nbeta\n"
    );

    fs::remove_dir_all(workspace).expect("remove workspace");
}

#[test]
fn execute_bash_command_runs_shell_command() {
    let workspace = create_temp_workspace("bash");
    let result = execute_bash_command_from_root(&workspace, "echo hello", 5_000, ".")
        .expect("bash command result");

    assert_eq!(result.status, "success");
    assert!(result.output.to_ascii_lowercase().contains("hello"));

    fs::remove_dir_all(workspace).expect("remove workspace");
}

#[test]
fn background_process_manager_tracks_process_lifecycle() {
    let workspace = create_temp_workspace("bg-process");
    let manager = new_shared_process_manager();

    let started = start_background_process_from_root(&manager, &workspace, "echo hello", ".")
        .expect("start background process");
    assert!(started.contains("bg-1"));

    let listed = list_background_processes(&manager).expect("list background processes");
    assert!(listed.contains("bg-1"));

    let deadline = Instant::now() + Duration::from_secs(5);
    let output = loop {
        let output =
            read_background_process_output(&manager, "bg-1", true).expect("read background output");
        if output.to_ascii_lowercase().contains("hello") || Instant::now() >= deadline {
            break output;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(output.to_ascii_lowercase().contains("hello"));

    let stopped = stop_background_process(&manager, "bg-1").expect("stop process");
    assert_eq!(stopped, "Stopped and removed bg-1.");

    fs::remove_dir_all(workspace).expect("remove workspace");
}

#[test]
fn background_process_manager_enforces_running_process_limit() {
    let workspace = create_temp_workspace("bg-process-limit");
    let manager = new_shared_process_manager();
    set_background_process_limit(&manager, 1).expect("set background process limit");

    let started = start_background_process_from_root(&manager, &workspace, "sleep 1", ".")
        .expect("start background process");
    assert!(started.contains("bg-1"));

    let error = start_background_process_from_root(&manager, &workspace, "sleep 1", ".")
        .expect_err("second process should be rejected");
    assert!(error.contains("bg_start limit reached"), "{error}");

    let stopped = stop_background_process(&manager, "bg-1").expect("stop process");
    assert_eq!(stopped, "Stopped and removed bg-1.");

    fs::remove_dir_all(workspace).expect("remove workspace");
}

#[test]
fn todo_write_and_read_persist_structured_todos() {
    let workspace = create_temp_workspace("todo-list");
    let todos = vec![
        TodoItem {
            content: "Inspect the daemon registry".into(),
            status: "completed".into(),
            active_form: "Inspecting the daemon registry".into(),
        },
        TodoItem {
            content: "Port todo tools".into(),
            status: "in_progress".into(),
            active_form: "Porting todo tools".into(),
        },
        TodoItem {
            content: "Run validation".into(),
            status: "pending".into(),
            active_form: "Running validation".into(),
        },
    ];

    let write_result = write_todo_list_from_root(&workspace, &todos).expect("write todos");
    assert_eq!(
        write_result,
        "Todos updated (1 completed, 1 in progress, 1 pending). Proceed with current tasks."
    );

    let todo_file = todo_file_path_from_root(&workspace, "todo_read").expect("todo file path");
    let persisted = fs::read_to_string(&todo_file).expect("read persisted todos");
    assert!(persisted.contains("activeForm"));

    let read_result = read_todo_list_from_root(&workspace).expect("read todos");
    assert_eq!(
        read_result,
        "[x] 1. [completed] Inspect the daemon registry\n[>] 2. [in_progress] Port todo tools\n[ ] 3. [pending] Run validation"
    );

    fs::remove_dir_all(workspace).expect("remove workspace");
}

#[test]
fn todo_write_warns_when_multiple_items_are_in_progress() {
    let workspace = create_temp_workspace("todo-warning");
    let todos = vec![
        TodoItem {
            content: "One".into(),
            status: "in_progress".into(),
            active_form: "Doing one".into(),
        },
        TodoItem {
            content: "Two".into(),
            status: "in_progress".into(),
            active_form: "Doing two".into(),
        },
    ];

    let write_result = write_todo_list_from_root(&workspace, &todos).expect("write todos");
    assert!(
        write_result.contains("Warning: 2 todos are in_progress -- ideally only one at a time.")
    );

    fs::remove_dir_all(workspace).expect("remove workspace");
}

#[test]
fn configured_workspace_root_overrides_env_var() {
    let configured = PathBuf::from("C:\\configured\\root");
    let resolved = workspace_root_path("read_file", Some(configured.as_path()))
        .expect("configured root resolves");
    assert_eq!(resolved, configured);
}

#[test]
fn missing_config_falls_back_to_env_or_cwd() {
    // With no override, behavior is unchanged from today.
    let resolved = workspace_root_path("read_file", None);
    assert!(resolved.is_ok());
}

fn create_temp_workspace(prefix: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time went backwards")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("anima-daemon-{prefix}-{unique}"));
    fs::create_dir_all(&path).expect("create temp workspace");
    path
}
