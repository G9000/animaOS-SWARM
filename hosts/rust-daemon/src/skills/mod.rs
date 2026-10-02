//! Owner-approved skills (spec §8): `SKILL.md` files under
//! `<workspace>/skills/<slug>/`, the control-plane registry that pins each
//! one to the hash the owner approved, the drafts waiting for the owner, and
//! what a run sees of them. Later M5 tasks add `registry`, `disk`,
//! `service`, `scanner`, `drafts`, and `runtime`.
#![allow(dead_code)] // M5 Task 9 removes this once the routes, tools, and runs use every item.

pub(crate) mod file;

use serde::{Deserialize, Serialize};

#[allow(unused_imports)] // M5 Tasks 2–9 use them.
pub(crate) use file::{compose_skill_file, parse_skill_file, skill_hash, SkillFile};

/// A `SKILL.md`'s Markdown body, after its front matter (spec §8.1, §16).
pub(crate) const MAX_SKILL_BODY_BYTES: usize = 32 * 1024;
/// Skills one run's system prompt lists (spec §8.3, §16).
pub(crate) const MAX_INDEXED_SKILLS: usize = 50;
/// Drafts one agent may have waiting for the owner (spec §8.2, §16).
pub(crate) const MAX_PENDING_DRAFTS_PER_AGENT: usize = 10;
/// Front-matter `name`, in characters (spec §8.1).
pub(crate) const MAX_SKILL_NAME_CHARS: usize = 64;
/// Front-matter `description`, in characters (spec §8.1).
pub(crate) const MAX_SKILL_DESCRIPTION_CHARS: usize = 300;
/// A slug's length (spec §8.1: `^[a-z0-9][a-z0-9-]{0,63}$`).
pub(crate) const MAX_SKILL_SLUG_CHARS: usize = 64;
/// The front matter between the `---` lines (plan bound; spec §1 bounded growth).
pub(crate) const MAX_SKILL_FRONT_MATTER_BYTES: usize = 4 * 1024;
/// A whole `SKILL.md`: front matter, delimiters, and body. Larger files are
/// never read past this.
pub(crate) const MAX_SKILL_FILE_BYTES: usize =
    MAX_SKILL_BODY_BYTES + MAX_SKILL_FRONT_MATTER_BYTES + 16;
/// Skill records in one workspace (plan bound).
pub(crate) const MAX_SKILLS: usize = 200;
/// Skill folders one scan reads, by name (plan bound).
pub(crate) const MAX_SCANNED_SKILL_FOLDERS: usize = 200;
/// Imported drafts waiting for the owner (plan bound).
pub(crate) const MAX_PENDING_IMPORT_DRAFTS: usize = 10;
/// Decided drafts kept, newest first (plan bound).
pub(crate) const MAX_DECIDED_DRAFTS: usize = 50;
/// How long a decided draft is kept (spec §8.2: rejected drafts, 30 days).
pub(crate) const DECIDED_DRAFT_RETENTION_MS: u64 = 30 * 24 * 60 * 60 * 1000;
/// The background rescan's period (spec §8.1).
pub(crate) const SKILL_SCAN_INTERVAL_MS: u64 = 60_000;
/// One read, write, scan, or move of skill files (plan bound: every long
/// wait is bounded).
pub(crate) const SKILL_IO_TIMEOUT_MS: u64 = 10_000;
/// An imported `SKILL.md` (plan bound).
pub(crate) const MAX_SKILL_IMPORT_BYTES: usize = 64 * 1024;

/// The workspace folder that holds one folder per skill.
pub(crate) const SKILLS_FOLDER: &str = "skills";
pub(crate) const SKILL_FILE_NAME: &str = "SKILL.md";
/// Where a deleted skill's folder goes (spec §8.2), workspace-relative.
pub(crate) const SKILLS_TRASH_FOLDER: &str = ".anima-trash/skills";
/// A file draft's id is this plus its slug; it is never stored.
pub(crate) const FILE_DRAFT_ID_PREFIX: &str = "file:";
/// A stored draft's id is this plus a v4 UUID.
pub(crate) const DRAFT_ID_PREFIX: &str = "skd_";
/// The user message metadata of a `/skill` message (spec §3.3, §8.3).
pub(crate) const SKILL_METADATA_KEY: &str = "skill";
/// Slugs a route segment or Windows already uses: `POST /api/skills/import`
/// and the device names (Windows tools, Explorer, OneDrive, and git cannot
/// open or delete such a folder; the list applies on every platform because
/// workspaces are portable).
pub(crate) const RESERVED_SKILL_SLUGS: &[&str] = &[
    "import", "con", "prn", "aux", "nul", "com0", "com1", "com2", "com3", "com4", "com5", "com6",
    "com7", "com8", "com9", "lpt0", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8",
    "lpt9",
];

