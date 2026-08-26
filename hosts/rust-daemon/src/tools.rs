mod filesystem;
mod memory;
mod process;
#[cfg(test)]
mod tests;
mod todo;
mod utility;
mod web;
mod workspace;

use std::collections::{BTreeMap, HashMap};

use anima_core::{
    tool_not_configured_error, AgentState, Content, DataValue, Message, TaskResult, ToolCall,
    ToolDescriptor,
};
use futures::future::BoxFuture;

use crate::memory_embeddings::SharedMemoryEmbeddings;
use crate::memory_store::MemoryStoreConfig;
use crate::state::SharedMemoryStore;

pub(crate) use process::{
    background_process_count, new_shared_process_manager_with_limit, SharedProcessManager,
    DEFAULT_MAX_BACKGROUND_PROCESSES,
};
pub(crate) use workspace::{
    canonical_workspace_root, normalized_relative_path, resolve_workspace_write_path,
    workspace_root_path,
};

type ToolHandler = fn(
    ToolExecutionContext,
    AgentState,
    Message,
    ToolCall,
) -> BoxFuture<'static, TaskResult<Content>>;

#[derive(Clone)]
pub(crate) struct ToolRegistry {
    registrations: HashMap<String, ToolRegistration>,
}

#[derive(Clone)]
struct ToolRegistration {
    descriptor: ToolDescriptor,
    handler: ToolHandler,
}

#[derive(Clone)]
pub(crate) struct ToolExecutionContext {
    pub(super) memory: SharedMemoryStore,
    pub(super) memory_embeddings: SharedMemoryEmbeddings,
    pub(super) memory_store: Option<MemoryStoreConfig>,
    tool_registry: ToolRegistry,
    pub(super) process_manager: SharedProcessManager,
}

impl ToolExecutionContext {
    pub(crate) fn new(
        memory: SharedMemoryStore,
        memory_embeddings: SharedMemoryEmbeddings,
        memory_store: Option<MemoryStoreConfig>,
        tool_registry: ToolRegistry,
        process_manager: SharedProcessManager,
    ) -> Self {
        Self {
            memory,
            memory_embeddings,
            memory_store,
            tool_registry,
            process_manager,
        }
    }

    pub(crate) async fn execute_tool(
        self,
        agent: AgentState,
        user_message: Message,
        tool_call: ToolCall,
    ) -> TaskResult<Content> {
        if !agent.config.allows_tool(&tool_call.name) {
            return TaskResult::error(tool_not_configured_error(&tool_call.name), 0);
        }
        let handler = self.tool_registry.lookup(&tool_call.name);
        match handler {
            Some(handler) => handler(self, agent, user_message, tool_call).await,
            None => TaskResult::error(format!("Unknown tool: {}", tool_call.name), 0),
        }
    }
}

