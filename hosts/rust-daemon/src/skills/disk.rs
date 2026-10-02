//! Skill files in the workspace (spec §8.1–§8.2): reading a `SKILL.md`
//! whole and only inside the workspace, scanning the skills folder (a file
//! whose modification time and size are unchanged is not read again),
//! writing approved content through the hardened workspace writer (spec
//! §14), and moving a deleted skill's folder to the workspace trash.
//! Everything here blocks: callers run it through `spawn_blocking`, bounded
//! by `SKILL_IO_TIMEOUT_MS`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::registry::ScannedFile;
use super::{
    is_valid_slug, MAX_EXAMINED_SKILL_FOLDERS, MAX_SCANNED_SKILL_FOLDERS, MAX_SKILL_FILE_BYTES,
    SKILLS_FOLDER, SKILLS_FOLDER_OUTSIDE, SKILLS_TRASH_FOLDER, SKILL_FILE_NAME,
    SKILL_FILE_NOT_REGULAR, SKILL_FILE_OUTSIDE, SKILL_FILE_TOO_LARGE, SKILL_FOLDER_NOT_LOWERCASE,
    SKILL_SLUG_INVALID,
};
use crate::tools::{canonical_workspace_root, write_workspace_bytes};

/// What the hardened writer's messages name.
const WRITER: &str = "skills";
/// The trash folder's parent, workspace-relative.
const TRASH_ROOT: &str = ".anima-trash";
const TRASH_OUTSIDE: &str = "the workspace trash resolves outside the workspace";

/// `skills/<slug>/SKILL.md`, workspace-relative.
pub(crate) fn skill_file_path(slug: &str) -> String {
    format!("{SKILLS_FOLDER}/{slug}/{SKILL_FILE_NAME}")
}

/// `.anima-trash/skills/<name>`, workspace-relative: where a trashed folder
/// (as `trash_skill_folder` names it) lives.
pub(crate) fn trash_relative_path(name: &str) -> String {
    format!("{SKILLS_TRASH_FOLDER}/{name}")
}

fn checked(slug: &str) -> Result<(), String> {
    if is_valid_slug(slug) {
        Ok(())
    } else {
        Err(SKILL_SLUG_INVALID.to_string())
    }
}

fn root_of(workspace: &Path) -> Result<PathBuf, String> {
    canonical_workspace_root(workspace, WRITER)
}

/// The skill's `SKILL.md` bytes; `Ok(None)` when there is none.
pub(crate) fn read_skill_bytes(workspace: &Path, slug: &str) -> Result<Option<Vec<u8>>, String> {
    checked(slug)?;
    read_in(&root_of(workspace)?, slug)
}

fn read_in(root: &Path, slug: &str) -> Result<Option<Vec<u8>>, String> {
    let path = root.join(SKILLS_FOLDER).join(slug).join(SKILL_FILE_NAME);
    match fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("SKILL.md could not be read: {error}")),
    }
    // A dangling link has no canonical path: it is outside by definition.
    let canonical = path
        .canonicalize()
        .map_err(|_| SKILL_FILE_OUTSIDE.to_string())?;
    if !canonical.starts_with(root) {
        return Err(SKILL_FILE_OUTSIDE.to_string());
    }
    // Opening a FIFO or a device can block forever: only a regular file is
    // opened, and it is checked again once open.
    let regular = fs::metadata(&canonical)
        .map_err(|error| format!("SKILL.md could not be read: {error}"))?
        .is_file();
    if !regular {
        return Err(SKILL_FILE_NOT_REGULAR.to_string());
    }
    let file = fs::File::open(&canonical)
        .map_err(|error| format!("SKILL.md could not be read: {error}"))?;
    let still_regular = file
        .metadata()
        .map_err(|error| format!("SKILL.md could not be read: {error}"))?
        .is_file();
    if !still_regular {
        return Err(SKILL_FILE_NOT_REGULAR.to_string());
    }
    let mut bytes = Vec::new();
    file.take(MAX_SKILL_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("SKILL.md could not be read: {error}"))?;
    if bytes.len() > MAX_SKILL_FILE_BYTES {
        return Err(SKILL_FILE_TOO_LARGE.to_string());
    }
    Ok(Some(bytes))
}