pub(crate) const SKILL_SLUG_INVALID: &str = "slug must be 1–64 lowercase letters, digits, or hyphens, starting with a letter or digit, and not a reserved name (import, con, nul, …)";
pub(crate) const SKILL_TEXT_HIDDEN: &str =
    "Skill text must not contain invisible tag or direction-override characters";
pub(crate) const SKILL_NAME_INVALID: &str = "name must be 1–64 characters on one line";
pub(crate) const SKILL_DESCRIPTION_INVALID: &str =
    "description must be 1–300 characters on one line";
pub(crate) const SKILL_BODY_EMPTY: &str = "body must not be empty";
pub(crate) const SKILL_BODY_TOO_LARGE: &str = "body must be at most 32 KiB";
pub(crate) const SKILL_HASH_REQUIRED: &str = "hash is required: the SKILL.md you reviewed";

pub(crate) const SKILL_FILE_NOT_UTF8: &str = "SKILL.md is not UTF-8 text";
pub(crate) const SKILL_FILE_NO_FRONT_MATTER: &str =
    "SKILL.md must start with front matter between --- lines";
pub(crate) const SKILL_FILE_FRONT_MATTER_TOO_LARGE: &str =
    "SKILL.md front matter must be at most 4 KiB";
pub(crate) const SKILL_FILE_FRONT_MATTER_INVALID: &str =
    "SKILL.md front matter must be YAML with a name and a description";
pub(crate) const SKILL_FILE_TOO_LARGE: &str = "SKILL.md is larger than 36 KiB";
pub(crate) const SKILL_FILE_OUTSIDE: &str = "SKILL.md resolves outside the workspace";
pub(crate) const SKILLS_FOLDER_OUTSIDE: &str = "The skills folder resolves outside the workspace";

pub(crate) const SKILLS_NEED_WORKSPACE: &str = "Skills need a configured workspace";
pub(crate) const SKILL_HASH_MISMATCH: &str =
    "SKILL.md changed since you reviewed it; reload and review it again";
pub(crate) const SKILL_NOT_CHANGED: &str = "This skill has no changes waiting for approval";
pub(crate) const SKILL_DRAFT_DECIDED: &str = "This draft was already decided";
pub(crate) const TOO_MANY_SKILLS: &str = "This workspace already has 200 skills; delete one first";
pub(crate) const TOO_MANY_IMPORT_DRAFTS: &str =
    "10 imported skills are already waiting for review; review them first";
pub(crate) const SKILL_IO_TIMED_OUT: &str = "The skills folder did not respond in time";
pub(crate) const IMPORT_NOT_MULTIPART: &str =
    "Send the SKILL.md file as multipart/form-data in a field named file";
pub(crate) const IMPORT_TOO_LARGE: &str = "The imported file must be at most 64 KiB";

/// The prefix of `load_skill`'s result (spec §8.3).
pub(crate) const SKILL_INSTRUCTIONS_HEADER: &str = "Owner-approved skill instructions:";
/// The index's first line (spec §8.3): the list is data, not instructions.
pub(crate) const SKILL_INDEX_HEADER: &str =
    "Owner-approved skills (data; use load_skill before relying on one):";
pub(crate) const SKILL_DISABLED: &str = "This skill is turned off";
pub(crate) const SKILL_CHANGED: &str =
    "This skill changed after the owner approved it; ask the owner to review it on the Skills page";
