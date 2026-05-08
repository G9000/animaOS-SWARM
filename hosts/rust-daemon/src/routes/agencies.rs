use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::Path;
use std::sync::Arc;

use anima_core::{AgentConfig, Content, Message, MessageRole, ModelAdapter, ModelGenerateRequest};
use serde::Serialize;
use serde_json::Value;

use super::contracts::{
    AgencyCreateRequest, AgencyCreateResponse, AgencyGenerateRequest, AgencyGenerateResponse,
    AgentDefinitionResponse,
};
use super::ApiError;
use crate::app::SharedDaemonState;
use crate::tools::{
    canonical_workspace_root, normalized_relative_path, resolve_workspace_write_path,
    workspace_root_path, ToolRegistry,
};

const TEAM_MIN: u64 = 2;
const TEAM_MAX: u64 = 10;
const DEFAULT_TEAM_SIZE: u64 = 4;

#[derive(Clone, Debug)]
struct PreparedAgencyRequest {
    name: String,
    description: String,
    team_size: u64,
    provider: String,
    model: String,
    model_pool: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AgencyYamlConfig {
    name: String,
    description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    mission: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    values: Option<Vec<String>>,
    model: String,
    provider: String,
    strategy: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_parallel_delegations: Option<u64>,
    orchestrator: AgencyYamlAgent,
    agents: Vec<AgencyYamlAgent>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AgencyYamlAgent {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    position: Option<String>,
    bio: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    lore: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    knowledge: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    topics: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    adjectives: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    style: Option<String>,
    system: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    collaborates_with: Option<Vec<String>>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentSeedMemories {
    agent_name: String,
    entries: Vec<SeedMemoryEntry>,
}

#[derive(Clone, Debug, Serialize)]
struct SeedMemoryEntry {
    #[serde(rename = "type")]
    kind: String,
    content: String,
    importance: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    tags: Option<Vec<String>>,
}

pub(crate) async fn handle_generate_agency(
    body: Vec<u8>,
    state: &SharedDaemonState,
) -> Result<AgencyGenerateResponse, ApiError> {
    let request: AgencyGenerateRequest = super::parse_json_body(body)?;

    generate_agency_from_request(request, state).await
}

pub(crate) async fn handle_create_agency(
    body: Vec<u8>,
    state: &SharedDaemonState,
) -> Result<AgencyCreateResponse, ApiError> {
    let request: AgencyCreateRequest = super::parse_json_body(body)?;
    let output_dir = request
        .output_dir
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let seed_memories = request.seed_memories;
    let overwrite = request.overwrite;

    let prepared = prepare_generate_request(AgencyGenerateRequest {
        name: request.name,
        description: request.description,
        team_size: request.team_size,
        provider: request.provider,
        model: request.model,
        model_pool: request.model_pool,
    })?;
    let agency = generate_agency_from_prepared(prepared.clone(), state).await?;
    let seeds = if seed_memories {
        generate_seed_memories(&prepared, &agency, state).await?
    } else {
        Vec::new()
    };

    let workspace_root = workspace_root_path("agency_create").map_err(ApiError::bad_request)?;
    let canonical_root = canonical_workspace_root(&workspace_root, "agency_create")
        .map_err(ApiError::bad_request)?;
    let output_dir = output_dir.unwrap_or_else(|| agency_dir_slug(&agency.name));
    let target_dir = resolve_workspace_write_path(&canonical_root, &output_dir, "agency_create")
        .map_err(ApiError::bad_request)?;
    // materialize_agency_workspace is filesystem-heavy (creates a directory
    // tree, writes several files). Run it on the blocking pool so it doesn't
    // stall a tokio worker.
    let files = {
        let canonical_root = canonical_root.clone();
        let target_dir = target_dir.clone();
        let agency_for_fs = agency.clone();
        let seeds_for_fs = seeds.clone();
        tokio::task::spawn_blocking(move || {
            materialize_agency_workspace(
                &canonical_root,
                &target_dir,
                overwrite,
                &agency_for_fs,
                &seeds_for_fs,
            )
        })
        .await
        .map_err(|error| {
            ApiError::bad_request(format!("agency materialize worker panicked: {error}"))
        })??
    };
    let output_dir =
        normalized_relative_path(&canonical_root, &target_dir).map_err(ApiError::bad_request)?;
    let seed_memory_count = seeds.iter().map(|seed| seed.entries.len() as u64).sum();
    let seeded_agents = seeds.len() as u64;

    Ok(AgencyCreateResponse {
        agency,
        output_dir,
        files,
        seed_memory_count,
        seeded_agents,
    })
}

async fn generate_agency_from_request(
    request: AgencyGenerateRequest,
    state: &SharedDaemonState,
) -> Result<AgencyGenerateResponse, ApiError> {
    let prepared = prepare_generate_request(request)?;

    generate_agency_from_prepared(prepared, state).await
}

fn prepare_generate_request(
    request: AgencyGenerateRequest,
) -> Result<PreparedAgencyRequest, ApiError> {
    let name = require_trimmed(request.name, "name is required")?;
    let description = require_trimmed(request.description, "description is required")?;
    let team_size = request
        .team_size
        .map(|n| n.clamp(TEAM_MIN, TEAM_MAX))
        .unwrap_or(DEFAULT_TEAM_SIZE);

    let provider = request
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("deterministic")
        .to_string();
    let model = request
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("claude-sonnet-4-6")
        .to_string();

    let model_pool: Vec<String> = request
        .model_pool
        .unwrap_or_default()
        .into_iter()
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect();

    Ok(PreparedAgencyRequest {
        name,
        description,
        team_size,
        provider,
        model,
        model_pool,
    })
}

async fn generate_agency_from_prepared(
    prepared: PreparedAgencyRequest,
    state: &SharedDaemonState,
) -> Result<AgencyGenerateResponse, ApiError> {
    let PreparedAgencyRequest {
        name,
        description,
        team_size,
        provider,
        model,
        model_pool,
    } = prepared;

    let (adapter, tool_registry): (Arc<dyn ModelAdapter>, ToolRegistry) = {
        let guard = state.read().await;
        (
            Arc::clone(&guard.model_adapter),
            guard.tool_registry.clone(),
        )
    };
    let prompt = build_prompt(
        &name,
        &description,
        team_size,
        &model_pool,
        &tool_registry.tool_names(),
    );

    let agent_config = AgentConfig {
        name: "agency-generator".into(),
        model: model.clone(),
        bio: None,
        lore: None,
        knowledge: None,
        topics: None,
        adjectives: None,
        style: None,
        provider: Some(provider.clone()),
        system: Some("You are a helpful assistant that outputs only valid JSON.".into()),
        tools: None,
        plugins: None,
        settings: None,
    };

    let request = ModelGenerateRequest {
        system: "You are a helpful assistant that outputs only valid JSON.".into(),
        messages: vec![Message {
            id: String::new(),
            agent_id: String::new(),
            room_id: String::new(),
            content: Content {
                text: prompt,
                attachments: None,
                metadata: None,
            },
            role: MessageRole::User,
            created_at_ms: 0,
        }],
        temperature: Some(0.7),
        max_tokens: None,
    };

    let response = adapter
        .generate(&agent_config, &request)
        .await
        .map_err(|message| ApiError::bad_request(format!("model error: {message}")))?;

    let agents = parse_agents_payload(&response.content.text, &tool_registry)?;
    let (mission, values, definitions) = agents;

    Ok(AgencyGenerateResponse {
        name,
        description,
        provider,
        model,
        team_size,
        mission,
        values,
        agents: definitions,
    })
}

fn materialize_agency_workspace(
    workspace_root: &Path,
    output_dir: &Path,
    overwrite: bool,
    agency: &AgencyGenerateResponse,
    seeds: &[AgentSeedMemories],
) -> Result<Vec<String>, ApiError> {
    if output_dir.exists() {
        if !output_dir.is_dir() {
            return Err(ApiError::bad_request(format!(
                "agency output path is not a directory: {}",
                output_dir.display()
            )));
        }
        if !overwrite {
            return Err(ApiError::bad_request(format!(
                "agency output directory already exists: {}",
                output_dir.display()
            )));
        }

        fs::remove_dir_all(output_dir).map_err(|error| {
            ApiError::bad_request(format!(
                "failed to clear agency directory {}: {error}",
                output_dir.display()
            ))
        })?;
    }

    fs::create_dir_all(output_dir).map_err(|error| {
        ApiError::bad_request(format!(
            "failed to create agency directory {}: {error}",
            output_dir.display()
        ))
    })?;

    let (orchestrator, workers) = split_roster(agency)?;
    let mut written_files = Vec::new();
    let seeds_by_name = seeds
        .iter()
        .map(|seed| (seed.agent_name.as_str(), seed))
        .collect::<HashMap<_, _>>();

    write_workspace_file(
        workspace_root,
        &output_dir.join("anima.yaml"),
        &render_agency_yaml(agency, orchestrator, &workers)?,
        &mut written_files,
    )?;
    write_workspace_file(
        workspace_root,
        &output_dir.join("org-chart.mmd"),
        &render_org_chart(agency, orchestrator, &workers),
        &mut written_files,
    )?;
    write_workspace_file(
        workspace_root,
        &output_dir.join("README.md"),
        &render_agency_brief(agency, orchestrator, &workers),
        &mut written_files,
    )?;

    let agents_root = output_dir.join("agents");
    fs::create_dir_all(&agents_root).map_err(|error| {
        ApiError::bad_request(format!(
            "failed to create agents directory {}: {error}",
            agents_root.display()
        ))
    })?;

    for agent in std::iter::once(orchestrator).chain(workers.iter().copied()) {
        let agent_dir = agents_root.join(agent_slug(&agent.name));
        let assets_dir = agent_dir.join("assets");
        let memory_dir = agent_dir.join("memory");
        fs::create_dir_all(&assets_dir).map_err(|error| {
            ApiError::bad_request(format!(
                "failed to create assets directory {}: {error}",
                assets_dir.display()
            ))
        })?;
        fs::create_dir_all(&memory_dir).map_err(|error| {
            ApiError::bad_request(format!(
                "failed to create memory directory {}: {error}",
                memory_dir.display()
            ))
        })?;

        write_workspace_file(
            workspace_root,
            &agent_dir.join("profile.md"),
            &render_agent_profile(agent, &agency.name),
            &mut written_files,
        )?;
        write_workspace_file(
            workspace_root,
            &assets_dir.join(".gitkeep"),
            "",
            &mut written_files,
        )?;

        if let Some(seed_set) = seeds_by_name.get(agent.name.as_str()) {
            let seed_json = serde_json::to_string_pretty(&seed_set.entries).map_err(|error| {
                ApiError::bad_request(format!(
                    "failed to serialize seed memories for {}: {error}",
                    agent.name
                ))
            })?;
            write_workspace_file(
                workspace_root,
                &memory_dir.join("seed.json"),
                &seed_json,
                &mut written_files,
            )?;
        } else {
            write_workspace_file(
                workspace_root,
                &memory_dir.join(".gitkeep"),
                "",
                &mut written_files,
            )?;
        }
    }

    written_files.sort();
    Ok(written_files)
}

fn write_workspace_file(
    workspace_root: &Path,
    path: &Path,
    content: &str,
    written_files: &mut Vec<String>,
) -> Result<(), ApiError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            ApiError::bad_request(format!("failed to create {}: {error}", parent.display()))
        })?;
    }

    fs::write(path, content).map_err(|error| {
        ApiError::bad_request(format!("failed to write {}: {error}", path.display()))
    })?;

    written_files
        .push(normalized_relative_path(workspace_root, path).map_err(ApiError::bad_request)?);
    Ok(())
}