/// What `slug`'s `SKILL.md` holds now; `Ok(None)` when there is none.
pub(crate) fn scan_skill(
    workspace: &Path,
    slug: &str,
    previous: Option<&ScannedFile>,
) -> Result<Option<ScannedFile>, String> {
    checked(slug)?;
    Ok(scan_in(&root_of(workspace)?, slug, previous))
}

fn scan_in(root: &Path, slug: &str, previous: Option<&ScannedFile>) -> Option<ScannedFile> {
    let path = root.join(SKILLS_FOLDER).join(slug).join(SKILL_FILE_NAME);
    let metadata = match fs::metadata(&path) {
        Ok(metadata) => metadata,
        // A dangling link exists without a target: outside by definition.
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return fs::symlink_metadata(&path)
                .ok()
                .map(|_| ScannedFile::unreadable(SKILL_FILE_OUTSIDE, None, 0));
        }
        Err(error) => {
            return Some(ScannedFile::unreadable(
                format!("SKILL.md could not be read: {error}"),
                None,
                0,
            ))
        }
    };
    let modified = metadata.modified().ok();
    let len = metadata.len();
    if let Some(previous) = previous {
        if modified.is_some() && previous.modified == modified && previous.len == len {
            return Some(previous.clone());
        }
    }
    match read_in(root, slug) {
        Ok(Some(bytes)) => Some(ScannedFile::read(&bytes, modified)),
        Ok(None) => None,
        Err(problem) => Some(ScannedFile::unreadable(
            problem.clone(),
            cache_time(&problem, modified),
            len,
        )),
    }
}

/// The modification time to cache a read problem under: only problems that
/// a retry would repeat keep it; a transient I/O error (a sharing violation,
/// a placeholder that failed to fetch) caches none, so the next scan reads
/// the file again.
fn cache_time(problem: &str, modified: Option<SystemTime>) -> Option<SystemTime> {
    let stable = [
        SKILL_FILE_TOO_LARGE,
        SKILL_FILE_OUTSIDE,
        SKILL_FILE_NOT_REGULAR,
    ];
    stable.contains(&problem).then_some(modified).flatten()
}

/// `name` when it is not a slug only because of its case.
fn lowercase_slug(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    (lower != name && is_valid_slug(&lower)).then_some(lower)
}