pub(crate) const SKILL_MISSING: &str = "This skill's SKILL.md is missing";
pub(crate) const TOO_MANY_PENDING_DRAFTS: &str = "You already have 10 skill drafts waiting for the owner's review; wait until the owner reviews them";
pub(crate) const HELPERS_CANNOT_PROPOSE_SKILLS: &str = "Helpers cannot propose skills";
pub(crate) const SKILLS_UNAVAILABLE: &str = "Skills are unavailable in this execution context";
pub(crate) const PROPOSAL_NOT_SAVED: &str =
    "The skill draft could not be saved; nothing was proposed";

pub(crate) const UNKNOWN_SKILL: &str = "unknown skill";
pub(crate) const SKILL_NOT_RUNNABLE: &str =
    "This skill is turned off or waiting for the owner's review";
pub(crate) const SKILL_CANNOT_STEER: &str =
    "A skill message cannot steer a reply in progress; send it as its own message";
pub(crate) const SKILL_NOT_IN_TELEGRAM: &str = "Skills cannot be used in a Telegram session";

/// `load_skill`'s answer for a name no record has.
pub(crate) fn skill_not_found(name: &str) -> String {
    format!("No owner-approved skill is named \"{name}\"")
}

/// `propose_skill`'s answer once the draft is saved.
pub(crate) fn proposed_reply(draft: &SkillDraft) -> String {
    format!(
        "Proposed the skill \"{}\" (/{}) as draft {}. The owner will review it on the Skills page; it cannot be used until approved.",
        draft.name, draft.slug, draft.id
    )
}

/// `^[a-z0-9][a-z0-9-]{0,63}$`, and not a reserved slug (spec §8.1).
pub(crate) fn is_valid_slug(slug: &str) -> bool {
    let bytes = slug.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_SKILL_SLUG_CHARS
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
        && !RESERVED_SKILL_SLUGS.contains(&slug)
}

/// A slug for `name`: ASCII letters and digits lowercased, every other run of
/// characters one hyphen, at most 64 characters; `None` when nothing is left
/// or the result is reserved.
pub(crate) fn slugify(name: &str) -> Option<String> {
    let mut slug = String::new();
    let mut after_hyphen = false;
    for character in name.chars().flat_map(char::to_lowercase) {
        if slug.len() >= MAX_SKILL_SLUG_CHARS {
            break;
        }
        if character.is_ascii_alphanumeric() {
            slug.push(character);
            after_hyphen = false;
        } else if !slug.is_empty() && !after_hyphen {
            slug.push('-');
            after_hyphen = true;
        }
    }
    let slug = slug.trim_end_matches('-').to_string();
    is_valid_slug(&slug).then_some(slug)
}

/// Unicode tag characters, bidirectional embeddings, overrides, and
/// isolates, and the supplementary variation selectors: invisible text that
/// can smuggle instructions past the owner.
fn is_smuggling_character(character: char) -> bool {
    matches!(
        character as u32,
        0xE0000..=0xE007F | 0x202A..=0x202E | 0x2066..=0x2069 | 0xE0100..=0xE01EF
    )
}

/// What a one-line field refuses on top of control characters: the
/// smuggling characters, line and paragraph separators, and every Unicode
/// `Cf` format character (spelled out; std has no general-category API).
fn is_hidden_in_one_line(character: char) -> bool {
    is_smuggling_character(character)
        || matches!(
            character as u32,
            0x2028
                | 0x2029
                | 0x00AD
                | 0x034F
                | 0x115F
                | 0x1160
                | 0x3164
                | 0xFFA0
                | 0x0890..=0x0891
                | 0x0600..=0x0605
                | 0x061C
                | 0x06DD
                | 0x070F
                | 0x08E2
                | 0x180E
                | 0x200B..=0x200F
                | 0x2060..=0x2064
                | 0x2066..=0x206F
                | 0xFEFF
                | 0xFFF9..=0xFFFB
                | 0x110BD
                | 0x110CD
                | 0x13430..=0x1343F
                | 0x1BCA0..=0x1BCA3
                | 0x1D173..=0x1D17A
                | 0xE0001
                | 0xE0020..=0xE007F
        )
}

/// Two or more variation selectors in a row: one after an emoji (❤️) is
/// fine, a run can carry hidden bytes.
fn has_variation_selector_run(value: &str) -> bool {
    let selector = |character: char| matches!(character as u32, 0xFE00..=0xFE0F);
    value
        .chars()
        .zip(value.chars().skip(1))
        .any(|(first, second)| selector(first) && selector(second))
}

