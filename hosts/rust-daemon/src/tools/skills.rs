//! `load_skill` and `propose_skill` (spec §8.3). `load_skill` reads an
//! owner-approved skill's instructions, checked against the hash the owner
//! approved; `propose_skill` leaves a draft for the owner and never touches
//! `SKILL.md`.

use anima_core::{AgentState, Content, DataValue, Message, TaskResult, ToolCall};
use futures::future::BoxFuture;

use super::ToolExecutionContext;
use crate::agent_runs::is_helper_config;
use crate::skills::{
    proposed_reply, Proposal, ProposedBy, SkillError, HELPERS_CANNOT_PROPOSE_SKILLS,
    PROPOSAL_NOT_SAVED, SKILLS_UNAVAILABLE, SKILL_INSTRUCTIONS_HEADER,
};

fn text_arg<'a>(call: &'a ToolCall, key: &str) -> Option<&'a str> {
    match call.args.get(key) {
        Some(DataValue::String(value)) => Some(value.as_str()),
        _ => None,
    }
}

fn text(text: String) -> TaskResult<Content> {
    TaskResult::success(
        Content {
            text,
            ..Content::default()
        },
        0,
    )
}

pub(super) fn load_skill(
    context: ToolExecutionContext,
    _agent: AgentState,
    _user_message: Message,
    call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    Box::pin(async move {
        let Some(name) = text_arg(&call, "name")
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
        else {
            return TaskResult::error("load_skill name must be a non-empty string", 0);
        };
        let Some(coordinator) = context.team.clone() else {
            return TaskResult::error(SKILLS_UNAVAILABLE, 0);
        };
        match coordinator.skills().load(&name).await {
            Ok(skill) => text(format!("{SKILL_INSTRUCTIONS_HEADER}\n\n{}", skill.body)),
            Err(message) => TaskResult::error(message, 0),
        }
    })
}

pub(super) fn propose_skill(
    context: ToolExecutionContext,
    agent: AgentState,
    _user_message: Message,
    call: ToolCall,
) -> BoxFuture<'static, TaskResult<Content>> {
    Box::pin(async move {
        // Spec §8.3: helpers cannot use propose_skill, even if a config
        // somehow carries it.
        if is_helper_config(&agent.config) {
            return TaskResult::error(HELPERS_CANNOT_PROPOSE_SKILLS, 0);
        }
        let (Some(name), Some(description), Some(body)) = (
            text_arg(&call, "name"),
            text_arg(&call, "description"),
            text_arg(&call, "body"),
        ) else {
            return TaskResult::error("propose_skill needs name, description, and body strings", 0);
        };
        let (Some(coordinator), Some(link)) = (context.team.clone(), context.run_link.clone())
        else {
            return TaskResult::error(SKILLS_UNAVAILABLE, 0);
        };
        let proposal = Proposal {
            by: ProposedBy {
                agent_id: agent.id.clone(),
                session_id: link.session_id,
                run_id: link.run_id,
            },
            name: name.to_string(),
            description: description.to_string(),
            body: body.to_string(),
            slug: text_arg(&call, "slug").map(str::to_string),
        };
        match coordinator.skills().propose(proposal).await {
            Ok(draft) => text(proposed_reply(&draft)),
            Err(SkillError::Unavailable(_)) => TaskResult::error(PROPOSAL_NOT_SAVED, 0),
            Err(error) => TaskResult::error(error.message(), 0),
        }
    })
}