fn split_roster<'a>(
    agency: &'a AgencyGenerateResponse,
) -> Result<
    (
        &'a AgentDefinitionResponse,
        Vec<&'a AgentDefinitionResponse>,
    ),
    ApiError,
> {
    let Some(orchestrator_index) = agency
        .agents
        .iter()
        .position(|agent| agent.role == "orchestrator")
        .or_else(|| (!agency.agents.is_empty()).then_some(0))
    else {
        return Err(ApiError::bad_request_static(
            "agency draft must contain at least one agent",
        ));
    };

    let orchestrator = &agency.agents[orchestrator_index];
    let workers = agency
        .agents
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != orchestrator_index)
        .map(|(_, agent)| agent)
        .collect::<Vec<_>>();

    if workers.is_empty() {
        return Err(ApiError::bad_request_static(
            "agency draft must contain at least one worker",
        ));
    }

    Ok((orchestrator, workers))
}

async fn generate_seed_memories(
    prepared: &PreparedAgencyRequest,
    agency: &AgencyGenerateResponse,
    state: &SharedDaemonState,
) -> Result<Vec<AgentSeedMemories>, ApiError> {
    let adapter: Arc<dyn ModelAdapter> = {
        let guard = state.read().await;
        Arc::clone(&guard.model_adapter)
    };

    let agent_config = AgentConfig {
        name: "agency-seed-generator".into(),
        model: prepared.model.clone(),
        bio: None,
        lore: None,
        knowledge: None,
        topics: None,
        adjectives: None,
        style: None,
        provider: Some(prepared.provider.clone()),
        system: Some("You are a helpful assistant that outputs only valid JSON.".into()),
        tools: None,
        plugins: None,
        settings: None,
    };

    let request = ModelGenerateRequest {
        system: "You are a helpful assistant that outputs only valid JSON.".into(),
        messages: vec![Message {
            id: String::new(),
            agent_id: String::new(),
            room_id: String::new(),
            content: Content {
                text: build_seed_prompt(agency),
                attachments: None,
                metadata: None,
            },
            role: MessageRole::User,
            created_at_ms: 0,
        }],
        temperature: Some(0.7),
        max_tokens: None,
    };

    let response = adapter
        .generate(&agent_config, &request)
        .await
        .map_err(|message| ApiError::bad_request(format!("seed model error: {message}")))?;

    parse_seed_payload(&response.content.text, &agency.agents)
}

