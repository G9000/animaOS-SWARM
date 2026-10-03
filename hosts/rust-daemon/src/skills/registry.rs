//! The control-plane skills registry (spec §8.1–§8.2): each skill's record
//! pinned to the `SKILL.md` hash the owner approved, the drafts waiting for
//! the owner and the decided ones (kept 30 days, at most 50), and what the
//! last scan of the skills folder found. Records and drafts are saved; the
//! scan is not. Nothing here touches the disk or awaits.
//!
//! Every record change, `set_scanned`, and every applied scan bumps
//! `generation`. A scan reads the generation before its file work and
//! `apply_scan` drops it when the generation moved meanwhile, so a scan can
//! never put back what a newer change or a newer scan replaced (two scans
//! that began together: only the first to finish applies).
//!
//! A change that records a file it read before the transaction checks
//! `scan_moved` first: when a scan applied since found other content, the
//! read may be the older one, and recording it would put back a stale state.

use std::collections::{BTreeMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use super::{
    is_valid_slug, parse_skill_file, skill_hash, DraftSource, DraftStatus, SkillDraft, SkillFile,
    SkillRecord, SkillStatus, DECIDED_DRAFT_RETENTION_MS, DRAFT_ID_PREFIX, FILE_DRAFT_ID_PREFIX,
    MAX_DECIDED_DRAFTS, MAX_INDEXED_SKILLS, MAX_SKILLS, TOO_MANY_SKILLS,
};

/// One `SKILL.md` as a scan found it. Never saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ScannedFile {
    /// With `len`, the cache key: a file whose modification time and size
    /// are unchanged is not read again by the next scan. Loading never uses
    /// the cache.
    pub(crate) modified: Option<SystemTime>,
    pub(crate) len: u64,
    /// `None` when the file could not be read whole.
    pub(crate) hash: Option<String>,
    pub(crate) parsed: Result<SkillFile, String>,
}

impl ScannedFile {
    /// A file read whole.
    pub(crate) fn read(bytes: &[u8], modified: Option<SystemTime>) -> Self {
        Self {
            modified,
            len: bytes.len() as u64,
            hash: Some(skill_hash(bytes)),
            parsed: parse_skill_file(bytes).map_err(str::to_string),
        }
    }

    /// A file that could not be read whole (too large, outside the
    /// workspace, or an I/O error).
    pub(crate) fn unreadable(
        problem: impl Into<String>,
        modified: Option<SystemTime>,
        len: u64,
    ) -> Self {
        Self {
            modified,
            len,
            hash: None,
            parsed: Err(problem.into()),
        }
    }

    /// What is wrong with the file, when it is not a valid `SKILL.md`.
    pub(crate) fn problem(&self) -> Option<&str> {
        self.parsed.as_ref().err().map(String::as_str)
    }
}

/// A record's status for what the scan found (spec §8.1).
pub(crate) fn status_for(record: &SkillRecord, scanned: Option<&ScannedFile>) -> SkillStatus {
    match scanned {
        None => SkillStatus::Missing,
        Some(file) if file.parsed.is_err() || file.hash.is_none() => SkillStatus::Invalid,
        Some(file) if file.hash.as_deref() == Some(record.approved_hash.as_str()) => {
            SkillStatus::Active
        }
        Some(_) => SkillStatus::Changed,
    }
}

/// A draft as the owner sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DraftView {
    pub(crate) draft: SkillDraft,
    /// What is wrong with a file draft's `SKILL.md`.
    pub(crate) problem: Option<String>,
    /// The skill's approved hash now; `None` when it has no record.
    pub(crate) current_hash: Option<String>,
}

/// The saved part of the registry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SkillSnapshot {
    pub(crate) skills: Vec<SkillRecord>,
    pub(crate) drafts: Vec<SkillDraft>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SkillRegistry {
    records: BTreeMap<String, SkillRecord>,
    drafts: Vec<SkillDraft>,
    scanned: BTreeMap<String, ScannedFile>,
    generation: u64,
    /// Whether a whole scan has been applied since the registry was made or
    /// last reset: until then, saved statuses stand.
    scanned_once: bool,
}

