use std::fs;
use std::path::Path;

use super::super::workspace::{
    resolve_existing_workspace_file, resolve_workspace_write_path, workspace_root_path,
};
use super::FileEditOperation;

pub(in super::super) fn write_workspace_file(
    file_path: &str,
    content: &str,
) -> Result<String, String> {
    let workspace_root = workspace_root_path("write_file")?;
    write_workspace_file_from_root(&workspace_root, file_path, content)
}

pub(in super::super) fn write_workspace_file_from_root(
    workspace_root: &Path,
    file_path: &str,
    content: &str,
) -> Result<String, String> {
    let target = resolve_workspace_write_path(workspace_root, file_path, "write_file")?;
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!("write_file failed to create directories for {file_path}: {error}")
        })?;
    }

    fs::write(&target, content)
        .map_err(|error| format!("write_file failed to write {file_path}: {error}"))?;

    Ok(format!(
        "Wrote {} chars to {}",
        content.chars().count(),
        file_path
    ))
}

pub(in super::super) fn edit_workspace_file(
    file_path: &str,
    old_string: &str,
    new_string: &str,
) -> Result<String, String> {
    let workspace_root = workspace_root_path("edit_file")?;
    edit_workspace_file_from_root(&workspace_root, file_path, old_string, new_string)
}

pub(in super::super) fn edit_workspace_file_from_root(
    workspace_root: &Path,
    file_path: &str,
    old_string: &str,
    new_string: &str,
) -> Result<String, String> {
    let target = resolve_existing_workspace_file(workspace_root, file_path, "edit_file")?;
    let content = fs::read_to_string(&target)
        .map_err(|error| format!("edit_file failed to read {file_path}: {error}"))?;
    let normalized_content = normalize_line_endings(&content);
    let normalized_old_string = normalize_line_endings(old_string);

    let updated = apply_single_edit(
        &normalized_content,
        &normalized_old_string,
        new_string,
        file_path,
    )?;

    fs::write(&target, updated)
        .map_err(|error| format!("edit_file failed to write {file_path}: {error}"))?;

    Ok(format!("Edited {}", file_path))
}

pub(in super::super) fn multi_edit_workspace_file(
    file_path: &str,
    edits: &[FileEditOperation],
) -> Result<String, String> {
    let workspace_root = workspace_root_path("multi_edit")?;
    multi_edit_workspace_file_from_root(&workspace_root, file_path, edits)
}

pub(in super::super) fn multi_edit_workspace_file_from_root(
    workspace_root: &Path,
    file_path: &str,
    edits: &[FileEditOperation],
) -> Result<String, String> {
    if edits.is_empty() {
        return Err("No edits provided".to_string());
    }

    let target = resolve_existing_workspace_file(workspace_root, file_path, "multi_edit")?;
    let content = fs::read_to_string(&target)
        .map_err(|error| format!("multi_edit failed to read {file_path}: {error}"))?;
    let mut dry_run = normalize_line_endings(&content);

    for (index, edit) in edits.iter().enumerate() {
        let normalized_old_string = normalize_line_endings(&edit.old_string);
        dry_run = apply_edit_for_multi(
            &dry_run,
            &normalized_old_string,
            &edit.new_string,
            file_path,
            index,
            edits.len(),
        )?;
    }

    fs::write(&target, dry_run)
        .map_err(|error| format!("multi_edit failed to write {file_path}: {error}"))?;

    Ok(format!("Applied {} edit(s) to {}", edits.len(), file_path))
}

fn normalize_line_endings(input: &str) -> String {
    input.replace("\r\n", "\n")
}

fn unescape_over_escaped(input: &str) -> String {
    input
        .replace("\\\\n", "__BACKSLASH_N__")
        .replace("\\\\t", "__BACKSLASH_T__")
        .replace("\\\\r", "__BACKSLASH_R__")
        .replace("\\\\f", "__BACKSLASH_F__")
        .replace("\\\\v", "__BACKSLASH_V__")
        .replace("\\\\\"", "__BACKSLASH_DQ__")
        .replace("\\\\'", "__BACKSLASH_SQ__")
        .replace("\\\\`", "__BACKSLASH_BT__")
        .replace("\\n", "\n")
        .replace("\\t", "\t")
        .replace("\\r", "\r")
        .replace("\\f", "\u{000C}")
        .replace("\\v", "\u{000B}")
        .replace("\\\"", "\"")
        .replace("\\'", "'")
        .replace("\\`", "`")
        .replace("__BACKSLASH_N__", "\\n")
        .replace("__BACKSLASH_T__", "\\t")
        .replace("__BACKSLASH_R__", "\\r")
        .replace("__BACKSLASH_F__", "\\f")
        .replace("__BACKSLASH_V__", "\\v")
        .replace("__BACKSLASH_DQ__", "\\\"")
        .replace("__BACKSLASH_SQ__", "\\'")
        .replace("__BACKSLASH_BT__", "\\`")
}