fn build_seed_prompt(agency: &AgencyGenerateResponse) -> String {
    let agent_summaries = agency.agents.iter().map(|agent| {
        let mut lines = vec![
            format!("Name: {}", agent.name),
            format!(
                "Position: {}",
                agent.position.as_deref().unwrap_or("unspecified")
            ),
        ];
        if let Some(bio) = &agent.bio {
            lines.push(format!("Bio: {bio}"));
        }
        if let Some(topics) = &agent.topics {
            if !topics.is_empty() {
                lines.push(format!("Expertise: {}", topics.join(", ")));
            }
        }
        if let Some(knowledge) = &agent.knowledge {
            if !knowledge.is_empty() {
                lines.push(format!("Knows: {}", knowledge.join("; ")));
            }
        }
        lines.join("\n")
    });

    let mut lines = vec![
        format!("Agency: \"{}\"", agency.name),
        format!("Purpose: {}", agency.description),
    ];
    if let Some(mission) = &agency.mission {
        lines.push(format!("Mission: {mission}"));
    }
    lines.push(String::new());
    lines.push(
        "For each agent below, generate 3-5 seed memories — concrete facts, observations,".into(),
    );
    lines.push(
        "or prior knowledge this person would realistically hold given their role and the agency context.".into(),
    );
    lines.push("Make them specific and useful, not generic platitudes.".into());
    lines.push(String::new());
    lines.push("Agents:".into());
    for (index, summary) in agent_summaries.enumerate() {
        lines.push(format!("\n[{}]\n{}", index + 1, summary));
    }
    lines.push(String::new());
    lines.push("Respond with ONLY valid JSON — a single object, no markdown.".into());
    lines.push("{".into());
    lines.push("  \"seeds\": [".into());
    lines.push("    {".into());
    lines.push("      \"agentName\": string  — exactly as given above".into());
    lines.push("      \"memories\": [".into());
    lines.push("        {".into());
    lines.push(
        "          \"type\": \"fact\" | \"observation\" | \"task_result\" | \"reflection\"".into(),
    );
    lines.push("          \"content\": string  — the memory (1-2 sentences)".into());
    lines.push(
        "          \"importance\": number  — 0.0 to 1.0, how relevant this is to day-to-day work"
            .into(),
    );
    lines.push("          \"tags\": string[]  — 1-3 short labels (optional)".into());
    lines.push("        }".into());
    lines.push("      ]".into());
    lines.push("    }".into());
    lines.push("  ]".into());
    lines.push("}".into());

    lines.join("\n")
}

fn parse_seed_payload(
    raw: &str,
    agents: &[AgentDefinitionResponse],
) -> Result<Vec<AgentSeedMemories>, ApiError> {
    let cleaned = strip_code_fences(raw);
    let parsed: Value = serde_json::from_str(&cleaned).map_err(|error| {
        ApiError::bad_request(format!("seed model returned invalid JSON: {error}"))
    })?;
    let raw_seeds = parsed
        .as_object()
        .and_then(|value| value.get("seeds"))
        .and_then(Value::as_array)
        .ok_or_else(|| ApiError::bad_request_static("seed model JSON missing \"seeds\" array"))?;

    let known_agents = agents
        .iter()
        .map(|agent| (agent.name.as_str(), agent))
        .collect::<HashMap<_, _>>();
    let mut seeds_by_name = HashMap::new();

    for raw_seed in raw_seeds {
        let Some(object) = raw_seed.as_object() else {
            continue;
        };
        let Some(agent_name) = object.get("agentName").and_then(Value::as_str) else {
            continue;
        };
        if !known_agents.contains_key(agent_name) {
            continue;
        }
        let Some(raw_memories) = object.get("memories").and_then(Value::as_array) else {
            continue;
        };
        let entries = raw_memories
            .iter()
            .filter_map(map_seed_memory_entry)
            .collect::<Vec<_>>();
        if !entries.is_empty() {
            seeds_by_name.insert(
                agent_name.to_string(),
                AgentSeedMemories {
                    agent_name: agent_name.to_string(),
                    entries,
                },
            );
        }
    }

    Ok(agents
        .iter()
        .filter_map(|agent| seeds_by_name.remove(&agent.name))
        .collect())
}

fn map_seed_memory_entry(raw: &Value) -> Option<SeedMemoryEntry> {
    let object = raw.as_object()?;
    let kind = object.get("type")?.as_str()?.trim().to_string();
    if !matches!(
        kind.as_str(),
        "fact" | "observation" | "task_result" | "reflection"
    ) {
        return None;
    }

    let content = object.get("content")?.as_str()?.trim().to_string();
    if content.is_empty() {
        return None;
    }

    let importance = object
        .get("importance")
        .and_then(Value::as_f64)
        .filter(|value| *value >= 0.0 && *value <= 1.0)
        .unwrap_or(0.5);

    let tags = object
        .get("tags")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|value| value.as_str().map(|item| item.to_string()))
                .collect::<Vec<_>>()
        })
        .filter(|items| !items.is_empty());

    Some(SeedMemoryEntry {
        kind,
        content,
        importance,
        tags,
    })
}