/// Every skill folder's `SKILL.md`, by slug: first the `registered` slugs,
/// read directly, then the other folders whose names are valid slugs, by
/// name. At most `MAX_SCANNED_SKILL_FOLDERS` entries (folders holding a
/// `SKILL.md`), and at most `MAX_EXAMINED_SKILL_FOLDERS` unregistered
/// folders looked into, so a flood of empty folders cannot stall a scan. A
/// folder whose name differs from a slug only in case is reported as a
/// problem under the lowercase slug (spec §8.1).
pub(crate) fn scan_skills_folder(
    workspace: &Path,
    previous: &BTreeMap<String, ScannedFile>,
    registered: &BTreeSet<String>,
) -> Result<BTreeMap<String, ScannedFile>, String> {
    let root = root_of(workspace)?;
    let folder = root.join(SKILLS_FOLDER);
    let canonical = match folder.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(format!("the skills folder could not be read: {error}")),
    };
    if !canonical.starts_with(&root) {
        return Err(SKILLS_FOLDER_OUTSIDE.to_string());
    }

    let mut scanned = BTreeMap::new();
    for slug in registered {
        if scanned.len() >= MAX_SCANNED_SKILL_FOLDERS {
            break;
        }
        if !is_valid_slug(slug) {
            continue;
        }
        if let Some(file) = scan_in(&root, slug, previous.get(slug)) {
            scanned.insert(slug.clone(), file);
        }
    }

    let mut slugs = Vec::new();
    let mut case_only = Vec::new();
    for entry in fs::read_dir(&folder)
        .map_err(|error| format!("the skills folder could not be read: {error}"))?
        .filter_map(Result::ok)
    {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if registered.contains(&name) {
            continue;
        }
        // The directory listing says what an entry is without another stat;
        // a link is only resolved when it is examined.
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if !kind.is_dir() && !kind.is_symlink() {
            continue;
        }
        let candidate = (name, kind.is_symlink());
        if is_valid_slug(&candidate.0) {
            slugs.push(candidate);
        } else if lowercase_slug(&candidate.0).is_some() {
            case_only.push(candidate);
        }
    }
    slugs.sort();
    case_only.sort();

    let mut examined = 0;
    for (slug, is_link) in slugs {
        if scanned.len() >= MAX_SCANNED_SKILL_FOLDERS || examined >= MAX_EXAMINED_SKILL_FOLDERS {
            break;
        }
        examined += 1;
        if is_link && !folder.join(&slug).is_dir() {
            continue;
        }
        if let Some(file) = scan_in(&root, &slug, previous.get(&slug)) {
            scanned.insert(slug, file);
        }
    }
    for (name, is_link) in case_only {
        if scanned.len() >= MAX_SCANNED_SKILL_FOLDERS || examined >= MAX_EXAMINED_SKILL_FOLDERS {
            break;
        }
        let Some(slug) = lowercase_slug(&name) else {
            continue;
        };
        let path = folder.join(&name);
        examined += 1;
        if is_link && !path.is_dir() {
            continue;
        }
        if scanned.contains_key(&slug) || fs::symlink_metadata(path.join(SKILL_FILE_NAME)).is_err()
        {
            continue;
        }
        scanned.insert(
            slug,
            ScannedFile::unreadable(SKILL_FOLDER_NOT_LOWERCASE, None, 0),
        );
    }
    Ok(scanned)
}

/// Writes approved content through the hardened workspace writer: no `..`,
/// no escaping link, parents re-verified (spec §14). Not atomic: a reader
/// that catches it midway sees a hash that does not match and refuses it.
pub(crate) fn write_skill_file(workspace: &Path, slug: &str, bytes: &[u8]) -> Result<(), String> {
    checked(slug)?;
    write_workspace_bytes(workspace, &skill_file_path(slug), bytes, WRITER).map(|_| ())
}

/// Refuses an existing `path` (a link, a junction) that resolves outside the
/// workspace; a path that does not exist is fine.
fn existing_inside(root: &Path, path: &Path) -> Result<(), String> {
    match path.canonicalize() {
        Ok(canonical) if canonical.starts_with(root) => Ok(()),
        Ok(_) => Err(TRASH_OUTSIDE.to_string()),
        // A dangling link exists without a target: outside by definition.
        Err(_) if fs::symlink_metadata(path).is_ok() => Err(TRASH_OUTSIDE.to_string()),
        Err(_) => Ok(()),
    }
}

/// The canonical trash folder, created if needed, inside `root`.
fn trash_folder(root: &Path) -> Result<PathBuf, String> {
    let parent = root.join(TRASH_ROOT);
    let trash = root.join(SKILLS_TRASH_FOLDER);
    existing_inside(root, &parent)?;
    existing_inside(root, &trash)?;
    fs::create_dir_all(&trash)
        .map_err(|error| format!("the workspace trash could not be created: {error}"))?;
    let canonical = trash
        .canonicalize()
        .map_err(|error| format!("the workspace trash could not be read: {error}"))?;
    if !canonical.starts_with(root) {
        return Err(TRASH_OUTSIDE.to_string());
    }
    Ok(canonical)
}

