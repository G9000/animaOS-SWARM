//! Skills (spec §8.4): the owner's skills, and (M5 Task 7) their drafts and
//! imports. Every route requires the local owner, answers
//! `Cache-Control: no-store`, and answers 409 without a configured workspace.

use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::contracts::{
    ErrorBody, SkillDeleteResponse, SkillDetailEnvelope, SkillEnvelope, SkillFileResponse,
    SkillResponse, SkillsEnvelope,
};
use super::http::{json_response, read_limited_body};
use super::jobs::{authorize, no_store};
use super::sessions::rejected;
use super::{parse_json_body, ApiError, AppState};
use crate::skills::{SkillContent, SkillError, SKILLS_NEED_WORKSPACE};

/// Bodies the skill routes read, or the daemon-wide limit when larger: a
/// 32 KiB body can take six bytes a character once JSON-escaped, and the
/// body check must answer, not the body limit (as M3's run route).
pub(super) const MAX_SKILL_REQUEST_BYTES: usize = 256 * 1024;

pub(super) fn skill_error(error: SkillError) -> Response {
    rejected(match error {
        SkillError::NoWorkspace => ApiError::conflict(SKILLS_NEED_WORKSPACE),
        SkillError::NotFound => ApiError::not_found(),
        SkillError::Invalid(message) => ApiError::bad_request(message),
        SkillError::Conflict(message) => ApiError::conflict(message),
        SkillError::Unavailable(message) => ApiError::service_unavailable(message),
    })
}

pub(super) async fn skill_body<T: DeserializeOwned>(
    state: &AppState,
    request: Request,
) -> Result<T, Response> {
    let limit = state.config.max_request_bytes.max(MAX_SKILL_REQUEST_BYTES);
    let bytes = read_limited_body(request, limit).await.map_err(no_store)?;
    parse_json_body(bytes).map_err(|error: ApiError| no_store(error.into_response()))
}

pub(super) fn answer<T: Serialize>(status: StatusCode, body: &T) -> Response {
    no_store(json_response(status, body))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SkillContentRequest {
    /// 1–64 characters on one line.
    name: String,
    /// 1–300 characters on one line.
    description: String,
    /// The Markdown body, 1 byte to 32 KiB.
    body: String,
    /// Absent: a new skill starts on, an existing one keeps its switch.
    #[serde(default)]
    enabled: Option<bool>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct SkillEnabledRequest {
    enabled: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct SkillApprovalRequest {
    /// The `file.hash` the owner reviewed.
    hash: String,
}

#[utoipa::path(get, path = "/api/skills", tag = "skills",
    responses(
        (status = 200, description = "Every skill by slug, after a rescan of the skills folder", body = SkillsEnvelope),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 409, description = "No workspace is configured", body = ErrorBody)
    ))]
pub(super) async fn list_skills(State(state): State<AppState>, request: Request) -> Response {
    if let Err(response) = authorize(&state, &request, true) {
        return response;
    }
    match state.agent_runs.skills().list().await {
        Ok(skills) => answer(
            StatusCode::OK,
            &SkillsEnvelope {
                skills: skills.iter().map(SkillResponse::from).collect(),
            },
        ),
        Err(error) => skill_error(error),
    }
}

#[utoipa::path(get, path = "/api/skills/{slug}", tag = "skills",
    params(("slug" = String, Path)),
    responses(
        (status = 200, description = "The record (or null) and what SKILL.md holds now (or null)", body = SkillDetailEnvelope),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "Neither a record nor a SKILL.md", body = ErrorBody),
        (status = 409, description = "No workspace is configured", body = ErrorBody),
        (status = 503, description = "The file could not be read in time", body = ErrorBody)
    ))]
pub(super) async fn get_skill(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, true) {
        return response;
    }
    match state.agent_runs.skills().detail(&slug).await {
        Ok(detail) => answer(
            StatusCode::OK,
            &SkillDetailEnvelope {
                skill: detail.record.as_ref().map(SkillResponse::from),
                file: detail.file.as_ref().map(SkillFileResponse::from),
            },
        ),
        Err(error) => skill_error(error),
    }
}