fn render_agency_yaml(
    agency: &AgencyGenerateResponse,
    orchestrator: &AgentDefinitionResponse,
    workers: &[&AgentDefinitionResponse],
) -> Result<String, ApiError> {
    let config = AgencyYamlConfig {
        name: agency.name.clone(),
        description: agency.description.clone(),
        mission: agency.mission.clone(),
        values: agency.values.clone(),
        model: agency.model.clone(),
        provider: agency.provider.clone(),
        strategy: "supervisor".to_string(),
        max_parallel_delegations: None,
        orchestrator: agency_yaml_agent(orchestrator),
        agents: workers
            .iter()
            .map(|agent| agency_yaml_agent(agent))
            .collect(),
    };

    serde_yaml::to_string(&config)
        .map_err(|error| ApiError::bad_request(format!("failed to serialize agency yaml: {error}")))
}

fn agency_yaml_agent(agent: &AgentDefinitionResponse) -> AgencyYamlAgent {
    AgencyYamlAgent {
        name: agent.name.clone(),
        position: agent.position.clone(),
        bio: agent.bio.clone().unwrap_or_default(),
        lore: agent.lore.clone(),
        knowledge: agent.knowledge.clone(),
        topics: agent.topics.clone(),
        adjectives: agent.adjectives.clone(),
        style: agent.style.clone(),
        system: agent.system.clone().unwrap_or_default(),
        model: agent.model.clone(),
        tools: agent.tools.clone(),
        collaborates_with: agent.collaborates_with.clone(),
    }
}

fn render_org_chart(
    agency: &AgencyGenerateResponse,
    orchestrator: &AgentDefinitionResponse,
    workers: &[&AgentDefinitionResponse],
) -> String {
    let mut lines = vec![
        "%% Auto-generated by `animaos create`".to_string(),
        format!("%% Agency: {}", agency.name),
    ];
    if let Some(mission) = &agency.mission {
        lines.push(format!("%% Mission: {mission}"));
    }
    lines.push(String::new());
    lines.push("flowchart TD".to_string());

    let orchestrator_id = node_id(&orchestrator.name);
    lines.push(format!(
        "  {orchestrator_id}[\"{}\"]:::orchestrator",
        node_label(orchestrator)
    ));

    for worker in workers {
        lines.push(format!(
            "  {}[\"{}\"]:::worker",
            node_id(&worker.name),
            node_label(worker)
        ));
    }

    for worker in workers {
        lines.push(format!("  {orchestrator_id} --> {}", node_id(&worker.name)));
    }

    let mut drawn_pairs = BTreeSet::new();
    draw_collaboration_edges(orchestrator, &mut drawn_pairs, &mut lines);
    for worker in workers {
        draw_collaboration_edges(worker, &mut drawn_pairs, &mut lines);
    }

    lines.push(String::new());
    lines.push("  classDef orchestrator fill:#fef3c7,stroke:#b45309,stroke-width:2px;".to_string());
    lines.push("  classDef worker fill:#dbeafe,stroke:#1e40af,stroke-width:1px;".to_string());
    lines.join("\n")
}

fn draw_collaboration_edges(
    agent: &AgentDefinitionResponse,
    drawn_pairs: &mut BTreeSet<String>,
    lines: &mut Vec<String>,
) {
    for peer in agent.collaborates_with.clone().unwrap_or_default() {
        let pair = if agent.name <= peer {
            format!("{}::{peer}", agent.name)
        } else {
            format!("{peer}::{}", agent.name)
        };
        if drawn_pairs.insert(pair) {
            lines.push(format!(
                "  {} -.->|collab| {}",
                node_id(&agent.name),
                node_id(&peer)
            ));
        }
    }
}

fn render_agency_brief(
    agency: &AgencyGenerateResponse,
    orchestrator: &AgentDefinitionResponse,
    workers: &[&AgentDefinitionResponse],
) -> String {
    let mut lines = vec![format!("# {}", agency.name), String::new()];
    lines.push(format!("> {}", agency.description));
    lines.push(String::new());

    if let Some(mission) = &agency.mission {
        lines.push("## Mission".to_string());
        lines.push(String::new());
        lines.push(mission.clone());
        lines.push(String::new());
    }

    if let Some(values) = &agency.values {
        if !values.is_empty() {
            lines.push("## Values".to_string());
            lines.push(String::new());
            for value in values {
                lines.push(format!("- {value}"));
            }
            lines.push(String::new());
        }
    }

    lines.push("## Org chart".to_string());
    lines.push(String::new());
    lines.push("```mermaid".to_string());
    lines.push(render_org_chart(agency, orchestrator, workers));
    lines.push("```".to_string());
    lines.push(String::new());
    lines.push("## Roster".to_string());
    lines.push(String::new());
    push_roster_section(&mut lines, orchestrator, "★");
    for worker in workers {
        push_roster_section(&mut lines, worker, "•");
    }

    lines.join("\n")
}

fn push_roster_section(lines: &mut Vec<String>, agent: &AgentDefinitionResponse, marker: &str) {
    let title = match &agent.position {
        Some(position) => format!("{} — {position}", agent.name),
        None => agent.name.clone(),
    };
    lines.push(format!("### {marker} {title}"));
    lines.push(String::new());
    lines.push(agent.bio.clone().unwrap_or_default());
    if let Some(tools) = &agent.tools {
        if !tools.is_empty() {
            lines.push(String::new());
            lines.push(format!("**Skills:** {}", tools.join(", ")));
        }
    }
    if let Some(collaborators) = &agent.collaborates_with {
        if !collaborators.is_empty() {
            lines.push(format!(
                "**Collaborates with:** {}",
                collaborators.join(", ")
            ));
        }
    }
    lines.push(String::new());
}