fn millis(time: Option<SystemTime>) -> u64 {
    time.and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// The draft a `SKILL.md` without a record shows as (spec §8.1).
fn file_draft(slug: &str, file: &ScannedFile) -> SkillDraft {
    let (name, description, body) = match &file.parsed {
        Ok(parsed) => (
            parsed.name.clone(),
            parsed.description.clone(),
            parsed.body.clone(),
        ),
        Err(_) => (slug.to_string(), String::new(), String::new()),
    };
    SkillDraft {
        id: format!("{FILE_DRAFT_ID_PREFIX}{slug}"),
        slug: slug.to_string(),
        name,
        description,
        body,
        source: DraftSource::File,
        proposed_by: None,
        base_hash: None,
        file_hash: file.hash.clone(),
        created_at_ms: millis(file.modified),
        status: DraftStatus::Pending,
        decided_at_ms: None,
    }
}

pub(super) fn is_hex_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl SkillRegistry {
    /// A record's status now: what the scan says once a scan (or a write of
    /// that slug) has been seen, otherwise the status as saved.
    fn status_now(&self, record: &SkillRecord) -> SkillStatus {
        if self.scanned_once || self.scanned.contains_key(&record.slug) {
            status_for(record, self.scanned.get(&record.slug))
        } else {
            record.status
        }
    }

    pub(crate) fn get(&self, slug: &str) -> Option<&SkillRecord> {
        self.records.get(slug)
    }

    /// Every record, by slug.
    pub(crate) fn records(&self) -> Vec<&SkillRecord> {
        self.records.values().collect()
    }

    /// A record by slug (with or without a leading `/`), or by name ignoring
    /// case.
    pub(crate) fn find(&self, name: &str) -> Option<&SkillRecord> {
        let wanted = name.trim().trim_start_matches('/');
        self.records.get(wanted).or_else(|| {
            let lowered = wanted.to_lowercase();
            self.records
                .values()
                .find(|record| record.name.to_lowercase() == lowered)
        })
    }

    /// What a run lists (spec §8.3): enabled `active` skills, by slug, at
    /// most `MAX_INDEXED_SKILLS`.
    pub(crate) fn index(&self) -> Vec<&SkillRecord> {
        self.records
            .values()
            .filter(|record| record.enabled && record.status == SkillStatus::Active)
            .take(MAX_INDEXED_SKILLS)
            .collect()
    }

    /// Whether `slug` can be sent with a message; `None` without a record.
    pub(crate) fn runnable(&self, slug: &str) -> Option<bool> {
        self.records
            .get(slug)
            .map(|record| record.enabled && record.status == SkillStatus::Active)
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Adds or replaces a record, its status recomputed from the last scan.
    /// Returns the record it replaced.
    pub(crate) fn put(
        &mut self,
        mut record: SkillRecord,
    ) -> Result<Option<SkillRecord>, &'static str> {
        if !self.records.contains_key(&record.slug) && self.records.len() >= MAX_SKILLS {
            return Err(TOO_MANY_SKILLS);
        }
        record.status = self.status_now(&record);
        self.generation += 1;
        Ok(self.records.insert(record.slug.clone(), record))
    }

    /// Puts back what `put` or `remove` replaced (a change whose save failed).
    pub(crate) fn restore(&mut self, slug: &str, previous: Option<SkillRecord>) {
        match previous {
            Some(mut record) => {
                record.status = self.status_now(&record);
                self.records.insert(slug.to_string(), record);
            }
            None => {
                self.records.remove(slug);
            }
        }
        self.generation += 1;
    }

    pub(crate) fn remove(&mut self, slug: &str) -> Option<SkillRecord> {
        self.generation += 1;
        self.records.remove(slug)
    }

    /// Records what `slug`'s `SKILL.md` holds now (after a write, a move, or
    /// a read that approved it) and recomputes its status.
    pub(crate) fn set_scanned(&mut self, slug: &str, file: Option<ScannedFile>) {
        match file {
            Some(file) => {
                self.scanned.insert(slug.to_string(), file);
            }
            None => {
                self.scanned.remove(slug);
            }
        }
        let scanned = self.scanned.get(slug);
        if let Some(record) = self.records.get_mut(slug) {
            record.status = status_for(record, scanned);
        }
        self.generation += 1;
    }

    /// Forgets the last scan (the workspace changed): a scan that began
    /// before this is dropped, and record statuses stay as they are until the
    /// next scan.
    pub(crate) fn reset_scan(&mut self) {
        self.scanned.clear();
        self.scanned_once = false;
        self.generation += 1;
    }

    pub(crate) fn scanned(&self, slug: &str) -> Option<&ScannedFile> {
        self.scanned.get(slug)
    }

    pub(crate) fn scanned_files(&self) -> &BTreeMap<String, ScannedFile> {
        &self.scanned
    }

    /// The hash the last scan holds for `slug`: `None` with no entry, and
    /// `Some(None)` for an entry that could not be read whole. Read before a
    /// file read made outside the transaction, for `scan_moved`.
    pub(crate) fn scanned_hash(&self, slug: &str) -> Option<Option<String>> {
        self.scanned.get(slug).map(|file| file.hash.clone())
    }

    /// Whether a scan applied since `seen` (`scanned_hash` before a read)
    /// found content other than `read_hash`: the read may be the older one,
    /// so it must not be recorded over it.
    pub(crate) fn scan_moved(
        &self,
        slug: &str,
        seen: &Option<Option<String>>,
        read_hash: &Option<String>,
    ) -> bool {
        let now = self.scanned_hash(slug);
        now != *seen && now.as_ref() != Some(read_hash)
    }

    /// Applies a whole scan that began at `seen_generation`. `None` when a
    /// change or another scan came in between (the scan is dropped);
    /// otherwise whether a status or the set of file drafts changed.
    pub(crate) fn apply_scan(
        &mut self,
        seen_generation: u64,
        scanned: BTreeMap<String, ScannedFile>,
    ) -> Option<bool> {
        if seen_generation != self.generation {
            return None;
        }
        let drafts_before = self.file_draft_keys();
        let mut changed = false;
        for record in self.records.values_mut() {
            let status = status_for(record, scanned.get(&record.slug));
            if record.status != status {
                record.status = status;
                changed = true;
            }
        }
        self.scanned = scanned;
        self.scanned_once = true;
        // A scan that began at the same generation is now the older one.
        self.generation += 1;
        Some(changed || self.file_draft_keys() != drafts_before)
    }

    fn file_draft_keys(&self) -> Vec<(String, Option<String>)> {
        self.file_drafts()
            .into_iter()
            .map(|view| (view.draft.slug, view.draft.file_hash))
            .collect()
    }

    /// The `SKILL.md` files without a record, as drafts (spec §8.1), except
    /// one the owner rejected at its current hash.
    pub(crate) fn file_drafts(&self) -> Vec<DraftView> {
        self.scanned
            .iter()
            .filter(|(slug, _)| !self.records.contains_key(*slug))
            .filter(|(slug, file)| {
                !self.drafts.iter().any(|draft| {
                    draft.source == DraftSource::File
                        && draft.status == DraftStatus::Rejected
                        && &draft.slug == *slug
                        && draft.file_hash == file.hash
                })
            })
            .map(|(slug, file)| DraftView {
                draft: file_draft(slug, file),
                problem: file.problem().map(str::to_string),
                current_hash: None,
            })
            .collect()
    }

    pub(crate) fn draft(&self, id: &str) -> Option<&SkillDraft> {
        self.drafts.iter().find(|draft| draft.id == id)
    }

    /// Stored drafts waiting for the owner, oldest first.
    pub(crate) fn pending_drafts(&self) -> Vec<&SkillDraft> {
        let mut pending: Vec<&SkillDraft> = self
            .drafts
            .iter()
            .filter(|draft| draft.is_pending())
            .collect();
        pending.sort_by(|left, right| {
            (left.created_at_ms, &left.id).cmp(&(right.created_at_ms, &right.id))
        });
        pending
    }

    /// Decided drafts, newest decision first.
    pub(crate) fn decided_drafts(&self) -> Vec<&SkillDraft> {
        let mut decided: Vec<&SkillDraft> = self
            .drafts
            .iter()
            .filter(|draft| !draft.is_pending())
            .collect();
        decided.sort_by(|left, right| {
            (right.decided_at_ms, &right.id).cmp(&(left.decided_at_ms, &left.id))
        });
        decided
    }

    /// `agent_id`'s proposals waiting for the owner (spec §8.2 cap).
    pub(crate) fn pending_from(&self, agent_id: &str) -> usize {
        self.drafts
            .iter()
            .filter(|draft| {
                draft.is_pending()
                    && draft.source == DraftSource::Agent
                    && draft
                        .proposed_by
                        .as_ref()
                        .is_some_and(|by| by.agent_id == agent_id)
            })
            .count()
    }

    pub(crate) fn pending_imports(&self) -> usize {
        self.drafts
            .iter()
            .filter(|draft| draft.is_pending() && draft.source == DraftSource::Import)
            .count()
    }

    /// Adds a draft or replaces the one with its id; returns the replaced one.
    pub(crate) fn put_draft(&mut self, draft: SkillDraft) -> Option<SkillDraft> {
        self.generation += 1;
        match self.drafts.iter_mut().find(|known| known.id == draft.id) {
            Some(known) => Some(std::mem::replace(known, draft)),
            None => {
                self.drafts.push(draft);
                None
            }
        }
    }

    pub(crate) fn remove_draft(&mut self, id: &str) -> Option<SkillDraft> {
        let index = self.drafts.iter().position(|draft| draft.id == id)?;
        self.generation += 1;
        Some(self.drafts.remove(index))
    }

    /// Drops decided drafts older than 30 days, then the oldest past 50.
    /// Returns how many went. Pruning is not undone by a failed save: those
    /// drafts were past retention anyway.
    pub(crate) fn prune_decided(&mut self, now_ms: u64) -> usize {
        let before = self.drafts.len();
        let cutoff = now_ms.saturating_sub(DECIDED_DRAFT_RETENTION_MS);
        self.drafts.retain(|draft| {
            draft.is_pending() || draft.decided_at_ms.unwrap_or(draft.created_at_ms) >= cutoff
        });
        let decided = self.decided_drafts().len();
        if decided > MAX_DECIDED_DRAFTS {
            let mut dropped: HashSet<String> = self
                .decided_drafts()
                .into_iter()
                .skip(MAX_DECIDED_DRAFTS)
                .map(|draft| draft.id.clone())
                .collect();
            self.drafts.retain(|draft| !dropped.remove(&draft.id));
        }
        before - self.drafts.len()
    }

    pub(crate) fn snapshot(&self) -> SkillSnapshot {
        SkillSnapshot {
            skills: self.records.values().cloned().collect(),
            drafts: self.drafts.clone(),
        }
    }

    /// Refuses a saved registry the daemon could not have written.
    pub(crate) fn validate(skills: &[SkillRecord], drafts: &[SkillDraft]) -> Result<(), String> {
        let mut slugs = HashSet::new();
        for skill in skills {
            if !is_valid_slug(&skill.slug) || !slugs.insert(skill.slug.as_str()) {
                return Err(format!(
                    "invalid or duplicate skill slug in snapshot: {}",
                    skill.slug
                ));
            }
            if !is_hex_hash(&skill.approved_hash) {
                return Err(format!(
                    "skill {} has a malformed approved hash",
                    skill.slug
                ));
            }
        }
        let mut ids = HashSet::new();
        for draft in drafts {
            if !draft.id.starts_with(DRAFT_ID_PREFIX) || !ids.insert(draft.id.as_str()) {
                return Err(format!(
                    "invalid or duplicate skill draft id in snapshot: {}",
                    draft.id
                ));
            }
            if !is_valid_slug(&draft.slug) {
                return Err(format!(
                    "skill draft {} has an invalid slug: {}",
                    draft.id, draft.slug
                ));
            }
        }
        Ok(())
    }

    /// A validated snapshot as a registry: statuses as saved until the first
    /// scan, decided drafts past retention dropped.
    pub(crate) fn restored(snapshot: SkillSnapshot, now_ms: u64) -> Self {
        let mut registry = Self {
            records: snapshot
                .skills
                .into_iter()
                .map(|record| (record.slug.clone(), record))
                .collect(),
            drafts: snapshot.drafts,
            scanned: BTreeMap::new(),
            generation: 0,
            scanned_once: false,
        };
        registry.prune_decided(now_ms);
        registry
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::{Duration, SystemTime};

    use super::*;
    use crate::skills::{
        compose_skill_file, skill_hash, DraftSource, DraftStatus, ProposedBy, SkillDraft,
        SkillFile, SkillRecord, SkillStatus, DECIDED_DRAFT_RETENTION_MS, MAX_DECIDED_DRAFTS,
        MAX_INDEXED_SKILLS, MAX_SKILLS, SKILL_FILE_NO_FRONT_MATTER, TOO_MANY_SKILLS,
    };

    fn file(name: &str) -> SkillFile {
        SkillFile {
            name: name.into(),
            description: format!("About {name}"),
            body: format!("Do {name}."),
        }
    }

    fn bytes(name: &str) -> Vec<u8> {
        let file = file(name);
        compose_skill_file(&file.name, &file.description, &file.body).into_bytes()
    }

    fn scanned(name: &str) -> ScannedFile {
        ScannedFile::read(
            &bytes(name),
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(5)),
        )
    }

    fn record(slug: &str) -> SkillRecord {
        SkillRecord::approved(slug, &file(slug), skill_hash(&bytes(slug)), 1)
    }

    fn agent_draft(agent: &str, slug: &str, at_ms: u64) -> SkillDraft {
        SkillDraft::new(
            slug,
            file(slug),
            DraftSource::Agent,
            Some(ProposedBy {
                agent_id: agent.into(),
                session_id: "chat:1".into(),
                run_id: "run_1".into(),
            }),
            None,
            at_ms,
        )
    }

    #[test]
    fn a_status_follows_the_file_the_scan_found() {
        let notes = record("notes");
        assert_eq!(status_for(&notes, None), SkillStatus::Missing);
        assert_eq!(
            status_for(&notes, Some(&scanned("notes"))),
            SkillStatus::Active
        );
        assert_eq!(
            status_for(&notes, Some(&scanned("other"))),
            SkillStatus::Changed
        );
        let broken = ScannedFile::read(b"no front matter", None);
        assert_eq!(broken.problem(), Some(SKILL_FILE_NO_FRONT_MATTER));
        assert_eq!(status_for(&notes, Some(&broken)), SkillStatus::Invalid);
        let unreadable = ScannedFile::unreadable("too big", None, 99);
        assert_eq!(status_for(&notes, Some(&unreadable)), SkillStatus::Invalid);
    }

    #[test]
    fn records_are_capped_found_by_slug_or_name_and_bump_the_generation() {
        let mut registry = SkillRegistry::default();
        let start = registry.generation();
        assert_eq!(registry.put(record("notes")).unwrap(), None);
        assert!(registry.generation() > start);
        assert_eq!(
            registry.get("notes").unwrap().status,
            SkillStatus::Active,
            "kept until the first scan"
        );
        assert_eq!(registry.find("notes").unwrap().slug, "notes");
        assert_eq!(registry.find(" /notes ").unwrap().slug, "notes");
        assert_eq!(registry.find("NOTES").unwrap().slug, "notes", "by name");
        assert!(registry.find("ghost").is_none());

        let previous = registry.put(record("notes")).unwrap();
        assert!(previous.is_some(), "a replaced record is returned");
        registry.restore("notes", None);
        assert!(registry.get("notes").is_none());

        for index in 0..MAX_SKILLS {
            registry.put(record(&format!("s{index}"))).unwrap();
        }
        assert_eq!(registry.put(record("one-more")), Err(TOO_MANY_SKILLS));
        assert!(
            registry.put(record("s0")).is_ok(),
            "replacing an existing record is not capped"
        );
        let before = registry.generation();
        assert!(registry.remove("s0").is_some());
        assert!(registry.generation() > before);
    }

    #[test]
    fn the_index_lists_enabled_active_skills_by_slug_up_to_fifty() {
        let mut registry = SkillRegistry::default();
        let mut scan = BTreeMap::new();
        for index in 0..(MAX_INDEXED_SKILLS + 5) {
            let slug = format!("s{index:02}");
            registry.put(record(&slug)).unwrap();
            scan.insert(slug.clone(), scanned(&slug));
        }
        registry.put(record("zz-off")).unwrap();
        scan.insert("zz-off".into(), scanned("zz-off"));
        registry.put(record("aa-changed")).unwrap();
        scan.insert("aa-changed".into(), scanned("something else"));
        let generation = registry.generation();
        assert_eq!(registry.apply_scan(generation, scan), Some(true));
        let mut off = registry.get("zz-off").unwrap().clone();
        off.enabled = false;
        registry.put(off).unwrap();

        let index = registry.index();
        assert_eq!(index.len(), MAX_INDEXED_SKILLS);
        assert_eq!(index[0].slug, "s00");
        assert!(index.iter().all(|skill| skill.slug != "aa-changed"));
        assert_eq!(registry.runnable("s00"), Some(true));
        assert_eq!(registry.runnable("zz-off"), Some(false));
        assert_eq!(registry.runnable("aa-changed"), Some(false));
        assert_eq!(registry.runnable("ghost"), None);
    }

    #[test]
    fn a_scan_older_than_a_change_is_dropped() {
        let mut registry = SkillRegistry::default();
        registry.put(record("notes")).unwrap();
        let seen = registry.generation();
        registry.set_scanned("notes", Some(scanned("notes")));
        let stale = BTreeMap::from([("notes".to_string(), scanned("old content"))]);

        assert_eq!(registry.apply_scan(seen, stale), None);
        assert_eq!(registry.get("notes").unwrap().status, SkillStatus::Active);
    }

    #[test]
    fn of_two_scans_that_began_together_only_the_first_applies() {
        let mut registry = SkillRegistry::default();
        registry.put(record("notes")).unwrap();
        let seen = registry.generation();
        let newer = BTreeMap::from([("notes".to_string(), scanned("edited by hand"))]);
        let older = BTreeMap::from([("notes".to_string(), scanned("notes"))]);

        assert_eq!(registry.apply_scan(seen, newer), Some(true));
        assert_eq!(registry.get("notes").unwrap().status, SkillStatus::Changed);
        assert_eq!(
            registry.apply_scan(seen, older),
            None,
            "the second scan from the same generation is dropped"
        );
        assert_eq!(
            registry.get("notes").unwrap().status,
            SkillStatus::Changed,
            "a `changed` skill is never flipped back to `active` by an older scan"
        );
    }

    #[test]
    fn a_read_is_not_recorded_over_a_scan_that_found_other_content_since() {
        let mut registry = SkillRegistry::default();
        registry.put(record("notes")).unwrap();
        registry.set_scanned("notes", Some(scanned("notes")));
        let seen = registry.scanned_hash("notes");
        let read = Some(skill_hash(&bytes("v2")));

        assert!(
            !registry.scan_moved("notes", &seen, &read),
            "nothing applied since"
        );
        registry.set_scanned("notes", Some(scanned("v2")));
        assert!(
            !registry.scan_moved("notes", &seen, &read),
            "the scan agrees with the read"
        );
        registry.set_scanned("notes", Some(scanned("v3")));
        assert!(
            registry.scan_moved("notes", &seen, &read),
            "a scan may be newer than the read"
        );
        registry.set_scanned("notes", None);
        assert!(
            registry.scan_moved("notes", &seen, &read),
            "the file went meanwhile"
        );
    }

    #[test]
    fn files_without_a_record_are_drafts_until_rejected_at_that_hash() {
        let mut registry = SkillRegistry::default();
        registry.put(record("notes")).unwrap();
        let scan = BTreeMap::from([
            ("notes".to_string(), scanned("notes")),
            ("found".to_string(), scanned("found")),
            (
                "broken".to_string(),
                ScannedFile::read(b"no front matter", None),
            ),
        ]);
        let generation = registry.generation();
        assert_eq!(registry.apply_scan(generation, scan.clone()), Some(true));

        let drafts = registry.file_drafts();
        assert_eq!(drafts.len(), 2);
        let found = drafts
            .iter()
            .find(|view| view.draft.slug == "found")
            .unwrap();
        assert_eq!(found.draft.id, "file:found");
        assert_eq!(found.draft.source, DraftSource::File);
        assert_eq!(found.draft.name, "found");
        assert_eq!(found.draft.file_hash, Some(skill_hash(&bytes("found"))));
        assert_eq!(found.draft.created_at_ms, 5_000);
        assert_eq!(found.problem, None);
        let broken = drafts
            .iter()
            .find(|view| view.draft.slug == "broken")
            .unwrap();
        assert_eq!(broken.problem.as_deref(), Some(SKILL_FILE_NO_FRONT_MATTER));

        let mut rejected =
            SkillDraft::new("found", file("found"), DraftSource::File, None, None, 9);
        rejected.file_hash = Some(skill_hash(&bytes("found")));
        rejected.decide(DraftStatus::Rejected, 9);
        registry.put_draft(rejected);
        assert!(registry
            .file_drafts()
            .iter()
            .all(|view| view.draft.slug != "found"));

        let mut edited = scan;
        edited.insert("found".into(), scanned("found again"));
        let generation = registry.generation();
        assert_eq!(registry.apply_scan(generation, edited), Some(true));
        assert!(
            registry
                .file_drafts()
                .iter()
                .any(|view| view.draft.slug == "found"),
            "an edited file is a draft again"
        );
    }

    #[test]
    fn put_and_restore_keep_the_saved_status_until_the_first_scan() {
        let mut registry = SkillRegistry::default();
        registry.put(record("notes")).unwrap();
        assert_eq!(registry.get("notes").unwrap().status, SkillStatus::Active);
        registry.restore("notes", Some(record("notes")));
        assert_eq!(registry.get("notes").unwrap().status, SkillStatus::Active);

        let generation = registry.generation();
        assert_eq!(registry.apply_scan(generation, BTreeMap::new()), Some(true));
        assert_eq!(registry.get("notes").unwrap().status, SkillStatus::Missing);
        registry.put(record("other")).unwrap();
        assert_eq!(registry.get("other").unwrap().status, SkillStatus::Missing);
    }

    #[test]
    fn reset_scan_forgets_the_last_scan_and_drops_a_scan_in_flight() {
        let mut registry = SkillRegistry::default();
        registry.put(record("notes")).unwrap();
        let scan = BTreeMap::from([("notes".to_string(), scanned("notes"))]);
        let generation = registry.generation();
        assert_eq!(
            registry.apply_scan(generation, scan.clone()),
            Some(false),
            "the saved status was already right"
        );

        let seen = registry.generation();
        registry.reset_scan();
        assert!(registry.scanned_files().is_empty());
        assert!(registry.generation() > seen);
        assert_eq!(registry.apply_scan(seen, scan), None);
        assert_eq!(registry.get("notes").unwrap().status, SkillStatus::Active);
    }

    #[test]
    fn an_unchanged_rescan_reports_nothing() {
        let mut registry = SkillRegistry::default();
        registry.put(record("notes")).unwrap();
        let scan = BTreeMap::from([("notes".to_string(), scanned("edited since approval"))]);
        let generation = registry.generation();
        assert_eq!(registry.apply_scan(generation, scan.clone()), Some(true));
        assert_eq!(registry.get("notes").unwrap().status, SkillStatus::Changed);
        let generation = registry.generation();
        assert_eq!(registry.apply_scan(generation, scan), Some(false));
    }

    #[test]
    fn drafts_are_counted_ordered_and_pruned() {
        let mut registry = SkillRegistry::default();
        let first = agent_draft("agent-1", "a", 1);
        let second = agent_draft("agent-1", "b", 2);
        let other = agent_draft("agent-2", "c", 3);
        let mut import = SkillDraft::new("d", file("d"), DraftSource::Import, None, None, 4);
        for draft in [second.clone(), first.clone(), other.clone(), import.clone()] {
            assert!(registry.put_draft(draft).is_none());
        }
        assert_eq!(registry.pending_from("agent-1"), 2);
        assert_eq!(registry.pending_imports(), 1);
        let pending: Vec<_> = registry
            .pending_drafts()
            .into_iter()
            .map(|draft| draft.slug.clone())
            .collect();
        assert_eq!(pending, ["a", "b", "c", "d"]);

        import.decide(DraftStatus::Approved, 10);
        assert!(registry.put_draft(import.clone()).is_some());
        assert_eq!(registry.pending_imports(), 0);
        assert_eq!(registry.decided_drafts()[0].id, import.id);
        assert_eq!(registry.remove_draft(&other.id).unwrap().id, other.id);

        assert_eq!(registry.prune_decided(10 + DECIDED_DRAFT_RETENTION_MS), 0);
        assert_eq!(registry.prune_decided(11 + DECIDED_DRAFT_RETENTION_MS), 1);
        assert!(registry.draft(&import.id).is_none());

        for index in 0..(MAX_DECIDED_DRAFTS + 3) {
            let mut decided = agent_draft("agent-3", "e", 100);
            decided.decide(DraftStatus::Rejected, 100 + index as u64);
            registry.put_draft(decided);
        }
        assert_eq!(registry.prune_decided(200), 3);
        assert_eq!(registry.decided_drafts().len(), MAX_DECIDED_DRAFTS);
        assert_eq!(
            registry.decided_drafts().last().unwrap().decided_at_ms,
            Some(103),
            "the oldest decided drafts went first"
        );
        assert_eq!(registry.pending_from("agent-1"), 2, "pending drafts stay");
    }

    #[test]
    fn a_snapshot_with_bad_records_or_drafts_is_refused() {
        let good = record("notes");
        let draft = agent_draft("agent-1", "notes", 1);
        assert!(SkillRegistry::validate(&[good.clone()], &[draft.clone()]).is_ok());

        let mut bad_slug = good.clone();
        bad_slug.slug = "Bad Slug".into();
        let mut bad_hash = good.clone();
        bad_hash.approved_hash = "not-a-hash".into();
        for skills in [
            vec![good.clone(), good.clone()],
            vec![bad_slug],
            vec![bad_hash],
        ] {
            assert!(SkillRegistry::validate(&skills, &[]).is_err());
        }
        let mut bad_id = draft.clone();
        bad_id.id = "file:notes".into();
        let mut bad_draft_slug = draft.clone();
        bad_draft_slug.slug = "../x".into();
        for drafts in [
            vec![draft.clone(), draft.clone()],
            vec![bad_id],
            vec![bad_draft_slug],
        ] {
            assert!(SkillRegistry::validate(&[], &drafts).is_err());
        }
    }

    #[test]
    fn a_restored_registry_keeps_records_and_drops_expired_decided_drafts() {
        let mut expired = agent_draft("agent-1", "old", 1);
        expired.decide(DraftStatus::Rejected, 1);
        let pending = agent_draft("agent-1", "new", 2);
        let restored = SkillRegistry::restored(
            SkillSnapshot {
                skills: vec![record("notes")],
                drafts: vec![expired, pending.clone()],
            },
            2 + DECIDED_DRAFT_RETENTION_MS,
        );
        assert_eq!(restored.get("notes").unwrap().status, SkillStatus::Active);
        assert_eq!(restored.snapshot().drafts, vec![pending]);
        assert_eq!(restored.snapshot().skills, vec![record("notes")]);
    }

    #[test]
    fn the_registry_round_trips_through_a_saved_control_plane() {
        let mut source = crate::state::DaemonState::new();
        source.skills.put(record("notes")).unwrap();
        source.skills.put_draft(agent_draft("agent-1", "draft", 3));

        let payload = serde_json::to_value(source.control_plane_snapshot()).unwrap();
        assert_eq!(payload["version"], 9);
        assert_eq!(payload["skills"][0]["slug"], "notes");
        assert_eq!(payload["skillDrafts"][0]["slug"], "draft");

        let mut restored = crate::state::DaemonState::new();
        restored
            .restore_control_plane_snapshot(serde_json::from_value(payload).unwrap())
            .unwrap();
        assert_eq!(restored.skills.get("notes"), source.skills.get("notes"));
        assert_eq!(restored.skills.pending_from("agent-1"), 1);
    }

    #[test]
    fn a_saved_control_plane_with_a_bad_skill_refuses_to_load() {
        let mut snapshot = crate::state::DaemonState::new().control_plane_snapshot();
        let mut bad = record("notes");
        bad.slug = "../escape".into();
        snapshot.skills = vec![bad];
        assert!(crate::state::DaemonState::new()
            .restore_control_plane_snapshot(snapshot)
            .is_err());
    }
}