/// Moves `slug`'s folder to `.anima-trash/skills/<slug>-<now_ms>` (spec
/// §8.2), adding `-1`, `-2`, … if that name is taken, and returns the trash
/// folder's name (see `trash_relative_path`); `Ok(None)` when there is no
/// folder. A folder that is a link moves as the link.
pub(crate) fn trash_skill_folder(
    workspace: &Path,
    slug: &str,
    now_ms: u64,
) -> Result<Option<String>, String> {
    checked(slug)?;
    let root = root_of(workspace)?;
    let skills = root.join(SKILLS_FOLDER);
    let folder = skills.join(slug);
    match fs::symlink_metadata(&folder) {
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("the skill folder could not be moved: {error}")),
    }
    let canonical_skills = skills
        .canonicalize()
        .map_err(|error| format!("the skill folder could not be moved: {error}"))?;
    if !canonical_skills.starts_with(&root) {
        return Err(SKILLS_FOLDER_OUTSIDE.to_string());
    }
    let trash = trash_folder(&root)?;
    let mut name = format!("{slug}-{now_ms}");
    let mut suffix = 1;
    while fs::symlink_metadata(trash.join(&name)).is_ok() {
        name = format!("{slug}-{now_ms}-{suffix}");
        suffix += 1;
    }
    fs::rename(&folder, trash.join(&name))
        .map_err(|error| format!("the skill folder could not be moved: {error}"))?;
    Ok(Some(name))
}