fn render_agent_profile(agent: &AgentDefinitionResponse, agency_name: &str) -> String {
    let headline = match &agent.position {
        Some(position) => format!("{} — {position}", agent.name),
        None => agent.name.clone(),
    };
    let mut lines = vec![format!("# {headline}"), String::new()];
    if agent.role == "orchestrator" {
        lines.push(format!("*{agency_name} · orchestrator*"));
    } else {
        lines.push(format!("*{agency_name}*"));
    }
    lines.push(String::new());
    lines.push("## Bio".to_string());
    lines.push(String::new());
    lines.push(agent.bio.clone().unwrap_or_default());
    lines.push(String::new());

    if let Some(lore) = &agent.lore {
        lines.push("## Backstory".to_string());
        lines.push(String::new());
        lines.push(lore.clone());
        lines.push(String::new());
    }

    if let Some(adjectives) = &agent.adjectives {
        if !adjectives.is_empty() {
            lines.push(format!("**Personality:** {}", adjectives.join(", ")));
            lines.push(String::new());
        }
    }

    if let Some(style) = &agent.style {
        lines.push(format!("**Communication style:** {style}"));
        lines.push(String::new());
    }

    if let Some(topics) = &agent.topics {
        if !topics.is_empty() {
            lines.push("## Expertise".to_string());
            lines.push(String::new());
            for topic in topics {
                lines.push(format!("- {topic}"));
            }
            lines.push(String::new());
        }
    }

    if let Some(knowledge) = &agent.knowledge {
        if !knowledge.is_empty() {
            lines.push("## Knows deeply".to_string());
            lines.push(String::new());
            for item in knowledge {
                lines.push(format!("- {item}"));
            }
            lines.push(String::new());
        }
    }

    if let Some(tools) = &agent.tools {
        if !tools.is_empty() {
            lines.push("## Skills".to_string());
            lines.push(String::new());
            for tool in tools {
                lines.push(format!("- 🔧 `{tool}`"));
            }
            lines.push(String::new());
        }
    }

    if let Some(collaborators) = &agent.collaborates_with {
        if !collaborators.is_empty() {
            lines.push("## Frequent collaborators".to_string());
            lines.push(String::new());
            for collaborator in collaborators {
                lines.push(format!("- {collaborator}"));
            }
            lines.push(String::new());
        }
    }

    lines.push("## Mandate".to_string());
    lines.push(String::new());
    lines.push(agent.system.clone().unwrap_or_default());
    lines.push(String::new());
    lines.push("---".to_string());
    lines.push(String::new());
    lines.push("## Workspace".to_string());
    lines.push(String::new());
    lines.push("- `assets/` — avatars, images, brand kit (drop files here)".to_string());
    lines.push("- `memory/` — agent-specific memory and notes".to_string());
    lines.push(
        "- `memory/seed.json` — optional pre-loaded memories, posted to the daemon on every launch"
            .to_string(),
    );

    lines.join("\n")
}

fn agency_dir_slug(name: &str) -> String {
    name.split_whitespace()
        .map(|segment| segment.to_lowercase())
        .collect::<Vec<_>>()
        .join("-")
}

fn agent_slug(name: &str) -> String {
    name.trim()
        .chars()
        .map(|ch| match ch {
            '_' | ' ' => '-',
            value if value.is_ascii_alphanumeric() || value == '-' => value.to_ascii_lowercase(),
            _ => '\0',
        })
        .filter(|ch| *ch != '\0')
        .collect()
}

fn node_id(name: &str) -> String {
    name.chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
}

fn node_label(agent: &AgentDefinitionResponse) -> String {
    let mut lines = vec![format!("<b>{}</b>", agent.name)];
    if let Some(position) = &agent.position {
        lines.push(format!("<i>{position}</i>"));
    }
    if let Some(tools) = &agent.tools {
        if !tools.is_empty() {
            lines.push("---".to_string());
            lines.push(
                tools
                    .iter()
                    .map(|tool| format!("🔧 {tool}"))
                    .collect::<Vec<_>>()
                    .join("<br/>"),
            );
        }
    }
    lines.join("<br/>")
}

fn require_trimmed(value: Option<String>, message: &'static str) -> Result<String, ApiError> {
    let trimmed = value.unwrap_or_default().trim().to_string();
    if trimmed.is_empty() {
        Err(ApiError::bad_request_static(message))
    } else {
        Ok(trimmed)
    }
}

