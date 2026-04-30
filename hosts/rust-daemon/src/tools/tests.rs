use super::{
    filesystem::{
        edit::{
            edit_workspace_file_from_root, multi_edit_workspace_file_from_root,
            write_workspace_file_from_root,
        },
        search::{
            compile_glob_matcher, glob_workspace_paths_from_root, grep_workspace_files_from_root,
            list_workspace_dir_from_root, read_workspace_file_from_root,
        },
        FileEditOperation,
    },
    process::{
        background::{
            list_background_processes, read_background_process_output,
            start_background_process_from_root, stop_background_process,
        },
        new_shared_process_manager,
        shell::execute_bash_command_from_root,
    },
    todo::{
        read_todo_list_from_root, todo_file_path_from_root, write_todo_list_from_root, TodoItem,
    },
    utility::{current_time_iso_utc, evaluate_expression},
    web::{parse_exa_results, strip_html_text},
    ToolRegistry,
};
use anima_core::{DataValue, ToolDescriptor};
use chrono::DateTime;
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[test]
fn tool_registry_accepts_web_fetch_descriptor() {
    let registry = ToolRegistry::new();
    let tools = vec![ToolDescriptor {
        name: "web_fetch".into(),
        description: "Fetch a URL".into(),
        parameters: BTreeMap::from([
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
        parameters: BTreeMap::from([
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
            parameters: BTreeMap::new(),
            examples: None,
        },
        ToolDescriptor {
            name: "get_current_time".into(),
            description: "Current time".into(),
            parameters: BTreeMap::new(),
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
            parameters: BTreeMap::new(),
            examples: None,
        },
        ToolDescriptor {
            name: "list_dir".into(),
            description: "List a directory".into(),
            parameters: BTreeMap::new(),
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
            parameters: BTreeMap::new(),
            examples: None,
        },
        ToolDescriptor {
            name: "grep".into(),
            description: "Search files".into(),
            parameters: BTreeMap::new(),
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
            parameters: BTreeMap::new(),
            examples: None,
        },
        ToolDescriptor {
            name: "edit_file".into(),
            description: "Edit a file".into(),
            parameters: BTreeMap::new(),
            examples: None,
        },
        ToolDescriptor {
            name: "multi_edit".into(),
            description: "Edit a file atomically".into(),
            parameters: BTreeMap::new(),
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
            parameters: BTreeMap::new(),
            examples: None,
        },
        ToolDescriptor {
            name: "bg_start".into(),
            description: "Start background process".into(),
            parameters: BTreeMap::new(),
            examples: None,
        },
        ToolDescriptor {
            name: "bg_output".into(),
            description: "Read background output".into(),
            parameters: BTreeMap::new(),
            examples: None,
        },
        ToolDescriptor {
            name: "bg_stop".into(),
            description: "Stop background process".into(),
            parameters: BTreeMap::new(),
            examples: None,
        },
        ToolDescriptor {
            name: "bg_list".into(),
            description: "List background processes".into(),
            parameters: BTreeMap::new(),
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
fn tool_registry_accepts_todo_descriptors() {
    let registry = ToolRegistry::new();
    let tools = vec![
        ToolDescriptor {
            name: "todo_write".into(),
            description: "Write todos".into(),
            parameters: BTreeMap::new(),
            examples: None,
        },
        ToolDescriptor {
            name: "todo_read".into(),
            description: "Read todos".into(),
            parameters: BTreeMap::new(),
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
    fs::write(workspace.join("src/nested/util.ts"), "export const b = 2;\n")
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

    let result =
        edit_workspace_file_from_root(&workspace, "notes.txt", "alpha\\nbeta", "updated")
            .expect("edit workspace file");

    assert_eq!(result, "Edited notes.txt");
    assert_eq!(fs::read_to_string(&file_path).expect("read file"), "updated\n");

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
    assert_eq!(fs::read_to_string(&file_path).expect("read file"), "alpha\nbeta\n");

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

    std::thread::sleep(Duration::from_millis(50));

    let listed = list_background_processes(&manager).expect("list background processes");
    assert!(listed.contains("bg-1"));

    let output =
        read_background_process_output(&manager, "bg-1", true).expect("read background output");
    assert!(output.to_ascii_lowercase().contains("hello"));

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

fn create_temp_workspace(prefix: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time went backwards")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("anima-daemon-{prefix}-{unique}"));
    fs::create_dir_all(&path).expect("create temp workspace");
    path
}