/// `value` trimmed when it is 1 to `max_chars` characters with no control
/// or hidden character, so it can never span lines of the index.
fn one_line(value: &str, max_chars: usize, invalid: &'static str) -> Result<String, &'static str> {
    if value.chars().any(is_hidden_in_one_line) || has_variation_selector_run(value) {
        return Err(SKILL_TEXT_HIDDEN);
    }
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.chars().count() > max_chars
        || trimmed.chars().any(char::is_control)
    {
        return Err(invalid);
    }
    Ok(trimmed.to_string())
}

pub(crate) fn validate_name(name: &str) -> Result<String, &'static str> {
    one_line(name, MAX_SKILL_NAME_CHARS, SKILL_NAME_INVALID)
}

pub(crate) fn validate_description(description: &str) -> Result<String, &'static str> {
    one_line(
        description,
        MAX_SKILL_DESCRIPTION_CHARS,
        SKILL_DESCRIPTION_INVALID,
    )
}

/// A body keeps every format character except the smuggling ones (the
/// Skills page shows the rest as visible markers).
pub(crate) fn validate_body(body: &str) -> Result<(), &'static str> {
    if body.trim().is_empty() {
        Err(SKILL_BODY_EMPTY)
    } else if body.len() > MAX_SKILL_BODY_BYTES {
        Err(SKILL_BODY_TOO_LARGE)
    } else if body.chars().any(is_smuggling_character) {
        Err(SKILL_TEXT_HIDDEN)
    } else {
        Ok(())
    }
}

/// What the owner can rely on (spec §8.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SkillStatus {
    /// The file's hash is the approved one.
    Active,
    /// The file changed since it was approved; not loaded until approved again.
    Changed,
    /// There is no `SKILL.md`.
    Missing,
    /// The file cannot be read, is too large, resolves outside the
    /// workspace, or is not a valid `SKILL.md`.
    Invalid,
}

impl SkillStatus {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Changed => "changed",
            Self::Missing => "missing",
            Self::Invalid => "invalid",
        }
    }
}

/// A registered skill (spec §8.1), pinned to the hash the owner approved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SkillRecord {
    pub(crate) slug: String,
    /// The approved front matter's, never a changed file's.
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) enabled: bool,
    /// Lowercase hex SHA-256 of the approved `SKILL.md`.
    pub(crate) approved_hash: String,
    pub(crate) approved_at_ms: u64,
    pub(crate) updated_at_ms: u64,
    pub(crate) status: SkillStatus,
}

impl SkillRecord {
    /// Content the owner just approved: enabled and active.
    pub(crate) fn approved(slug: &str, file: &SkillFile, hash: String, now_ms: u64) -> Self {
        Self {
            slug: slug.to_string(),
            name: file.name.clone(),
            description: file.description.clone(),
            enabled: true,
            approved_hash: hash,
            approved_at_ms: now_ms,
            updated_at_ms: now_ms,
            status: SkillStatus::Active,
        }
    }
}

/// Where a draft came from (spec §8.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DraftSource {
    /// A companion's `propose_skill`.
    Agent,
    /// The owner's `POST /api/skills/import`.
    Import,
    /// A `SKILL.md` found without a record.
    File,
}

impl DraftSource {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Import => "import",
            Self::File => "file",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DraftStatus {
    Pending,
    Approved,
    Rejected,
}

impl DraftStatus {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        }
    }
}

/// The run that proposed a draft (spec §8.2 `proposedBy`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProposedBy {
    pub(crate) agent_id: String,
    pub(crate) session_id: String,
    pub(crate) run_id: String,
}

/// Content waiting for the owner, or decided (spec §8.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SkillDraft {
    pub(crate) id: String,
    pub(crate) slug: String,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) body: String,
    pub(crate) source: DraftSource,
    #[serde(default)]
    pub(crate) proposed_by: Option<ProposedBy>,
    /// The skill's approved hash when the draft was made (`None`: a new slug).
    #[serde(default)]
    pub(crate) base_hash: Option<String>,
    /// A file draft's `SKILL.md` hash (stored only on a rejected file draft).
    #[serde(default)]
    pub(crate) file_hash: Option<String>,
    pub(crate) created_at_ms: u64,
    pub(crate) status: DraftStatus,
    #[serde(default)]
    pub(crate) decided_at_ms: Option<u64>,
}