fn build_prompt(
    name: &str,
    description: &str,
    team_size: u64,
    model_pool: &[String],
    supported_tools: &[String],
) -> String {
    let worker_count = team_size.saturating_sub(1);
    let needs_skeptic = worker_count >= 3;

    let mut lines: Vec<String> = Vec::with_capacity(48);
    lines.push(format!(
        "You are designing a team of AI agents for an agency called \"{name}\"."
    ));
    lines.push(format!("Agency purpose: {description}"));
    lines.push(String::new());
    lines.push(format!(
        "Generate EXACTLY {team_size} agents in total: 1 orchestrator + {worker_count} workers."
    ));
    lines.push(
        "The first agent must be the orchestrator — the one who coordinates the team.".into(),
    );
    lines.push("Workers should have focused, distinct mandates.".into());
    lines.push(String::new());
    lines.push(
        "OVERLAP RULE: when cross-validation, multiple perspectives, or parallel exploration adds clear value,"
            .into(),
    );
    lines.push(
        "you may include 2-3 agents in similar roles but with DIFFERENT angles or methodologies"
            .into(),
    );
    lines.push(
        "(e.g. researcher_quantitative + researcher_qualitative, or writer_long_form + writer_punchy)."
            .into(),
    );
    lines.push(
        "Never duplicate an agent verbatim — each must contribute something distinct.".into(),
    );

    if needs_skeptic {
        lines.push(String::new());
        lines.push(
            "SKEPTIC RULE: one worker must be a dedicated contrarian — their explicit job is to challenge"
                .into(),
        );
        lines.push(
            "assumptions, poke holes in plans, and surface risks others miss. Their \"system\" must include"
                .into(),
        );
        lines.push(
            "an instruction to actively disagree when they see flaws, not to reach consensus."
                .into(),
        );
        lines.push(
            "Give this agent adjectives like \"skeptical\", \"rigorous\", \"contrarian\".".into(),
        );
    }

    lines.push(String::new());
    lines.push(
        "ANTI-SYCOPHANCY: every worker's \"system\" field MUST include a sentence instructing them to"
            .into(),
    );
    lines.push(
        "challenge assumptions and disagree with the orchestrator whenever they have a different view."
            .into(),
    );
    lines.push("Workers should surface dissent, not defer.".into());

    if !model_pool.is_empty() {
        let pool_list = model_pool
            .iter()
            .map(|m| format!("\"{m}\""))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(String::new());
        lines.push(format!(
            "MODEL POOL — assign each agent one model from this list: [{pool_list}]."
        ));
        lines.push("Distribute them so no two adjacent collaborators share the same model.".into());
        lines.push(
            "Each AgentObject must include a \"model\" field set to one of the listed values."
                .into(),
        );
    }

    lines.push(String::new());
    lines.push(
        "Respond with ONLY valid JSON — a single object. No markdown, no explanation.".into(),
    );
    let tool_list = supported_tools
        .iter()
        .map(|tool| format!("\"{tool}\""))
        .collect::<Vec<_>>()
        .join(", ");
    lines.push(format!(
        "Use only tool names from this exact daemon-supported list: [{tool_list}]"
    ));
    lines.push("The object MUST have this shape:".into());
    lines.push("{".into());
    lines.push("  \"mission\": string  — one-sentence north star the whole team shares".into());
    lines.push(
        "  \"values\": string[] — 3-5 cultural principles the team operates under (short phrases)"
            .into(),
    );
    lines.push("  \"agents\": AgentObject[]  — exactly the requested size".into());
    lines.push("}".into());
    lines.push(String::new());
    lines.push("Each AgentObject must have:".into());
    lines.push(
        "  - \"name\": a real human name. First name only when distinct (e.g. \"Sarah\", \"Marcus\", \"Aiko\"),"
            .into(),
    );
    lines.push(
        "             OR full first + last name when richer characterization fits (e.g. \"Sarah Chen\","
            .into(),
    );
    lines.push(
        "             \"Marcus Rivera\", \"Aiko Tanaka\"). Pick culturally diverse names that fit each"
            .into(),
    );
    lines.push(
        "             personality. NEVER use single-letter suffixes or initials like \"Sarah_C\" — if two"
            .into(),
    );
    lines.push(
        "             agents would share a first name, give them different first names entirely or use"
            .into(),
    );
    lines.push(
        "             full last names. Treat each agent as a real teammate, not a serial number."
            .into(),
    );
    lines.push(
        "  - \"position\": real-world job title (e.g. \"Head of Growth\", \"Chief Brand Officer\")"
            .into(),
    );
    lines.push("  - \"role\": either \"orchestrator\" or \"worker\"".into());
    lines.push("  - \"bio\": 1-2 sentences — personality and expertise".into());
    lines.push("  - \"lore\": 1-2 sentences of backstory".into());
    lines.push("  - \"adjectives\": array of 3-5 personality trait words".into());
    lines.push("  - \"topics\": array of 3-6 short expertise tags".into());
    lines.push("  - \"knowledge\": array of 2-4 specific things this agent knows deeply".into());
    lines.push("  - \"style\": 1-2 sentences describing how this agent communicates".into());
    lines.push(
        "  - \"system\": core instruction — what they do, decide, and own (2-3 sentences). Must include"
            .into(),
    );
    lines.push(
        "              a line instructing the agent to voice disagreement when they see a better path."
            .into(),
    );
    lines.push(
        "  - \"tools\": array of 1-5 tool names chosen only from the daemon-supported list above"
            .into(),
    );
    if !model_pool.is_empty() {
        lines.push("  - \"model\": one model from the provided pool".into());
    }
    lines.push(
        "  - \"collaborates_with\": array of agent names (snake_case) this agent frequently pairs with."
            .into(),
    );
    lines.push(
        "             Use this to express working relationships — which workers naturally hand off to,"
            .into(),
    );
    lines.push(
        "             review, or build on each other. Reference names that exist in this same array.".into(),
    );
    lines.push(
        "             The orchestrator may leave this empty (it implicitly delegates to all workers)."
            .into(),
    );

    lines.join("\n")
}

fn parse_agents_payload(
    raw: &str,
    tool_registry: &ToolRegistry,
) -> Result<
    (
        Option<String>,
        Option<Vec<String>>,
        Vec<AgentDefinitionResponse>,
    ),
    ApiError,
> {
    let cleaned = strip_code_fences(raw);

    let parsed: Value = serde_json::from_str(&cleaned)
        .map_err(|error| ApiError::bad_request(format!("model returned invalid JSON: {error}")))?;

    let mut mission: Option<String> = None;
    let mut values: Option<Vec<String>> = None;
    let raw_agents = match parsed {
        Value::Array(items) => items,
        Value::Object(mut map) => {
            mission = map
                .remove("mission")
                .and_then(|v| v.as_str().map(|s| s.to_string()));
            values = map
                .remove("values")
                .and_then(|v| v.as_array().cloned())
                .map(|items| {
                    items
                        .into_iter()
                        .filter_map(|item| item.as_str().map(|s| s.to_string()))
                        .collect()
                });
            map.remove("agents")
                .and_then(|v| v.as_array().cloned())
                .ok_or_else(|| {
                    ApiError::bad_request_static("model JSON missing \"agents\" array")
                })?
        }
        _ => {
            return Err(ApiError::bad_request_static(
                "model returned a JSON value that is neither an object nor an array",
            ));
        }
    };

    let agents = raw_agents
        .into_iter()
        .map(map_agent_definition)
        .collect::<Vec<_>>();

    let agents = normalize_agent_definitions(agents, tool_registry);

    Ok((mission, values, agents))
}

fn normalize_agent_definitions(
    mut agents: Vec<AgentDefinitionResponse>,
    tool_registry: &ToolRegistry,
) -> Vec<AgentDefinitionResponse> {
    let first_orchestrator = agents.iter().position(|agent| agent.role == "orchestrator");

    for (index, agent) in agents.iter_mut().enumerate() {
        agent.tools = agent.tools.take().map(|tools| {
            let mut filtered = tools
                .into_iter()
                .filter(|tool| tool_registry.lookup(tool).is_some())
                .collect::<Vec<_>>();
            filtered.sort();
            filtered.dedup();
            filtered
        });
        if matches!(agent.tools.as_ref(), Some(tools) if tools.is_empty()) {
            agent.tools = None;
        }

        if let Some(orchestrator_index) = first_orchestrator {
            if index != orchestrator_index && agent.role == "orchestrator" {
                agent.role = "worker".to_string();
            }
        }
    }

    if first_orchestrator.is_none() && !agents.is_empty() {
        agents[0].role = "orchestrator".to_string();
    }

    agents
}