impl ToolRegistry {
    pub(crate) fn new() -> Self {
        let mut registry = Self {
            registrations: HashMap::new(),
        };
        registry.register(
            tool_descriptor(
                "memory_search",
                "Search durable agent memories by semantic similarity",
                object_parameters(vec![
                    required_parameter(
                        "query",
                        non_empty_string_parameter("Search query used to find relevant memories"),
                    ),
                    optional_parameter(
                        "limit",
                        integer_parameter("Maximum number of memories to return", 1),
                    ),
                ]),
            ),
            memory::execute_memory_search,
        );
        registry.register(
            tool_descriptor(
                "memory_add",
                "Store a durable memory for future agent runs",
                object_parameters(vec![
                    required_parameter(
                        "content",
                        non_blank_string_parameter("Memory content to persist"),
                    ),
                    optional_parameter(
                        "type",
                        string_enum_parameter(
                            "Memory classification",
                            &["fact", "observation", "task_result", "reflection"],
                        ),
                    ),
                    optional_parameter(
                        "importance",
                        number_parameter("Memory importance from 0 to 1", 0.0, 1.0),
                    ),
                ]),
            ),
            memory::execute_memory_add,
        );
        registry.register(
            tool_descriptor(
                "recent_memories",
                "Read the most recently stored durable memories",
                object_parameters(vec![optional_parameter(
                    "limit",
                    integer_parameter("Maximum number of memories to return", 1),
                )]),
            ),
            memory::execute_recent_memories,
        );
        registry.register(
            tool_descriptor(
                "web_fetch",
                "Fetch readable text from a public HTTP or HTTPS URL",
                object_parameters(vec![
                    required_parameter("url", non_blank_string_parameter("Public URL to fetch")),
                    optional_parameter(
                        "max_length",
                        integer_parameter("Maximum number of characters to return", 1),
                    ),
                ]),
            ),
            web::execute_web_fetch,
        );
        registry.register(
            tool_descriptor(
                "exa_search",
                "Search the web with Exa and return ranked results",
                object_parameters(vec![
                    required_parameter("query", non_blank_string_parameter("Web search query")),
                    optional_parameter(
                        "num_results",
                        integer_parameter("Maximum number of search results", 1),
                    ),
                    optional_parameter(
                        "include_text",
                        boolean_parameter("Include result text excerpts"),
                    ),
                    optional_parameter(
                        "max_characters",
                        integer_parameter("Maximum characters per result excerpt", 1),
                    ),
                ]),
            ),
            web::execute_exa_search,
        );
        registry.register(
            tool_descriptor(
                "get_current_time",
                "Return the current UTC time in RFC 3339 format",
                object_parameters(Vec::new()),
            ),
            utility::execute_get_current_time,
        );
        registry.register(
            tool_descriptor(
                "calculate",
                "Evaluate a mathematical expression",
                object_parameters(vec![required_parameter(
                    "expression",
                    non_blank_string_parameter("Mathematical expression to evaluate"),
                )]),
            ),
            utility::execute_calculate,
        );
        registry.register(
            tool_descriptor(
                "read_file",
                "Read a workspace file with line numbers",
                object_parameters(vec![
                    required_parameter(
                        "file_path",
                        non_blank_string_parameter("Workspace-relative path of the file to read"),
                    ),
                    optional_parameter("offset", integer_parameter("Zero-based line offset", 0)),
                    optional_parameter(
                        "limit",
                        integer_parameter("Maximum number of lines to return", 0),
                    ),
                ]),
            ),
            filesystem::execute_read_file,
        );
        registry.register(
            tool_descriptor(
                "list_dir",
                "List files and directories within a workspace path",
                object_parameters(vec![required_parameter(
                    "path",
                    non_blank_string_parameter("Workspace-relative directory path"),
                )]),
            ),
            filesystem::execute_list_dir,
        );
        registry.register(
            tool_descriptor(
                "glob",
                "Find workspace files whose paths match a glob pattern",
                object_parameters(vec![
                    required_parameter(
                        "pattern",
                        non_blank_string_parameter("Glob pattern to match"),
                    ),
                    optional_parameter(
                        "path",
                        string_parameter("Workspace-relative directory to search"),
                    ),
                ]),
            ),
            filesystem::execute_glob,
        );
        registry.register(
            tool_descriptor(
                "grep",
                "Search workspace files for a regular expression",
                object_parameters(vec![
                    required_parameter(
                        "pattern",
                        non_blank_string_parameter("Regular expression to search for"),
                    ),
                    optional_parameter(
                        "path",
                        string_parameter("Workspace-relative directory to search"),
                    ),
                    optional_parameter(
                        "include",
                        string_parameter("Glob pattern limiting files to search"),
                    ),
                ]),
            ),
            filesystem::execute_grep,
        );
        registry.register(
            tool_descriptor(
                "write_file",
                "Create or overwrite a workspace file",
                object_parameters(vec![
                    required_parameter(
                        "file_path",
                        non_blank_string_parameter("Workspace-relative path of the file to write"),
                    ),
                    required_parameter("content", string_parameter("Complete file content")),
                ]),
            ),
            filesystem::execute_write_file,
        );
        registry.register(
            tool_descriptor(
                "edit_file",
                "Replace one exact string occurrence in a workspace file",
                object_parameters(vec![
                    required_parameter(
                        "file_path",
                        non_blank_string_parameter("Workspace-relative path of the file to edit"),
                    ),
                    required_parameter(
                        "old_string",
                        string_parameter("Exact existing text to replace"),
                    ),
                    required_parameter("new_string", string_parameter("Replacement text")),
                ]),
            ),
            filesystem::execute_edit_file,
        );
        registry.register(
            tool_descriptor(
                "multi_edit",
                "Apply multiple exact string replacements to a workspace file atomically",
                object_parameters(vec![
                    required_parameter(
                        "file_path",
                        non_blank_string_parameter("Workspace-relative path of the file to edit"),
                    ),
                    required_parameter(
                        "edits",
                        array_parameter(
                            "Ordered replacements to apply atomically",
                            object_parameter(vec![
                                required_parameter(
                                    "old_string",
                                    string_parameter("Exact existing text to replace"),
                                ),
                                required_parameter(
                                    "new_string",
                                    string_parameter("Replacement text"),
                                ),
                            ]),
                            Some(1),
                        ),
                    ),
                ]),
            ),
            filesystem::execute_multi_edit,
        );
        registry.register(
            tool_descriptor(
                "todo_write",
                "Persist the agent's structured workspace todo list",
                object_parameters(vec![required_parameter(
                    "todos",
                    array_parameter(
                        "Complete todo list to persist",
                        object_parameter(vec![
                            required_parameter(
                                "content",
                                non_blank_string_parameter("Todo description"),
                            ),
                            required_parameter(
                                "status",
                                string_enum_parameter(
                                    "Todo status",
                                    &["pending", "in_progress", "completed"],
                                ),
                            ),
                            required_parameter(
                                "activeForm",
                                non_blank_string_parameter(
                                    "Present-tense description of active work",
                                ),
                            ),
                        ]),
                        None,
                    ),
                )]),
            ),
            todo::execute_todo_write,
        );
        registry.register(
            tool_descriptor(
                "todo_read",
                "Read the agent's persisted workspace todo list",
                object_parameters(Vec::new()),
            ),
            todo::execute_todo_read,
        );
        registry.register(
            tool_descriptor(
                "bash",
                "Run a foreground shell command in the workspace",
                object_parameters(vec![
                    required_parameter(
                        "command",
                        non_blank_string_parameter("Shell command to run"),
                    ),
                    optional_parameter(
                        "timeout",
                        integer_parameter("Command timeout in milliseconds", 1),
                    ),
                    optional_parameter(
                        "cwd",
                        string_parameter("Workspace-relative working directory"),
                    ),
                ]),
            ),
            process::execute_bash,
        );
        registry.register(
            tool_descriptor(
                "bg_start",
                "Start a background shell command in the workspace",
                object_parameters(vec![
                    required_parameter(
                        "command",
                        non_blank_string_parameter("Shell command to start"),
                    ),
                    optional_parameter(
                        "cwd",
                        string_parameter("Workspace-relative working directory"),
                    ),
                ]),
            ),
            process::execute_bg_start,
        );
        registry.register(
            tool_descriptor(
                "bg_output",
                "Read captured output from a background process",
                object_parameters(vec![
                    required_parameter("id", non_blank_string_parameter("Background process id")),
                    optional_parameter(
                        "all",
                        boolean_parameter("Return all captured output instead of unread output"),
                    ),
                ]),
            ),
            process::execute_bg_output,
        );
        registry.register(
            tool_descriptor(
                "bg_stop",
                "Stop a running background process",
                object_parameters(vec![required_parameter(
                    "id",
                    non_blank_string_parameter("Background process id"),
                )]),
            ),
            process::execute_bg_stop,
        );
        registry.register(
            tool_descriptor(
                "bg_list",
                "List background processes managed by the daemon",
                object_parameters(Vec::new()),
            ),
            process::execute_bg_list,
        );
        registry.register(
            tool_descriptor(
                "send_message",
                "Send a message to another live swarm agent by coordinator agent id or configured agent name",
                with_any_required_parameter(
                    object_parameters(vec![
                        optional_parameter(
                            "to_agent_id",
                            non_empty_string_parameter(
                                "Coordinator agent id to receive the message",
                            ),
                        ),
                        optional_parameter(
                            "to_agent_name",
                            non_empty_string_parameter(
                                "Configured swarm agent name to receive the message",
                            ),
                        ),
                        required_parameter(
                            "message",
                            non_empty_string_parameter("Message text to deliver"),
                        ),
                    ]),
                    &["to_agent_id", "to_agent_name"],
                ),
            ),
            execute_swarm_only_tool,
        );
        registry.register(
            tool_descriptor(
                "broadcast_message",
                "Broadcast a message to every other live swarm agent",
                object_parameters(vec![required_parameter(
                    "message",
                    non_empty_string_parameter("Message text to broadcast"),
                )]),
            ),
            execute_swarm_only_tool,
        );
        registry
    }

