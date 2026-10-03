//! Skill bodies (spec §8.4).

use serde::Serialize;
use utoipa::ToSchema;

use crate::skills::{DraftSource, DraftView, ScannedFile, SkillDraft, SkillRecord};

/// A registered skill (spec §8.1).
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SkillResponse {
    pub(crate) slug: String,
    /// The approved front matter's name, never a changed file's.
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) enabled: bool,
    /// `active`, `changed`, `missing`, or `invalid`.
    pub(crate) status: String,
    /// Lowercase hex SHA-256 of the approved `SKILL.md`.
    pub(crate) approved_hash: String,
    pub(crate) approved_at_ms: u64,
    pub(crate) updated_at_ms: u64,
}

impl From<&SkillRecord> for SkillResponse {
    fn from(record: &SkillRecord) -> Self {
        Self {
            slug: record.slug.clone(),
            name: record.name.clone(),
            description: record.description.clone(),
            enabled: record.enabled,
            status: record.status.as_str().into(),
            approved_hash: record.approved_hash.clone(),
            approved_at_ms: record.approved_at_ms,
            updated_at_ms: record.updated_at_ms,
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct SkillsEnvelope {
    pub(crate) skills: Vec<SkillResponse>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct SkillEnvelope {
    pub(crate) skill: SkillResponse,
}

/// What a `SKILL.md` holds now. Not approved content: clients show it as
/// text only.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SkillFileResponse {
    /// The file's hash; send it back to approve exactly this content.
    /// `null` when the file could not be read whole.
    pub(crate) hash: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) body: Option<String>,
    /// Why the file is not a valid `SKILL.md`.
    pub(crate) problem: Option<String>,
}

impl From<&ScannedFile> for SkillFileResponse {
    fn from(file: &ScannedFile) -> Self {
        let parsed = file.parsed.as_ref().ok();
        Self {
            hash: file.hash.clone(),
            name: parsed.map(|parsed| parsed.name.clone()),
            description: parsed.map(|parsed| parsed.description.clone()),
            body: parsed.map(|parsed| parsed.body.clone()),
            problem: file.problem().map(str::to_string),
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct SkillDetailEnvelope {
    pub(crate) skill: Option<SkillResponse>,
    pub(crate) file: Option<SkillFileResponse>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SkillDeleteResponse {
    pub(crate) deleted: bool,
    /// Where the folder went, workspace-relative; `null` when it had none.
    pub(crate) trash_path: Option<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProposedByResponse {
    pub(crate) agent_id: String,
    pub(crate) session_id: String,
    pub(crate) run_id: String,
}

/// A draft waiting for the owner, or decided (spec §8.2). Untrusted until
/// approved: clients show its text as text only.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SkillDraftResponse {
    /// `skd_<uuid>`, or `file:<slug>` for a `SKILL.md` without a record.
    pub(crate) id: String,
    pub(crate) slug: String,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) body: String,
    /// `agent`, `import`, or `file`.
    pub(crate) source: String,
    pub(crate) proposed_by: Option<ProposedByResponse>,
    /// The skill's approved hash when the draft was made; `null` for a new slug.
    pub(crate) base_hash: Option<String>,
    /// The skill's approved hash now; `null` when it has no record.
    pub(crate) current_hash: Option<String>,
    /// The skill was approved again since this draft was made.
    pub(crate) stale: bool,
    /// A file draft's `SKILL.md` hash: send it back as `hash` to approve it.
    pub(crate) file_hash: Option<String>,
    pub(crate) created_at_ms: u64,
    /// `pending`, `approved`, or `rejected`.
    pub(crate) status: String,
    pub(crate) decided_at_ms: Option<u64>,
    /// Why a file draft's `SKILL.md` is not valid.
    pub(crate) problem: Option<String>,
}

impl From<&DraftView> for SkillDraftResponse {
    fn from(view: &DraftView) -> Self {
        let draft = &view.draft;
        Self {
            id: draft.id.clone(),
            slug: draft.slug.clone(),
            name: draft.name.clone(),
            description: draft.description.clone(),
            body: draft.body.clone(),
            source: draft.source.as_str().into(),
            proposed_by: draft.proposed_by.as_ref().map(|by| ProposedByResponse {
                agent_id: by.agent_id.clone(),
                session_id: by.session_id.clone(),
                run_id: by.run_id.clone(),
            }),
            base_hash: draft.base_hash.clone(),
            current_hash: view.current_hash.clone(),
            stale: draft.is_pending()
                && draft.source != DraftSource::File
                && draft.base_hash != view.current_hash,
            file_hash: draft.file_hash.clone(),
            created_at_ms: draft.created_at_ms,
            status: draft.status.as_str().into(),
            decided_at_ms: draft.decided_at_ms,
            problem: view.problem.clone(),
        }
    }
}

impl SkillDraftResponse {
    /// A draft just changed by the owner or a tool.
    pub(crate) fn of(draft: &SkillDraft, current_hash: Option<String>) -> Self {
        Self::from(&DraftView {
            draft: draft.clone(),
            problem: None,
            current_hash,
        })
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct SkillDraftsEnvelope {
    pub(crate) drafts: Vec<SkillDraftResponse>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct SkillDraftEnvelope {
    pub(crate) draft: SkillDraftResponse,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct ApprovedSkillDraftEnvelope {
    pub(crate) skill: SkillResponse,
    pub(crate) draft: SkillDraftResponse,
}