fn map_agent_definition(raw: Value) -> AgentDefinitionResponse {
    let object = raw.as_object();
    let get_str = |key: &str| -> Option<String> {
        object
            .and_then(|map| map.get(key))
            .and_then(|value| value.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let get_str_array = |keys: &[&str]| -> Option<Vec<String>> {
        for key in keys {
            if let Some(items) = object
                .and_then(|map| map.get(*key))
                .and_then(|value| value.as_array())
            {
                let collected: Vec<String> = items
                    .iter()
                    .filter_map(|value| value.as_str().map(|s| s.to_string()))
                    .collect();
                if !collected.is_empty() {
                    return Some(collected);
                }
            }
        }
        None
    };

    let role = get_str("role")
        .map(|value| value.to_lowercase())
        .filter(|value| value == "orchestrator" || value == "worker")
        .unwrap_or_else(|| "worker".to_string());

    AgentDefinitionResponse {
        name: get_str("name").unwrap_or_else(|| "unnamed".into()),
        position: get_str("position"),
        role,
        bio: get_str("bio"),
        lore: get_str("lore"),
        adjectives: get_str_array(&["adjectives"]),
        topics: get_str_array(&["topics"]),
        knowledge: get_str_array(&["knowledge"]),
        style: get_str("style"),
        system: get_str("system"),
        model: get_str("model"),
        tools: get_str_array(&["tools"]),
        collaborates_with: get_str_array(&["collaborates_with", "collaboratesWith"]),
    }
}

fn strip_code_fences(value: &str) -> String {
    let mut text = value.trim().to_string();
    if text.starts_with("```") {
        if let Some(rest) = text.strip_prefix("```") {
            let after = rest.trim_start_matches(|c: char| c.is_alphanumeric());
            let after = after.strip_prefix('\n').unwrap_or(after);
            text = after.to_string();
        }
        if let Some(rest) = text.strip_suffix("```") {
            text = rest.trim_end().to_string();
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        map_seed_memory_entry, materialize_agency_workspace, normalize_agent_definitions,
        parse_agents_payload, parse_seed_payload, strip_code_fences, AgencyGenerateResponse,
        AgentDefinitionResponse, AgentSeedMemories, SeedMemoryEntry,
    };
    use crate::tools::ToolRegistry;
    use serde_json::json;

    #[test]
    fn strips_markdown_fences() {
        let raw = "```json\n{\"agents\": []}\n```";
        assert_eq!(strip_code_fences(raw).trim(), "{\"agents\": []}");
    }

    #[test]
    fn parses_full_payload() {
        let raw = r#"{
            "mission": "Make journalism great again",
            "values": ["accuracy", "speed"],
            "agents": [
                {
                    "name": "Sarah Chen",
                    "position": "Editor in chief",
                    "role": "orchestrator",
                    "bio": "Lead editor",
                    "lore": "20 years at the desk",
                    "adjectives": ["sharp", "decisive"],
                    "topics": ["editorial", "deadlines"],
                    "knowledge": ["AP style"],
                    "style": "Direct",
                    "system": "Coordinate the team",
                    "tools": ["assign_story"],
                    "collaborates_with": []
                }
            ]
        }"#;
        let registry = ToolRegistry::new();
        let (mission, values, agents) = parse_agents_payload(raw, &registry).expect("parses");
        assert_eq!(mission.as_deref(), Some("Make journalism great again"));
        assert_eq!(
            values.as_ref().map(|v| v.len()),
            Some(2),
            "values should be parsed"
        );
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].name, "Sarah Chen");
        assert_eq!(agents[0].role, "orchestrator");
    }

    #[test]
    fn rejects_invalid_json() {
        let registry = ToolRegistry::new();
        let result = parse_agents_payload("not json at all", &registry);
        assert!(result.is_err());
    }

    #[test]
    fn normalizes_tools_and_orchestrator_role() {
        let registry = ToolRegistry::new();
        let agents = normalize_agent_definitions(
            vec![
                AgentDefinitionResponse {
                    name: "Avery".into(),
                    position: None,
                    role: "worker".into(),
                    bio: None,
                    lore: None,
                    adjectives: None,
                    topics: None,
                    knowledge: None,
                    style: None,
                    system: None,
                    model: None,
                    tools: Some(vec![
                        "web_fetch".into(),
                        "trend_forecast".into(),
                        "web_fetch".into(),
                    ]),
                    collaborates_with: None,
                },
                AgentDefinitionResponse {
                    name: "Jordan".into(),
                    position: None,
                    role: "orchestrator".into(),
                    bio: None,
                    lore: None,
                    adjectives: None,
                    topics: None,
                    knowledge: None,
                    style: None,
                    system: None,
                    model: None,
                    tools: Some(vec!["bg_list".into()]),
                    collaborates_with: None,
                },
                AgentDefinitionResponse {
                    name: "Taylor".into(),
                    position: None,
                    role: "orchestrator".into(),
                    bio: None,
                    lore: None,
                    adjectives: None,
                    topics: None,
                    knowledge: None,
                    style: None,
                    system: None,
                    model: None,
                    tools: None,
                    collaborates_with: None,
                },
            ],
            &registry,
        );

        assert_eq!(agents[0].role, "worker");
        assert_eq!(agents[1].role, "orchestrator");
        assert_eq!(agents[2].role, "worker");
        assert_eq!(agents[0].tools, Some(vec!["web_fetch".into()]));
        assert_eq!(agents[1].tools, Some(vec!["bg_list".into()]));
    }

    #[test]
    fn writes_cli_style_agency_artifacts() {
        let workspace = temp_workspace("agency-create-artifacts");
        let output_dir = workspace.join("northstar-studio");
        let agency = sample_agency();

        let files = materialize_agency_workspace(&workspace, &output_dir, false, &agency, &[])
            .expect("creates workspace artifacts");

        assert!(files.iter().any(|file| file.ends_with("anima.yaml")));
        assert!(files.iter().any(|file| file.ends_with("org-chart.mmd")));
        assert!(files.iter().any(|file| file.ends_with("README.md")));
        assert!(
            output_dir.join("agents/ava-chen/profile.md").exists(),
            "orchestrator profile should exist"
        );
        assert!(
            output_dir
                .join("agents/miles-rivera/assets/.gitkeep")
                .exists(),
            "worker asset placeholder should exist"
        );

        let readme = fs::read_to_string(output_dir.join("README.md")).expect("read readme");
        assert!(readme.contains("## Mission"));
        assert!(readme.contains("## Org chart"));

        let yaml = fs::read_to_string(output_dir.join("anima.yaml")).expect("read yaml");
        assert!(yaml.contains("strategy: supervisor"));
        assert!(yaml.contains("orchestrator:"));

        fs::remove_dir_all(&workspace).ok();
    }

    #[test]
    fn rejects_existing_directory_without_overwrite() {
        let workspace = temp_workspace("agency-create-overwrite");
        let output_dir = workspace.join("northstar-studio");
        fs::create_dir_all(&output_dir).expect("create existing output dir");

        let error =
            materialize_agency_workspace(&workspace, &output_dir, false, &sample_agency(), &[])
                .expect_err("existing directory should be rejected without overwrite");

        assert_eq!(error.status.as_u16(), 400);
        assert!(error.message.contains("already exists"));

        fs::remove_dir_all(&workspace).ok();
    }

    #[test]
    fn writes_seed_json_when_memories_are_present() {
        let workspace = temp_workspace("agency-create-seeds");
        let output_dir = workspace.join("northstar-studio");
        let agency = sample_agency();
        let seeds = vec![AgentSeedMemories {
            agent_name: "Ava Chen".into(),
            entries: vec![SeedMemoryEntry {
                kind: "fact".into(),
                content: "Keeps a private launch-review checklist for every major campaign.".into(),
                importance: 0.8,
                tags: Some(vec!["launch".into(), "process".into()]),
            }],
        }];

        let files = materialize_agency_workspace(&workspace, &output_dir, false, &agency, &seeds)
            .expect("creates workspace with seeds");

        assert!(files
            .iter()
            .any(|file| file.ends_with("agents/ava-chen/memory/seed.json")));
        let seed_json = fs::read_to_string(output_dir.join("agents/ava-chen/memory/seed.json"))
            .expect("read seed file");
        assert!(seed_json.contains("launch-review checklist"));
        assert!(
            !output_dir.join("agents/ava-chen/memory/.gitkeep").exists(),
            "seeded agent should not keep the placeholder file"
        );

        fs::remove_dir_all(&workspace).ok();
    }

    #[test]
    fn parses_seed_payload_for_known_agents() {
        let seeds = parse_seed_payload(
                        r#"{
                            "seeds": [
                                {
                                    "agentName": "Ava Chen",
                                    "memories": [
                                        {
                                            "type": "fact",
                                            "content": "Tracks narrative drift during campaign reviews.",
                                            "importance": 0.9,
                                            "tags": ["review"]
                                        }
                                    ]
                                },
                                {
                                    "agentName": "Unknown",
                                    "memories": [
                                        {
                                            "type": "fact",
                                            "content": "Should be ignored.",
                                            "importance": 0.7
                                        }
                                    ]
                                }
                            ]
                        }"#,
                        &sample_agency().agents,
                )
                .expect("parses seed payload");

        assert_eq!(seeds.len(), 1);
        assert_eq!(seeds[0].agent_name, "Ava Chen");
        assert_eq!(seeds[0].entries.len(), 1);
        assert_eq!(seeds[0].entries[0].kind, "fact");
    }

    #[test]
    fn rejects_invalid_seed_types() {
        let entry = map_seed_memory_entry(&json!({
                "type": "invalid",
                "content": "Bad type",
                "importance": 0.4
        }));

        assert!(entry.is_none());
    }

    fn sample_agency() -> AgencyGenerateResponse {
        AgencyGenerateResponse {
            name: "Northstar Studio".into(),
            description: "Turns launch chaos into crisp campaign systems.".into(),
            provider: "ollama".into(),
            model: "qwen3:latest".into(),
            team_size: 2,
            mission: Some("Launch with clarity and healthy internal dissent.".into()),
            values: Some(vec![
                "clarity".into(),
                "taste".into(),
                "challenge assumptions".into(),
            ]),
            agents: vec![
                AgentDefinitionResponse {
                    name: "Ava Chen".into(),
                    position: Some("Creative Director".into()),
                    role: "orchestrator".into(),
                    bio: Some("Shapes direction and keeps the brief coherent.".into()),
                    lore: Some("Built launch systems across product and media teams.".into()),
                    adjectives: Some(vec!["decisive".into(), "curious".into()]),
                    topics: Some(vec!["brand strategy".into(), "campaigns".into()]),
                    knowledge: Some(vec!["narrative systems".into()]),
                    style: Some("Direct, visual, and synthesis-heavy.".into()),
                    system: Some("Coordinate the team and force tradeoff clarity.".into()),
                    model: Some("qwen3:latest".into()),
                    tools: Some(vec!["web_fetch".into(), "bg_list".into()]),
                    collaborates_with: None,
                },
                AgentDefinitionResponse {
                    name: "Miles Rivera".into(),
                    position: Some("Growth Strategist".into()),
                    role: "worker".into(),
                    bio: Some(
                        "Pressure-tests positioning and finds the practical path to demand.".into(),
                    ),
                    lore: Some("Lives between analytics, messaging, and launch execution.".into()),
                    adjectives: Some(vec!["skeptical".into(), "pragmatic".into()]),
                    topics: Some(vec!["growth".into(), "positioning".into()]),
                    knowledge: Some(vec!["demand capture".into()]),
                    style: Some("Blunt, specific, and metrics-minded.".into()),
                    system: Some("Disagree when the plan lacks evidence or leverage.".into()),
                    model: Some("qwen3:latest".into()),
                    tools: Some(vec!["web_fetch".into()]),
                    collaborates_with: Some(vec!["Ava Chen".into()]),
                },
            ],
        }
    }

    fn temp_workspace(prefix: &str) -> std::path::PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("timestamp")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{prefix}-{stamp}"));
        fs::create_dir_all(&dir).expect("create temp workspace");
        dir
    }
}