    fn register(&mut self, descriptor: ToolDescriptor, handler: ToolHandler) {
        let name = descriptor.name.clone();
        assert!(
            !self.registrations.contains_key(&name),
            "duplicate tool registration '{name}'"
        );
        self.registrations.insert(
            name,
            ToolRegistration {
                descriptor,
                handler,
            },
        );
    }

    pub(crate) fn lookup(&self, name: &str) -> Option<ToolHandler> {
        self.registrations
            .get(name)
            .map(|registration| registration.handler)
    }

    pub(crate) fn descriptor(&self, name: &str) -> Option<ToolDescriptor> {
        self.registrations
            .get(name)
            .map(|registration| registration.descriptor.clone())
    }

    pub(crate) fn resolve_descriptors<I, S>(&self, names: I) -> Result<Vec<ToolDescriptor>, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        names
            .into_iter()
            .map(|name| {
                let name = name.as_ref();
                self.descriptor(name)
                    .ok_or_else(|| format!("unknown tool '{name}'"))
            })
            .collect()
    }

    pub(crate) fn tool_names(&self) -> Vec<String> {
        let mut names = self.registrations.keys().cloned().collect::<Vec<_>>();
        names.sort();
        names
    }

    pub(crate) fn validate_tools(&self, tools: Option<&[ToolDescriptor]>) -> Result<(), String> {
        let Some(tools) = tools else {
            return Ok(());
        };

        for tool in tools {
            if !self.registrations.contains_key(&tool.name) {
                return Err(format!("unknown tool: {}", tool.name));
            }
        }

        Ok(())
    }
}