fn apply_single_edit(
    content: &str,
    old_string: &str,
    new_string: &str,
    file_path: &str,
) -> Result<String, String> {
    let occurrences = count_occurrences(content, old_string);
    if occurrences > 1 {
        return Err(format!(
            "old_string matches {} locations in {}. Provide more context to disambiguate.",
            occurrences, file_path
        ));
    }

    if occurrences == 1 {
        return Ok(content.replacen(old_string, new_string, 1));
    }

    let unescaped = unescape_over_escaped(old_string);
    if unescaped != old_string && content.contains(&unescaped) {
        let unescaped_occurrences = count_occurrences(content, &unescaped);
        if unescaped_occurrences == 1 {
            return Ok(content.replacen(&unescaped, new_string, 1));
        }
        if unescaped_occurrences > 1 {
            return Err(format!(
                "old_string (after fixing escaping) matches {} locations in {}. Provide more context to disambiguate.",
                unescaped_occurrences, file_path
            ));
        }
    }

    Err(build_not_found_error(file_path, old_string, content))
}

fn apply_edit_for_multi(
    content: &str,
    old_string: &str,
    new_string: &str,
    file_path: &str,
    index: usize,
    total: usize,
) -> Result<String, String> {
    if content.contains(old_string) {
        let occurrences = count_occurrences(content, old_string);
        if occurrences > 1 {
            return Err(format!(
                "Edit {}/{}: old_string matches {} locations in {}. Provide more context to disambiguate.",
                index + 1,
                total,
                occurrences,
                file_path
            ));
        }
        return Ok(content.replacen(old_string, new_string, 1));
    }

    let unescaped = unescape_over_escaped(old_string);
    if unescaped != old_string && content.contains(&unescaped) {
        let occurrences = count_occurrences(content, &unescaped);
        if occurrences > 1 {
            return Err(format!(
                "Edit {}/{}: old_string matches {} locations in {}. Provide more context to disambiguate.",
                index + 1,
                total,
                occurrences,
                file_path
            ));
        }
        return Ok(content.replacen(&unescaped, new_string, 1));
    }

    Err(format!(
        "Edit {}/{}: {}",
        index + 1,
        total,
        build_not_found_error(file_path, old_string, content)
    ))
}

fn count_occurrences(content: &str, needle: &str) -> usize {
    if needle.is_empty() {
        0
    } else {
        content.matches(needle).count()
    }
}

fn build_not_found_error(file_path: &str, old_string: &str, file_content: &str) -> String {
    if has_smart_quote_mismatch(old_string, file_content) {
        return format!(
            "old_string not found in {}. The file uses smart/curly quotes but old_string has straight quotes. Re-read the file and copy the exact characters.",
            file_path
        );
    }

    if has_whitespace_mismatch(old_string, file_content) {
        return format!(
            "old_string not found in {}. Found a near-match with different whitespace or indentation. Re-read the file for exact content.",
            file_path
        );
    }

    format!(
        "old_string not found in {}. The file may have changed -- re-read it and try again.",
        file_path
    )
}

fn has_smart_quote_mismatch(search: &str, content: &str) -> bool {
    if !search.contains('"') && !search.contains('\'') {
        return false;
    }

    let normalized_search = normalize_quotes(search);
    let normalized_content = normalize_quotes(content);
    normalized_content.contains(&normalized_search)
        && content
            .chars()
            .any(|character| matches!(character, '\u{2018}' | '\u{2019}' | '\u{201C}' | '\u{201D}'))
}

fn has_whitespace_mismatch(search: &str, content: &str) -> bool {
    let collapsed_search = collapse_whitespace(search);
    if collapsed_search.chars().count() < 10 {
        return false;
    }
    collapse_whitespace(content).contains(&collapsed_search)
}

fn normalize_quotes(input: &str) -> String {
    input
        .replace(['\u{2018}', '\u{2019}'], "'")
        .replace(['\u{201C}', '\u{201D}'], "\"")
}

fn collapse_whitespace(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}
