//! Skill bodies (spec §8.4).

use serde::Serialize;
use utoipa::ToSchema;

use crate::skills::{ScannedFile, SkillRecord};

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
