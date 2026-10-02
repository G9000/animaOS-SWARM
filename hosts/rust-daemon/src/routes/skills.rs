//! Skills (spec §8.4): the owner's skills, and (M5 Task 7) their drafts and
//! imports. Every route requires the local owner, answers
//! `Cache-Control: no-store`, and answers 409 without a configured workspace.

use axum::extract::{Path, Request, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::approvals::STATUS_INVALID;
use super::contracts::{
    ApprovedSkillDraftEnvelope, ErrorBody, SkillDeleteResponse, SkillDetailEnvelope,
    SkillDraftEnvelope, SkillDraftResponse, SkillDraftsEnvelope, SkillEnvelope, SkillFileResponse,
    SkillResponse, SkillsEnvelope,
};
use super::http::{json_response, read_limited_body, request_query};
use super::jobs::{authorize, no_store};
use super::multipart::{
    parse_form_with, FormLimits, FORM_TOO_LARGE, MAX_FORM_HEADER_BYTES, MAX_FORM_PARTS,
};
use super::sessions::rejected;
use super::{parse_json_body, ApiError, AppState};
use crate::skills::{
    DraftApproval, SkillContent, SkillError, IMPORT_NOT_MULTIPART, IMPORT_TOO_LARGE,
    MAX_SKILL_IMPORT_BYTES, SKILLS_NEED_WORKSPACE, SKILL_SLUG_INVALID,
};

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

/// An imported file plus its multipart framing.
const MAX_IMPORT_REQUEST_BYTES: usize = MAX_SKILL_IMPORT_BYTES + 16 * 1024;

/// The import form: a `file` and a `slug`, with room for a few more fields.
const IMPORT_FORM_LIMITS: FormLimits = FormLimits {
    max_total_bytes: MAX_IMPORT_REQUEST_BYTES,
    max_parts: MAX_FORM_PARTS,
    max_header_bytes: MAX_FORM_HEADER_BYTES,
    max_part_bytes: MAX_SKILL_IMPORT_BYTES,
};

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct DraftApprovalRequest {
    /// The owner's edit of the body; the draft's body when absent.
    #[serde(default)]
    body: Option<String>,
    /// Required for a file draft: the `fileHash` the owner reviewed.
    #[serde(default)]
    hash: Option<String>,
}

#[utoipa::path(get, path = "/api/skill-drafts", tag = "skills",
    params(("status" = Option<String>, Query, description = "`pending` (the default): stored drafts and SKILL.md files without a record, oldest first, after a rescan. `decided`: approved and rejected drafts of the last 30 days, newest first")),
    responses(
        (status = 200, description = "The drafts", body = SkillDraftsEnvelope),
        (status = 400, description = "An invalid status", body = ErrorBody),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 409, description = "No workspace is configured", body = ErrorBody)
    ))]
pub(super) async fn list_skill_drafts(State(state): State<AppState>, request: Request) -> Response {
    if let Err(response) = authorize(&state, &request, true) {
        return response;
    }
    let decided = match request_query(request.uri())
        .ok()
        .and_then(|params| params.get("status").cloned())
        .as_deref()
    {
        None | Some("") | Some("pending") => false,
        Some("decided") => true,
        Some(_) => return rejected(ApiError::bad_request_static(STATUS_INVALID)),
    };
    match state.agent_runs.skills().drafts(decided).await {
        Ok(views) => answer(
            StatusCode::OK,
            &SkillDraftsEnvelope {
                drafts: views.iter().map(SkillDraftResponse::from).collect(),
            },
        ),
        Err(error) => skill_error(error),
    }
}

#[utoipa::path(post, path = "/api/skill-drafts/{draft_id}/approve", tag = "skills",
    params(("draft_id" = String, Path, description = "`skd_<uuid>` or `file:<slug>`, percent-encoded")),
    request_body = DraftApprovalRequest,
    responses(
        (status = 200, description = "SKILL.md written (or, for an unedited file draft, kept) and its hash pinned", body = ApprovedSkillDraftEnvelope),
        (status = 400, description = "An invalid body, a file draft without its reviewed hash, or a file that is not a valid SKILL.md", body = ErrorBody),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "No such draft", body = ErrorBody),
        (status = 409, description = "No workspace is configured, the draft was already decided, the file changed since it was reviewed, or the workspace already has 200 skills", body = ErrorBody),
        (status = 503, description = "The file or the registry could not be saved; the draft stays pending", body = ErrorBody)
    ))]