#[utoipa::path(put, path = "/api/skills/{slug}", tag = "skills",
    params(("slug" = String, Path)),
    request_body = SkillContentRequest,
    responses(
        (status = 201, description = "A new skill, written and approved", body = SkillEnvelope),
        (status = 200, description = "The skill's content replaced and approved", body = SkillEnvelope),
        (status = 400, description = "An invalid slug, name, description, or body", body = ErrorBody),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 409, description = "No workspace is configured, the workspace already has 200 skills, or the folder holds a SKILL.md the owner has not reviewed", body = ErrorBody),
        (status = 503, description = "The file or the registry could not be saved; a file already written then reads changed", body = ErrorBody)
    ))]
pub(super) async fn put_skill(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, false) {
        return response;
    }
    let input: SkillContentRequest = match skill_body(&state, request).await {
        Ok(input) => input,
        Err(response) => return response,
    };
    let content = SkillContent {
        name: input.name,
        description: input.description,
        body: input.body,
        enabled: input.enabled,
    };
    match state.agent_runs.skills().save(&slug, content).await {
        Ok((record, created)) => answer(
            if created {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            },
            &SkillEnvelope {
                skill: SkillResponse::from(&record),
            },
        ),
        Err(error) => skill_error(error),
    }
}

#[utoipa::path(patch, path = "/api/skills/{slug}", tag = "skills",
    params(("slug" = String, Path)),
    request_body = SkillEnabledRequest,
    responses(
        (status = 200, description = "The skill, turned on or off", body = SkillEnvelope),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "No such skill", body = ErrorBody),
        (status = 409, description = "No workspace is configured", body = ErrorBody),
        (status = 503, description = "The change could not be saved; the switch stays", body = ErrorBody)
    ))]
pub(super) async fn patch_skill(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, false) {
        return response;
    }
    let input: SkillEnabledRequest = match skill_body(&state, request).await {
        Ok(input) => input,
        Err(response) => return response,
    };
    match state
        .agent_runs
        .skills()
        .set_enabled(&slug, input.enabled)
        .await
    {
        Ok(record) => answer(
            StatusCode::OK,
            &SkillEnvelope {
                skill: SkillResponse::from(&record),
            },
        ),
        Err(error) => skill_error(error),
    }
}

#[utoipa::path(delete, path = "/api/skills/{slug}", tag = "skills",
    params(("slug" = String, Path)),
    responses(
        (status = 200, description = "The record is gone and the folder is in .anima-trash/skills", body = SkillDeleteResponse),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "Neither a record nor a folder", body = ErrorBody),
        (status = 409, description = "No workspace is configured", body = ErrorBody),
        (status = 503, description = "The folder could not be moved or the change saved; the skill stays", body = ErrorBody)
    ))]
pub(super) async fn delete_skill(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, false) {
        return response;
    }
    match state.agent_runs.skills().delete(&slug).await {
        Ok(trash_path) => answer(
            StatusCode::OK,
            &SkillDeleteResponse {
                deleted: true,
                trash_path,
            },
        ),
        Err(error) => skill_error(error),
    }
}

#[utoipa::path(post, path = "/api/skills/{slug}/approve", tag = "skills",
    params(("slug" = String, Path)),
    request_body = SkillApprovalRequest,
    responses(
        (status = 200, description = "The changed SKILL.md, approved at the reviewed hash", body = SkillEnvelope),
        (status = 400, description = "The file is not a valid SKILL.md, or hash is not 64 hexadecimal characters (hash is required: the SKILL.md you reviewed)", body = ErrorBody),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "No such skill or file", body = ErrorBody),
        (status = 409, description = "No workspace is configured, the file changed since it was reviewed, or it has no unapproved changes", body = ErrorBody),
        (status = 503, description = "The file could not be read or the approval saved", body = ErrorBody)
    ))]
pub(super) async fn approve_skill(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, false) {
        return response;
    }
    let input: SkillApprovalRequest = match skill_body(&state, request).await {
        Ok(input) => input,
        Err(response) => return response,
    };
    match state
        .agent_runs
        .skills()
        .approve_changed(&slug, &input.hash)
        .await
    {
        Ok(record) => answer(
            StatusCode::OK,
            &SkillEnvelope {
                skill: SkillResponse::from(&record),
            },
        ),
        Err(error) => skill_error(error),
    }
}