/// Whether `name` is exactly `<slug>-<digits>` or `<slug>-<digits>-<digits>`.
fn is_trash_name(slug: &str, name: &str) -> bool {
    let Some(rest) = name
        .strip_prefix(slug)
        .and_then(|rest| rest.strip_prefix('-'))
    else {
        return false;
    };
    let parts = rest.split('-').collect::<Vec<_>>();
    (1..=2).contains(&parts.len())
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

/// Puts a trashed folder (a name `trash_skill_folder` returned) back, for a
/// delete whose save failed; refuses when a new folder took its place.
pub(crate) fn untrash_skill_folder(
    workspace: &Path,
    slug: &str,
    trashed: &str,
) -> Result<(), String> {
    checked(slug)?;
    if !is_trash_name(slug, trashed) {
        return Err("not a skills trash name".to_string());
    }
    let root = root_of(workspace)?;
    let trash = root.join(SKILLS_TRASH_FOLDER);
    existing_inside(&root, &root.join(TRASH_ROOT))?;
    existing_inside(&root, &trash)?;
    let skills = root.join(SKILLS_FOLDER);
    // The skills folder may be a link; a folder is never put back through it.
    if let Ok(canonical) = skills.canonicalize() {
        if !canonical.starts_with(&root) {
            return Err(SKILLS_FOLDER_OUTSIDE.to_string());
        }
    } else if fs::symlink_metadata(&skills).is_ok() {
        return Err(SKILLS_FOLDER_OUTSIDE.to_string());
    }
    let folder = skills.join(slug);
    if fs::symlink_metadata(&folder).is_ok() {
        return Err("a new folder took its place".to_string());
    }
    fs::rename(trash.join(trashed), folder)
        .map_err(|error| format!("the skill folder could not be put back: {error}"))
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime};

    use super::*;
    use crate::skills::{
        compose_skill_file, skill_hash, MAX_SCANNED_SKILL_FOLDERS, MAX_SKILL_FILE_BYTES,
        SKILL_FILE_NOT_REGULAR, SKILL_FILE_NO_FRONT_MATTER, SKILL_FILE_TOO_LARGE,
    };

    fn workspace(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "anima-skills-disk-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn put(root: &Path, slug: &str, text: &str) -> PathBuf {
        let folder = root.join("skills").join(slug);
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("SKILL.md");
        std::fs::write(&path, text).unwrap();
        path
    }

    fn skill_text(name: &str) -> String {
        compose_skill_file(name, &format!("About {name}"), &format!("Do {name}."))
    }

    fn none() -> BTreeSet<String> {
        BTreeSet::new()
    }

    #[test]
    fn a_skill_file_is_read_whole_or_refused_past_the_limit() {
        let root = workspace("read");
        assert_eq!(read_skill_bytes(&root, "notes"), Ok(None));
        put(&root, "notes", &skill_text("notes"));
        assert_eq!(
            read_skill_bytes(&root, "notes").unwrap().unwrap(),
            skill_text("notes").into_bytes()
        );
        put(&root, "huge", &"x".repeat(MAX_SKILL_FILE_BYTES + 1));
        assert_eq!(
            read_skill_bytes(&root, "huge"),
            Err(SKILL_FILE_TOO_LARGE.to_string())
        );
        assert!(read_skill_bytes(&root, "../notes").is_err(), "invalid slug");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_scan_reads_valid_slug_folders_by_name() {
        let root = workspace("scan");
        assert_eq!(
            scan_skills_folder(&root, &BTreeMap::new(), &none()),
            Ok(BTreeMap::new())
        );
        put(&root, "notes", &skill_text("notes"));
        put(&root, "broken", "no front matter");
        put(&root, "Bad_Name", &skill_text("bad"));
        std::fs::create_dir_all(root.join("skills").join("empty")).unwrap();
        std::fs::write(root.join("skills").join("readme.md"), "not a folder").unwrap();

        let scanned = scan_skills_folder(&root, &BTreeMap::new(), &none()).unwrap();
        assert_eq!(
            scanned.keys().cloned().collect::<Vec<_>>(),
            ["broken", "notes"],
            "an empty folder has no SKILL.md and an invalid name is skipped"
        );
        assert_eq!(
            scanned["notes"].hash.as_deref(),
            Some(skill_hash(skill_text("notes").as_bytes()).as_str())
        );
        assert_eq!(
            scanned["broken"].problem(),
            Some(SKILL_FILE_NO_FRONT_MATTER)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_scan_reads_at_most_two_hundred_folders() {
        let root = workspace("scan-cap");
        for index in 0..(MAX_SCANNED_SKILL_FOLDERS + 3) {
            put(&root, &format!("s{index:03}"), &skill_text("s"));
        }
        let scanned = scan_skills_folder(&root, &BTreeMap::new(), &none()).unwrap();
        assert_eq!(scanned.len(), MAX_SCANNED_SKILL_FOLDERS);
        assert!(scanned.contains_key("s000"));
        assert!(!scanned.contains_key("s202"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn folders_without_a_skill_file_do_not_count_toward_the_cap_and_registered_skills_come_first() {
        let root = workspace("scan-empty-flood");
        for index in 0..300 {
            let folder = root.join("skills").join(format!("a{index:03}"));
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(folder.join("notes.txt"), "not a skill").unwrap();
        }
        for index in 0..203 {
            put(&root, &format!("s{index:03}"), &skill_text("s"));
        }

        let scanned = scan_skills_folder(&root, &BTreeMap::new(), &none()).unwrap();
        assert_eq!(scanned.len(), MAX_SCANNED_SKILL_FOLDERS);
        assert!(scanned.contains_key("s000"));
        assert!(!scanned.contains_key("s202"));

        let registered = BTreeSet::from(["s202".to_string(), "ghost".to_string()]);
        let scanned = scan_skills_folder(&root, &BTreeMap::new(), &registered).unwrap();
        assert_eq!(scanned.len(), MAX_SCANNED_SKILL_FOLDERS);
        assert!(scanned.contains_key("s202"));
        assert!(!scanned.contains_key("s199"));
        assert!(!scanned.contains_key("ghost"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_folder_whose_name_differs_only_in_case_is_reported() {
        let root = workspace("scan-case");
        put(&root, "Notes", &skill_text("notes"));
        let scanned = scan_skills_folder(&root, &BTreeMap::new(), &none()).unwrap();
        assert_eq!(scanned["notes"].problem(), Some(SKILL_FOLDER_NOT_LOWERCASE));
        assert_eq!(scanned["notes"].hash, None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_unchanged_modification_time_and_size_reuse_the_last_hash() {
        let root = workspace("cache");
        let path = put(&root, "notes", "---\nname: A\ndescription: d\n---\n\nAAAA");
        let first = scan_skill(&root, "notes", None).unwrap().unwrap();
        let modified = first
            .modified
            .expect("this file system keeps modification times");

        std::fs::write(&path, "---\nname: B\ndescription: d\n---\n\nBBBB").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        let cached = scan_skill(&root, "notes", Some(&first)).unwrap().unwrap();
        assert_eq!(cached, first, "same time and size: not read again");

        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified + Duration::from_secs(10))
            .unwrap();
        let fresh = scan_skill(&root, "notes", Some(&first)).unwrap().unwrap();
        assert_ne!(fresh.hash, first.hash, "a new time is read again");
        assert_eq!(fresh.parsed.unwrap().name, "B");
        assert_eq!(scan_skill(&root, "ghost", None), Ok(None));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn approved_content_is_written_through_the_hardened_writer() {
        let root = workspace("write");
        write_skill_file(&root, "notes", skill_text("notes").as_bytes()).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("skills/notes/SKILL.md")).unwrap(),
            skill_text("notes")
        );
        assert_eq!(skill_file_path("notes"), "skills/notes/SKILL.md");
        assert!(write_skill_file(&root, "..", b"x").is_err());
        assert!(write_skill_file(&root, "nul", b"x").is_err());
        assert!(!root.join("skills/nul").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_deleted_skill_moves_to_the_workspace_trash_and_can_come_back() {
        let root = workspace("trash");
        assert_eq!(trash_skill_folder(&root, "notes", 42), Ok(None));
        put(&root, "notes", &skill_text("notes"));
        std::fs::write(root.join("skills/notes/extra.txt"), "kept").unwrap();

        let trashed = trash_skill_folder(&root, "notes", 42).unwrap().unwrap();
        assert_eq!(trashed, "notes-42");
        assert_eq!(
            trash_relative_path(&trashed),
            ".anima-trash/skills/notes-42"
        );
        assert!(!root.join("skills/notes").exists());
        assert_eq!(
            std::fs::read_to_string(root.join(trash_relative_path(&trashed)).join("extra.txt"))
                .unwrap(),
            "kept"
        );

        put(&root, "notes", &skill_text("notes"));
        assert_eq!(
            trash_skill_folder(&root, "notes", 42).unwrap().as_deref(),
            Some("notes-42-1"),
            "a second delete in the same millisecond gets a suffix"
        );
        untrash_skill_folder(&root, "notes", &trashed).unwrap();
        assert!(root.join("skills/notes/SKILL.md").exists());
        assert!(
            untrash_skill_folder(&root, "notes", "notes-42-1").is_err(),
            "a folder that took its place is never overwritten"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn untrash_refuses_a_name_that_is_not_a_trash_name() {
        let root = workspace("untrash-names");
        put(&root, "notes", &skill_text("notes"));
        let trashed = trash_skill_folder(&root, "notes", 42).unwrap().unwrap();
        for bad in [
            "../notes-42",
            "notes-42/../../x",
            "other-42",
            "notes-",
            "notes-abc",
            "notes-42-",
            "notes-42-1-2",
            ".anima-trash/skills/notes-42",
            "notes-4 2",
        ] {
            assert!(untrash_skill_folder(&root, "notes", bad).is_err(), "{bad}");
        }
        assert!(!root.join("skills/notes").exists());
        untrash_skill_folder(&root, "notes", &trashed).unwrap();
        assert!(root.join("skills/notes/SKILL.md").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn links_out_of_the_workspace_are_refused() {
        let root = workspace("links");
        let outside = workspace("links-outside");
        std::fs::write(outside.join("SKILL.md"), skill_text("secret")).unwrap();
        std::fs::create_dir_all(root.join("skills/linked")).unwrap();
        std::os::unix::fs::symlink(
            outside.join("SKILL.md"),
            root.join("skills/linked/SKILL.md"),
        )
        .unwrap();

        assert_eq!(
            read_skill_bytes(&root, "linked"),
            Err(SKILL_FILE_OUTSIDE.to_string())
        );
        let scanned = scan_skill(&root, "linked", None).unwrap().unwrap();
        assert_eq!(scanned.problem(), Some(SKILL_FILE_OUTSIDE));
        assert_eq!(scanned.hash, None);

        std::fs::create_dir_all(root.join("skills")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("skills/folder-link")).unwrap();
        let trashed = trash_skill_folder(&root, "folder-link", 7)
            .unwrap()
            .unwrap();
        assert!(
            outside.join("SKILL.md").exists(),
            "the link moved, never its target"
        );
        assert!(
            std::fs::symlink_metadata(root.join(trash_relative_path(&trashed)))
                .unwrap()
                .file_type()
                .is_symlink()
        );

        let other = workspace("links-folder");
        std::os::unix::fs::symlink(&outside, other.join("skills")).unwrap();
        assert_eq!(
            scan_skills_folder(&other, &BTreeMap::new(), &none()),
            Err(SKILLS_FOLDER_OUTSIDE.to_string())
        );
        for path in [root, outside, other] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_trash_folder_that_is_a_link_is_refused() {
        let root = workspace("trash-link");
        let outside = workspace("trash-link-outside");
        put(&root, "notes", &skill_text("notes"));
        std::os::unix::fs::symlink(&outside, root.join(".anima-trash")).unwrap();

        assert!(trash_skill_folder(&root, "notes", 1).is_err());
        assert!(root.join("skills/notes/SKILL.md").exists());
        assert!(!outside.join("skills").exists());
        let _ = std::fs::remove_file(root.join(".anima-trash"));
        for path in [root, outside] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    /// Makes `link` a junction to `target` (no privilege needed); `false`
    /// when `mklink` is unavailable.
    #[cfg(windows)]
    fn junction(link: &Path, target: &Path) -> bool {
        std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    #[cfg(windows)]
    #[test]
    fn junctions_out_of_the_workspace_are_refused() {
        let root = workspace("junctions");
        let outside = workspace("junctions-outside");
        let other = workspace("junctions-folder");
        std::fs::write(outside.join("SKILL.md"), skill_text("secret")).unwrap();
        std::fs::create_dir_all(root.join("skills")).unwrap();
        let linked = root.join("skills").join("linked");
        let skills_link = other.join("skills");

        let made = junction(&linked, &outside) && junction(&skills_link, &outside);
        if made {
            assert_eq!(
                read_skill_bytes(&root, "linked"),
                Err(SKILL_FILE_OUTSIDE.to_string())
            );
            let scanned = scan_skill(&root, "linked", None).unwrap().unwrap();
            assert_eq!(scanned.problem(), Some(SKILL_FILE_OUTSIDE));
            assert_eq!(scanned.hash, None);
            assert_eq!(
                scan_skills_folder(&other, &BTreeMap::new(), &none()),
                Err(SKILLS_FOLDER_OUTSIDE.to_string())
            );
            // A folder that is a junction moves as the junction; its target
            // is never touched.
            let trashed = trash_skill_folder(&root, "linked", 7).unwrap().unwrap();
            assert!(outside.join("SKILL.md").exists());
            assert!(!root.join("skills/linked").exists());
            let _ = std::fs::remove_dir(root.join(trash_relative_path(&trashed)));
        } else {
            println!("mklink /J is unavailable here; skipping the junction checks");
        }
        let _ = std::fs::remove_dir(&linked);
        let _ = std::fs::remove_dir(&skills_link);
        for path in [root, outside, other] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    #[test]
    fn a_skill_file_that_is_not_a_regular_file_is_refused_without_opening_it() {
        let root = workspace("not-regular");
        std::fs::create_dir_all(root.join("skills/dir-skill/SKILL.md")).unwrap();
        assert_eq!(
            read_skill_bytes(&root, "dir-skill"),
            Err(SKILL_FILE_NOT_REGULAR.to_string())
        );
        let scanned = scan_skill(&root, "dir-skill", None).unwrap().unwrap();
        assert_eq!(scanned.problem(), Some(SKILL_FILE_NOT_REGULAR));
        assert!(
            scanned.modified.is_some(),
            "a retry would repeat it: the time is cached"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_named_skill_md_is_refused_instead_of_blocking() {
        let root = workspace("fifo");
        let folder = root.join("skills/piped");
        std::fs::create_dir_all(&folder).unwrap();
        let made = std::process::Command::new("mkfifo")
            .arg(folder.join("SKILL.md"))
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if made {
            assert_eq!(
                read_skill_bytes(&root, "piped"),
                Err(SKILL_FILE_NOT_REGULAR.to_string())
            );
            let scanned = scan_skill(&root, "piped", None).unwrap().unwrap();
            assert_eq!(scanned.problem(), Some(SKILL_FILE_NOT_REGULAR));
        } else {
            println!("mkfifo is unavailable here; skipping the FIFO check");
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn only_problems_a_retry_would_repeat_keep_their_cache_time() {
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(9);
        for stable in [
            SKILL_FILE_TOO_LARGE,
            SKILL_FILE_OUTSIDE,
            SKILL_FILE_NOT_REGULAR,
        ] {
            assert_eq!(cache_time(stable, Some(at)), Some(at), "{stable}");
        }
        assert_eq!(
            cache_time("SKILL.md could not be read: sharing violation", Some(at)),
            None
        );
    }

    #[test]
    fn an_io_error_is_not_reused_by_the_next_scan() {
        let root = workspace("io-error");
        let path = put(&root, "notes", &skill_text("notes"));
        let metadata = std::fs::metadata(&path).unwrap();
        // What an earlier scan stored for a transient failure: it holds no
        // time, whatever the file's own.
        let failed = ScannedFile::unreadable(
            "SKILL.md could not be read: sharing violation",
            cache_time(
                "SKILL.md could not be read: sharing violation",
                metadata.modified().ok(),
            ),
            metadata.len(),
        );
        assert_eq!(failed.modified, None);
        let again = scan_skill(&root, "notes", Some(&failed)).unwrap().unwrap();
        assert_eq!(again.problem(), None, "the file is read again");
        assert_eq!(
            again.hash.as_deref(),
            Some(skill_hash(skill_text("notes").as_bytes()).as_str())
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn a_locked_file_is_read_again_once_it_is_free() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = workspace("locked");
        let path = put(&root, "notes", &skill_text("notes"));
        let lock = std::fs::File::options()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        let failed = scan_skill(&root, "notes", None).unwrap().unwrap();
        assert!(
            failed.problem().is_some(),
            "a sharing violation is a problem"
        );
        assert_eq!(failed.modified, None, "and is not cached");
        drop(lock);
        let again = scan_skill(&root, "notes", Some(&failed)).unwrap().unwrap();
        assert_eq!(again.problem(), None);
        assert!(again.hash.is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn untrash_refuses_a_skills_folder_that_is_a_link_out_of_the_workspace() {
        let root = workspace("untrash-link");
        let outside = workspace("untrash-link-outside");
        put(&root, "notes", &skill_text("notes"));
        let trashed = trash_skill_folder(&root, "notes", 42).unwrap().unwrap();
        std::fs::remove_dir_all(root.join("skills")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("skills")).unwrap();

        assert_eq!(
            untrash_skill_folder(&root, "notes", &trashed),
            Err(SKILLS_FOLDER_OUTSIDE.to_string())
        );
        assert!(!outside.join("notes").exists());
        assert!(root.join(trash_relative_path(&trashed)).exists());
        let _ = std::fs::remove_file(root.join("skills"));
        for path in [root, outside] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    #[cfg(windows)]
    #[test]
    fn untrash_refuses_a_skills_folder_that_is_a_junction_out_of_the_workspace() {
        let root = workspace("untrash-junction");
        let outside = workspace("untrash-junction-outside");
        put(&root, "notes", &skill_text("notes"));
        let trashed = trash_skill_folder(&root, "notes", 42).unwrap().unwrap();
        std::fs::remove_dir_all(root.join("skills")).unwrap();
        let skills_link = root.join("skills");

        if junction(&skills_link, &outside) {
            assert_eq!(
                untrash_skill_folder(&root, "notes", &trashed),
                Err(SKILLS_FOLDER_OUTSIDE.to_string())
            );
            assert!(!outside.join("notes").exists());
            assert!(root.join(trash_relative_path(&trashed)).exists());
        } else {
            println!("mklink /J is unavailable here; skipping the junction check");
        }
        let _ = std::fs::remove_dir(&skills_link);
        for path in [root, outside] {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    #[test]
    fn the_scan_cache_key_is_the_modification_time_and_size() {
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(9);
        let file = ScannedFile::read(skill_text("notes").as_bytes(), Some(at));
        assert_eq!(file.len, skill_text("notes").len() as u64);
        assert_eq!(file.modified, Some(at));
    }
}
