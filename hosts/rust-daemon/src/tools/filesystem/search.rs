use std::fs;
use std::path::Path;

use regex::Regex;

use super::super::workspace::{
    canonical_workspace_root, ensure_path_within_workspace, normalized_relative_path,
    resolve_input_path, resolve_workspace_search_path, resolve_workspace_search_root,
    walk_search_tree, workspace_root_path,
};

pub(in super::super) fn read_workspace_file(file_path: &str, offset: usize, limit: usize) -> Result<String, String> {
    let workspace_root = workspace_root_path("read_file")?;
    read_workspace_file_from_root(&workspace_root, file_path, offset, limit)
}

pub(in super::super) fn read_workspace_file_from_root(
    workspace_root: &Path,
    file_path: &str,
    offset: usize,
    limit: usize,
) -> Result<String, String> {
    let canonical_root = canonical_workspace_root(workspace_root, "read_file")?;
    let resolved = resolve_input_path(&canonical_root, file_path);
    if !resolved.exists() {
        return Err(format!("File not found: {file_path}"));
    }

    let canonical = resolved
        .canonicalize()
        .map_err(|error| format!("read_file path could not be resolved: {file_path} ({error})"))?;
    ensure_path_within_workspace(&canonical_root, &canonical, "read_file", file_path)?;

    if !fs::metadata(&canonical)
        .map_err(|error| format!("read_file failed to inspect {file_path}: {error}"))?
        .is_file()
    {
        return Err(format!("File not found: {file_path}"));
    }

    let content = fs::read_to_string(&canonical)
        .map_err(|error| format!("read_file failed to read {file_path}: {error}"))?;
    let lines = content.split('\n').collect::<Vec<_>>();
    let numbered = lines
        .iter()
        .skip(offset)
        .take(limit)
        .enumerate()
        .map(|(index, line)| format!("{:>6}| {}", offset + index + 1, line))
        .collect::<Vec<_>>();

    Ok(numbered.join("\n"))
}

pub(in super::super) fn list_workspace_dir(path: &str) -> Result<String, String> {
    let workspace_root = workspace_root_path("list_dir")?;
    list_workspace_dir_from_root(&workspace_root, path)
}

pub(in super::super) fn list_workspace_dir_from_root(
    workspace_root: &Path,
    path: &str,
) -> Result<String, String> {
    let canonical_root = canonical_workspace_root(workspace_root, "list_dir")?;
    let resolved = resolve_input_path(&canonical_root, path);
    if !resolved.exists() {
        return Err(format!("Directory not found: {path}"));
    }

    let canonical = resolved
        .canonicalize()
        .map_err(|error| format!("list_dir path could not be resolved: {path} ({error})"))?;
    ensure_path_within_workspace(&canonical_root, &canonical, "list_dir", path)?;

    if !fs::metadata(&canonical)
        .map_err(|error| format!("list_dir failed to inspect {path}: {error}"))?
        .is_dir()
    {
        return Err(format!("Directory not found: {path}"));
    }

    let mut entries = fs::read_dir(&canonical)
        .map_err(|error| format!("list_dir failed to read {path}: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("list_dir failed to read {path}: {error}"))?;

    entries.sort_by_key(|entry| entry.file_name());

    let lines = entries
        .into_iter()
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let prefix = match entry.file_type() {
                Ok(file_type) if file_type.is_dir() => "[dir]  ",
                Ok(_) => "[file] ",
                Err(_) => "[file] ",
            };
            format!("{prefix}{name}")
        })
        .collect::<Vec<_>>();

    Ok(lines.join("\n"))
}

pub(in super::super) fn glob_workspace_paths(pattern: &str, path: &str) -> Result<String, String> {
    let workspace_root = workspace_root_path("glob")?;
    glob_workspace_paths_from_root(&workspace_root, pattern, path)
}

pub(in super::super) fn glob_workspace_paths_from_root(
    workspace_root: &Path,
    pattern: &str,
    path: &str,
) -> Result<String, String> {
    let canonical_root = canonical_workspace_root(workspace_root, "glob")?;
    let search_root = resolve_workspace_search_root(&canonical_root, path, "glob")?;
    let matcher = compile_glob_matcher(pattern)
        .map_err(|error| format!("glob pattern is invalid: {error}"))?;
    let mut matches = Vec::new();

    walk_search_tree(&search_root, &mut |file_path| {
        let relative_path = normalized_relative_path(&canonical_root, file_path)?;
        if glob_matches_path(pattern, &matcher, &relative_path) {
            matches.push(relative_path);
        }
        Ok(())
    })?;

    matches.sort();

    if matches.is_empty() {
        Ok("No files found".to_string())
    } else {
        Ok(matches.join("\n"))
    }
}