impl SkillDraft {
    pub(crate) fn new(
        slug: &str,
        file: SkillFile,
        source: DraftSource,
        proposed_by: Option<ProposedBy>,
        base_hash: Option<String>,
        now_ms: u64,
    ) -> Self {
        Self {
            id: format!("{DRAFT_ID_PREFIX}{}", uuid::Uuid::new_v4()),
            slug: slug.to_string(),
            name: file.name,
            description: file.description,
            body: file.body,
            source,
            proposed_by,
            base_hash,
            file_hash: None,
            created_at_ms: now_ms,
            status: DraftStatus::Pending,
            decided_at_ms: None,
        }
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.status == DraftStatus::Pending
    }

    pub(crate) fn decide(&mut self, status: DraftStatus, now_ms: u64) {
        self.status = status;
        self.decided_at_ms = Some(now_ms);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notes() -> SkillFile {
        SkillFile {
            name: "Notes".into(),
            description: "Take notes".into(),
            body: "Write them down.".into(),
        }
    }

    #[test]
    fn the_limits_are_the_specs() {
        assert_eq!(MAX_SKILL_BODY_BYTES, 32 * 1024);
        assert_eq!(MAX_INDEXED_SKILLS, 50);
        assert_eq!(MAX_PENDING_DRAFTS_PER_AGENT, 10);
        assert_eq!(MAX_SKILL_NAME_CHARS, 64);
        assert_eq!(MAX_SKILL_DESCRIPTION_CHARS, 300);
        assert_eq!(SKILL_SCAN_INTERVAL_MS, 60_000);
        assert_eq!(
            SKILL_SLUG_INVALID,
            "slug must be 1–64 lowercase letters, digits, or hyphens, starting with a letter or digit, and not a reserved name (import, con, nul, …)"
        );
        assert_eq!(
            SKILL_TEXT_HIDDEN,
            "Skill text must not contain invisible tag or direction-override characters"
        );
        assert_eq!(DECIDED_DRAFT_RETENTION_MS, 30 * 24 * 60 * 60 * 1000);
    }

    #[test]
    fn slugs_follow_the_pattern_and_skip_reserved_names() {
        let longest = "a".repeat(64);
        let too_long = "a".repeat(65);
        for valid in [
            "a",
            "notes",
            "weekly-review",
            "2026-plan",
            "com10",
            "console",
            "con-1",
            "nul2",
            longest.as_str(),
        ] {
            assert!(is_valid_slug(valid), "{valid}");
        }
        for invalid in [
            "",
            "-lead",
            "Upper",
            "under_score",
            "dot.ted",
            "sp ace",
            "../x",
            "import",
            "con",
            "prn",
            "aux",
            "nul",
            "com0",
            "com1",
            "com9",
            "lpt0",
            "lpt9",
            too_long.as_str(),
        ] {
            assert!(!is_valid_slug(invalid), "{invalid}");
        }
    }

    #[test]
    fn a_slug_is_derived_from_a_name() {
        assert_eq!(slugify("Weekly Review!").as_deref(), Some("weekly-review"));
        assert_eq!(slugify("--Plan  B--").as_deref(), Some("plan-b"));
        assert_eq!(slugify("!!!"), None);
        assert_eq!(slugify("Import"), None, "reserved");
        assert_eq!(slugify("Con"), None);
        assert_eq!(slugify("COM1"), None);
        assert_eq!(slugify("Nul."), None);
        assert_eq!(slugify("Console").as_deref(), Some("console"));
        assert_eq!(slugify(&"a".repeat(70)), Some("a".repeat(64)));
    }

    #[test]
    fn names_and_descriptions_are_one_trimmed_line() {
        assert_eq!(validate_name("  Notes "), Ok("Notes".to_string()));
        assert_eq!(validate_name(&"é".repeat(64)), Ok("é".repeat(64)));
        let too_long = "x".repeat(65);
        for invalid in ["", "   ", "two\nlines", "tab\there", too_long.as_str()] {
            assert_eq!(
                validate_name(invalid),
                Err(SKILL_NAME_INVALID),
                "{invalid:?}"
            );
        }
        assert!(validate_description(&"d".repeat(300)).is_ok());
        assert_eq!(
            validate_description(&"d".repeat(301)),
            Err(SKILL_DESCRIPTION_INVALID)
        );
        assert_eq!(
            validate_description("line\r\nbreak"),
            Err(SKILL_DESCRIPTION_INVALID)
        );
    }

    #[test]
    fn hidden_characters_are_refused_in_one_line_fields_and_smuggling_characters_in_bodies() {
        for hidden in [
            "a\u{E0041}b",
            "a\u{202E}b",
            "a\u{2066}b",
            "a\u{2028}b",
            "a\u{2029}b",
            "a\u{200B}b",
            "a\u{FEFF}b",
            "a\u{00AD}b",
            "ok \u{E0100}\u{E0101}",
            "a\u{FE0F}\u{FE0F}b",
            "a\u{034F}b",
            "a\u{3164}b",
            "a\u{0890}b",
        ] {
            assert_eq!(validate_name(hidden), Err(SKILL_TEXT_HIDDEN), "{hidden:?}");
            assert_eq!(
                validate_description(hidden),
                Err(SKILL_TEXT_HIDDEN),
                "{hidden:?}"
            );
        }
        assert_eq!(validate_name("\u{200B}"), Err(SKILL_TEXT_HIDDEN));
        for hidden in ["x\u{E0041}", "x\u{202E}", "x\u{E0100}"] {
            assert_eq!(validate_body(hidden), Err(SKILL_TEXT_HIDDEN), "{hidden:?}");
        }
        assert!(validate_name("Love \u{2764}\u{FE0F}").is_ok());
        for allowed in ["family 👨\u{200D}👩", "x\u{200B}", "x\u{2028}"] {
            assert_eq!(validate_body(allowed), Ok(()), "{allowed:?}");
        }
    }

    #[test]
    fn a_body_is_not_blank_and_at_most_32_kib() {
        assert_eq!(validate_body(" \n\t"), Err(SKILL_BODY_EMPTY));
        assert_eq!(validate_body(&"b".repeat(MAX_SKILL_BODY_BYTES)), Ok(()));
        assert_eq!(
            validate_body(&"b".repeat(MAX_SKILL_BODY_BYTES + 1)),
            Err(SKILL_BODY_TOO_LARGE)
        );
    }

    #[test]
    fn records_and_drafts_serialize_in_camel_case() {
        let record = SkillRecord::approved("notes", &notes(), "ab".repeat(32), 7);
        let value = serde_json::to_value(&record).unwrap();
        assert_eq!(value["approvedHash"], "ab".repeat(32));
        assert_eq!(value["status"], "active");
        assert_eq!(value["enabled"], true);

        let mut draft = SkillDraft::new(
            "notes",
            notes(),
            DraftSource::Agent,
            Some(ProposedBy {
                agent_id: "agent-1".into(),
                session_id: "chat:1".into(),
                run_id: "run_1".into(),
            }),
            None,
            9,
        );
        assert!(draft.id.starts_with(DRAFT_ID_PREFIX));
        assert!(draft.is_pending());
        draft.decide(DraftStatus::Rejected, 12);
        let value = serde_json::to_value(&draft).unwrap();
        assert_eq!(value["proposedBy"]["runId"], "run_1");
        assert_eq!(value["status"], "rejected");
        assert_eq!(value["decidedAtMs"], 12);
        assert_eq!(value["source"], "agent");
        let back: SkillDraft = serde_json::from_value(value).unwrap();
        assert_eq!(back, draft);
    }

    #[test]
    fn the_tool_answers_name_the_draft_and_the_missing_skill() {
        let draft = SkillDraft::new("notes", notes(), DraftSource::Agent, None, None, 1);
        assert_eq!(
            proposed_reply(&draft),
            format!(
                "Proposed the skill \"Notes\" (/notes) as draft {}. The owner will review it on the Skills page; it cannot be used until approved.",
                draft.id
            )
        );
        assert_eq!(
            skill_not_found("ghost"),
            "No owner-approved skill is named \"ghost\""
        );
    }
}
