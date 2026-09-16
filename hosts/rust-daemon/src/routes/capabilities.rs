//! Discovery describes registered handlers, never grants or connection health.
use serde::Serialize;
use utoipa::ToSchema;

use crate::state::DaemonState;

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct CapabilityInventory {
    schema_version: u32,
    tools: Vec<RegisteredTool>,
    persistence: PersistenceInventory,
    extensions: Vec<PlannedExtension>,
    limitations: Vec<&'static str>,
}

#[derive(Serialize, ToSchema)]
struct RegisteredTool {
    name: String,
    description: String,
    category: &'static str,
    requirements: Vec<&'static str>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct PersistenceInventory {
    control_plane: String,
    memory: &'static str,
    execution_journal: bool,
}

#[derive(Serialize, ToSchema)]
struct PlannedExtension {
    id: &'static str,
    label: &'static str,
    status: &'static str,
    description: &'static str,
}

pub(super) fn inventory(state: &DaemonState) -> CapabilityInventory {
    let tools = state
        .tool_registry
        .tool_names()
        .into_iter()
        .filter_map(|name| {
            let descriptor = state.tool_registry.descriptor(&name)?;
            let (category, requirements) = metadata(&name);
            Some(RegisteredTool {
                name,
                description: descriptor.description,
                category,
                requirements,
            })
        })
        .collect();
    CapabilityInventory {
        schema_version: 1,
        tools,
        persistence: PersistenceInventory {
            control_plane: state.control_plane_durability(),
            memory: state.memory_store.as_ref().map_or("ephemeral", |config| config.storage_label()),
            execution_journal: state.database_configured(),
        },
        extensions: [
            ("camera", "Camera observation", "Observe an authorized camera through a bounded perception module."),
            ("voice", "Voice", "Add microphone input and speech with separate access controls."),
            ("smart-home", "Smart home", "Connect devices, inspect their state, and verify authorized actions."),
            ("browser-control", "Browser control", "Interact with web applications through an isolated browser session."),
            ("document-engine", "Document processing", "Extract and create PDF, Word, and spreadsheet artifacts with format-aware validation."),
            ("coding-agents", "External coding agents", "Connect an external coding worker through a bounded execution adapter."),
        ].into_iter().map(|(id, label, description)| PlannedExtension { id, label, status: "planned", description }).collect(),
        limitations: vec![
            "Registered tools still require agent access and their connection or workspace prerequisites.",
            "Storage labels describe configuration, not a successful storage health check.",
            "Saved tasks and memories do not imply automatic resumption of interrupted runs. Uncertain scheduled actions require review.",
            "Background shell processes are managed only while this daemon is running. Shell access is not an operating-system sandbox.",
            "Native file tools handle text; format-aware PDF, Word, spreadsheet, and OCR processing require a document engine.",
            "Extension installation and the portable capability execution bridge are not available yet.",
        ],
    }
}

fn metadata(name: &str) -> (&'static str, Vec<&'static str>) {
    match name {
        "read_file" | "write_file" | "edit_file" | "multi_edit" | "list_dir" | "glob" | "grep" =>
            ("workspace", vec!["Workspace access and an explicit tool grant."]),
        "todo_read" | "todo_write" =>
            ("workspace", vec!["An agent-owned task list in the workspace. Creating a task does not enable a schedule."]),
        "bash" | "bg_start" | "bg_output" | "bg_stop" | "bg_list" =>
            ("terminal", vec!["Shell access. Programs run with the daemon user's operating-system permissions."]),
        "memory_search" | "memory_add" | "recent_memories" =>
            ("memory", vec!["Agent memory access. Persistence depends on the configured memory storage."]),
        "spawn_helper" =>
            ("team", vec!["A direct companion run. Reuses idle helpers, up to four busy helpers, with current companion tool permissions except shell/background-process tools and no recursive communication. Execution deadlines do not roll back completed effects."]),
        "list_workspace_agents" | "delegate_to_agent" | "send_message" | "broadcast_message" =>
            ("team", vec!["Existing workspace agents and permitted communication. Delegation is bounded by the manager's current authority."]),
        "web_fetch" =>
            ("research", vec!["Network access to a public HTTP or HTTPS URL."]),
        "exa_search" =>
            ("research", vec!["A daemon-configured Exa API key and authorized provider usage; provider charges may apply."]),
        name if name.starts_with("calendar_") =>
            ("productivity", vec!["A connected Google Calendar. Changes remain drafts until owner approval."]),
        name if name.starts_with("mail_") =>
            ("productivity", vec!["A connected mailbox. Sending follows the connector's owner approval flow."]),
        _ => ("utility", vec!["An explicit agent tool grant."]),
    }
}