pub(in super::super) fn grep_workspace_files(pattern: &str, path: &str, include: Option<&str>) -> Result<String, String> {
    let workspace_root = workspace_root_path("grep")?;
    grep_workspace_files_from_root(&workspace_root, pattern, path, include)
}

pub(in super::super) fn grep_workspace_files_from_root(
    workspace_root: &Path,
    pattern: &str,
    path: &str,
    include: Option<&str>,
) -> Result<String, String> {
    let canonical_root = canonical_workspace_root(workspace_root, "grep")?;
    let search_path = resolve_workspace_search_path(&canonical_root, path, "grep")?;
    let matcher =
        Regex::new(pattern).map_err(|error| format!("grep pattern is not valid regex: {error}"))?;
    let include_matcher = include
        .map(compile_glob_matcher)
        .transpose()
        .map_err(|error| format!("grep include glob is invalid: {error}"))?;
    let include_pattern = include.unwrap_or_default();
    let mut matches = Vec::new();

    if search_path.is_file() {
        grep_single_file(
            &canonical_root,
            &search_path,
            &matcher,
            include_pattern,
            include_matcher.as_ref(),
            &mut matches,
        )?;
    } else {
        walk_search_tree(&search_path, &mut |file_path| {
            grep_single_file(
                &canonical_root,
                file_path,
                &matcher,
                include_pattern,
                include_matcher.as_ref(),
                &mut matches,
            )
        })?;
    }

    if matches.is_empty() {
        Ok("No matches found".to_string())
    } else {
        Ok(truncate_chars(&matches.join("\n"), 50_000))
    }
}

fn grep_single_file(
    workspace_root: &Path,
    file_path: &Path,
    matcher: &Regex,
    include_pattern: &str,
    include_matcher: Option<&Regex>,
    matches: &mut Vec<String>,
) -> Result<(), String> {
    let relative_path = normalized_relative_path(workspace_root, file_path)?;
    if !include_pattern.is_empty() && !path_matches_glob(include_pattern, include_matcher, &relative_path) {
        return Ok(());
    }

    let content = match fs::read_to_string(file_path) {
        Ok(content) => content,
        Err(_) => return Ok(()),
    };

    for (index, line) in content.lines().enumerate() {
        if matcher.is_match(line) {
            matches.push(format!("{}:{}:{}", relative_path, index + 1, line));
        }
    }

    Ok(())
}

pub(in super::super) fn compile_glob_matcher(pattern: &str) -> Result<Regex, regex::Error> {
    let mut regex = String::from("^");
    let chars = pattern.chars().collect::<Vec<_>>();
    let mut index = 0;

    while index < chars.len() {
        match chars[index] {
            '*' => {
                if chars.get(index + 1) == Some(&'*') {
                    index += 1;
                    if chars.get(index + 1) == Some(&'/') {
                        index += 1;
                        regex.push_str("(?:.*/)?");
                    } else {
                        regex.push_str(".*");
                    }
                } else {
                    regex.push_str("[^/]*");
                }
            }
            '?' => regex.push_str("[^/]"),
            '\\' | '.' | '+' | '(' | ')' | '|' | '^' | '$' | '{' | '}' | '[' | ']' => {
                regex.push('\\');
                regex.push(chars[index]);
            }
            '/' => regex.push('/'),
            character => regex.push(character),
        }

        index += 1;
    }

    regex.push('$');
    Regex::new(&regex)
}

fn glob_matches_path(pattern: &str, matcher: &Regex, relative_path: &str) -> bool {
    path_matches_glob(pattern, Some(matcher), relative_path)
}

fn path_matches_glob(pattern: &str, matcher: Option<&Regex>, relative_path: &str) -> bool {
    let Some(matcher) = matcher else {
        return true;
    };

    if matcher.is_match(relative_path) {
        return true;
    }

    if pattern.contains('/') {
        return false;
    }

    Path::new(relative_path)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|file_name| matcher.is_match(file_name))
}

fn truncate_chars(input: &str, max_characters: usize) -> String {
    if input.chars().count() <= max_characters {
        return input.to_string();
    }

    format!("{}...", input.chars().take(max_characters).collect::<String>())
}