pub(super) async fn approve_skill_draft(
    State(state): State<AppState>,
    Path(draft_id): Path<String>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, false) {
        return response;
    }
    let limit = state.config.max_request_bytes.max(MAX_SKILL_REQUEST_BYTES);
    let bytes = match read_limited_body(request, limit).await {
        Ok(bytes) => bytes,
        Err(response) => return no_store(response),
    };
    let input: DraftApprovalRequest = if bytes.is_empty() {
        DraftApprovalRequest::default()
    } else {
        match parse_json_body(bytes) {
            Ok(input) => input,
            Err(error) => return rejected(error),
        }
    };
    let approval = DraftApproval {
        body: input.body,
        hash: input.hash,
    };
    match state
        .agent_runs
        .skills()
        .approve_draft(&draft_id, approval)
        .await
    {
        Ok(approved) => answer(
            StatusCode::OK,
            &ApprovedSkillDraftEnvelope {
                draft: SkillDraftResponse::of(
                    &approved.draft,
                    Some(approved.skill.approved_hash.clone()),
                ),
                skill: SkillResponse::from(&approved.skill),
            },
        ),
        Err(error) => skill_error(error),
    }
}

#[utoipa::path(post, path = "/api/skill-drafts/{draft_id}/reject", tag = "skills",
    params(("draft_id" = String, Path, description = "`skd_<uuid>` or `file:<slug>`, percent-encoded")),
    responses(
        (status = 200, description = "The draft, rejected; a file draft stays hidden until its file changes, and the file stays", body = SkillDraftEnvelope),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 404, description = "No such draft", body = ErrorBody),
        (status = 409, description = "No workspace is configured, or the draft was already decided", body = ErrorBody),
        (status = 503, description = "The rejection could not be saved", body = ErrorBody)
    ))]
pub(super) async fn reject_skill_draft(
    State(state): State<AppState>,
    Path(draft_id): Path<String>,
    request: Request,
) -> Response {
    if let Err(response) = authorize(&state, &request, false) {
        return response;
    }
    match state.agent_runs.skills().reject_draft(&draft_id).await {
        Ok(draft) => answer(
            StatusCode::OK,
            &SkillDraftEnvelope {
                draft: SkillDraftResponse::of(&draft, None),
            },
        ),
        Err(error) => skill_error(error),
    }
}

#[utoipa::path(post, path = "/api/skills/import", tag = "skills",
    request_body(content = String, content_type = "multipart/form-data", description = "A `file` part holding a SKILL.md (at most 64 KiB) and an optional `slug` field"),
    responses(
        (status = 201, description = "A pending import draft", body = SkillDraftEnvelope),
        (status = 400, description = "Not multipart with a file part, a file over 64 KiB, an invalid slug, or a file that is not a valid SKILL.md", body = ErrorBody),
        (status = 403, description = "Local owner required", body = ErrorBody),
        (status = 409, description = "No workspace is configured, or 10 imports already wait for review", body = ErrorBody),
        (status = 503, description = "The draft could not be saved", body = ErrorBody)
    ))]
pub(super) async fn import_skill(State(state): State<AppState>, request: Request) -> Response {
    if let Err(response) = authorize(&state, &request, false) {
        return response;
    }
    let content_type = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    // Any read failure here is almost always the size bound.
    let Ok(body) = read_limited_body(request, MAX_IMPORT_REQUEST_BYTES).await else {
        return rejected(ApiError::bad_request_static(IMPORT_TOO_LARGE));
    };
    let parts = match parse_form_with(&content_type, &body, &IMPORT_FORM_LIMITS) {
        Ok(parts) => parts,
        Err(FORM_TOO_LARGE) => return rejected(ApiError::bad_request_static(IMPORT_TOO_LARGE)),
        Err(_) => return rejected(ApiError::bad_request_static(IMPORT_NOT_MULTIPART)),
    };
    let Some(file) = parts.iter().find(|part| part.name == "file") else {
        return rejected(ApiError::bad_request_static(IMPORT_NOT_MULTIPART));
    };
    let slug = match parts.iter().find(|part| part.name == "slug") {
        None => None,
        Some(part) => match String::from_utf8(part.bytes.clone()) {
            Ok(slug) => Some(slug),
            Err(_) => return skill_error(SkillError::Invalid(SKILL_SLUG_INVALID.to_string())),
        },
    };
    match state
        .agent_runs
        .skills()
        .import(file.bytes.clone(), slug)
        .await
    {
        Ok(draft) => answer(
            StatusCode::CREATED,
            &SkillDraftEnvelope {
                draft: SkillDraftResponse::of(&draft, draft.base_hash.clone()),
            },
        ),
        Err(error) => skill_error(error),
    }
}