struct SchemaParameter {
    name: &'static str,
    schema: DataValue,
    required: bool,
}

fn tool_descriptor(
    name: &str,
    description: &str,
    parameters_schema: BTreeMap<String, DataValue>,
) -> ToolDescriptor {
    ToolDescriptor {
        name: name.into(),
        description: description.into(),
        parameters_schema,
        examples: None,
    }
}

fn required_parameter(name: &'static str, schema: DataValue) -> SchemaParameter {
    SchemaParameter {
        name,
        schema,
        required: true,
    }
}

fn optional_parameter(name: &'static str, schema: DataValue) -> SchemaParameter {
    SchemaParameter {
        name,
        schema,
        required: false,
    }
}

fn object_parameters(parameters: Vec<SchemaParameter>) -> BTreeMap<String, DataValue> {
    let mut properties = BTreeMap::new();
    let mut required = Vec::new();

    for parameter in parameters {
        properties.insert(parameter.name.into(), parameter.schema);
        if parameter.required {
            required.push(DataValue::String(parameter.name.into()));
        }
    }

    BTreeMap::from([
        ("type".into(), DataValue::String("object".into())),
        ("properties".into(), DataValue::Object(properties)),
        ("required".into(), DataValue::Array(required)),
    ])
}

fn object_parameter(parameters: Vec<SchemaParameter>) -> DataValue {
    DataValue::Object(object_parameters(parameters))
}

fn with_any_required_parameter(
    mut parameters_schema: BTreeMap<String, DataValue>,
    alternatives: &[&str],
) -> BTreeMap<String, DataValue> {
    parameters_schema.insert(
        "anyOf".into(),
        DataValue::Array(
            alternatives
                .iter()
                .map(|name| {
                    DataValue::Object(BTreeMap::from([(
                        "required".into(),
                        DataValue::Array(vec![DataValue::String((*name).into())]),
                    )]))
                })
                .collect(),
        ),
    );
    parameters_schema
}

fn string_parameter(description: &str) -> DataValue {
    typed_parameter("string", description)
}

fn non_empty_string_parameter(description: &str) -> DataValue {
    let mut schema = typed_parameter_schema("string", description);
    schema.insert("minLength".into(), DataValue::Number(1.0));
    DataValue::Object(schema)
}

fn non_blank_string_parameter(description: &str) -> DataValue {
    let mut schema = typed_parameter_schema("string", description);
    schema.insert("minLength".into(), DataValue::Number(1.0));
    schema.insert("pattern".into(), DataValue::String(r".*\S.*".into()));
    DataValue::Object(schema)
}

fn integer_parameter(description: &str, minimum: u64) -> DataValue {
    let mut schema = typed_parameter_schema("integer", description);
    schema.insert("minimum".into(), DataValue::Number(minimum as f64));
    DataValue::Object(schema)
}

fn number_parameter(description: &str, minimum: f64, maximum: f64) -> DataValue {
    let mut schema = typed_parameter_schema("number", description);
    schema.insert("minimum".into(), DataValue::Number(minimum));
    schema.insert("maximum".into(), DataValue::Number(maximum));
    DataValue::Object(schema)
}

fn boolean_parameter(description: &str) -> DataValue {
    typed_parameter("boolean", description)
}

fn string_enum_parameter(description: &str, values: &[&str]) -> DataValue {
    let mut schema = typed_parameter_schema("string", description);
    schema.insert(
        "enum".into(),
        DataValue::Array(
            values
                .iter()
                .map(|value| DataValue::String((*value).into()))
                .collect(),
        ),
    );
    DataValue::Object(schema)
}

fn array_parameter(description: &str, items: DataValue, minimum_items: Option<u64>) -> DataValue {
    let mut schema = typed_parameter_schema("array", description);
    schema.insert("items".into(), items);
    if let Some(minimum_items) = minimum_items {
        schema.insert("minItems".into(), DataValue::Number(minimum_items as f64));
    }
    DataValue::Object(schema)
}

fn typed_parameter(kind: &str, description: &str) -> DataValue {
    DataValue::Object(typed_parameter_schema(kind, description))
}

fn typed_parameter_schema(kind: &str, description: &str) -> BTreeMap<String, DataValue> {
    BTreeMap::from([
        ("type".into(), DataValue::String(kind.into())),
        ("description".into(), DataValue::String(description.into())),
    ])
}

fn execute_swarm_only_tool(
    _context: ToolExecutionContext,
    _agent: AgentState,
    _user_message: Message,
    tool_call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    Box::pin(async move {
        TaskResult::error(
            format!("{} is only available inside a swarm", tool_call.name),
            0,
        )
    })
}
