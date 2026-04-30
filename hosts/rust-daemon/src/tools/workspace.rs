use std::fs;
use std::path::{Path, PathBuf};

pub(super) fn workspace_root_path(tool_name: &str) -> Result<PathBuf, String> {
    match std::env::var("ANIMAOS_WORKSPACE_ROOT") {
        Ok(value) if !value.trim().is_empty() => Ok(PathBuf::from(value)),
        Ok(_) => Err(format!("{tool_name} workspace root is empty")),
        Err(std::env::VarError::NotPresent) => std::env::current_dir()
            .map_err(|error| format!("{tool_name} workspace root could not be determined: {error}")),
        Err(error) => Err(format!("{tool_name} workspace root could not be read: {error}")),
    }
}

pub(super) fn canonical_workspace_root(
    workspace_root: &Path,
    tool_name: &str,
) -> Result<PathBuf, String> {
    workspace_root
        .canonicalize()
        .map_err(|error| format!("{tool_name} workspace root could not be resolved: {error}"))
}

pub(super) fn resolve_input_path(workspace_root: &Path, user_path: &str) -> PathBuf {
    let path = PathBuf::from(user_path);
    if path.is_absolute() {
        path
    } else {
        workspace_root.join(path)
    }
}

pub(super) fn ensure_path_within_workspace(
    workspace_root: &Path,
    candidate: &Path,
    tool_name: &str,
    user_path: &str,
) -> Result<(), String> {
    if candidate.starts_with(workspace_root) {
        Ok(())
    } else {
        Err(format!("{tool_name} path escapes workspace root: {user_path}"))
    }
}

pub(super) fn resolve_workspace_search_root(
    workspace_root: &Path,
    path: &str,
    tool_name: &str,
) -> Result<PathBuf, String> {
    let search_path = resolve_workspace_search_path(workspace_root, path, tool_name)?;
    if search_path.is_dir() {
        Ok(search_path)
    } else {
        Err(format!("Directory not found: {path}"))
    }
}

pub(super) fn resolve_workspace_search_path(
    workspace_root: &Path,
    path: &str,
    tool_name: &str,
) -> Result<PathBuf, String> {
    let resolved = resolve_input_path(workspace_root, path);
    if !resolved.exists() {
        return Err(format!("Search path not found: {path}"));
    }

    let canonical = resolved
        .canonicalize()
        .map_err(|error| format!("{tool_name} path could not be resolved: {path} ({error})"))?;
    ensure_path_within_workspace(workspace_root, &canonical, tool_name, path)?;
    Ok(canonical)
}

pub(super) fn walk_search_tree<F>(current_dir: &Path, on_file: &mut F) -> Result<(), String>
where
    F: FnMut(&Path) -> Result<(), String>,
{
    let mut entries = fs::read_dir(current_dir)
        .map_err(|error| format!("search walk failed for {}: {error}", current_dir.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("search walk failed for {}: {error}", current_dir.display()))?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }

        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        let path = entry.path();

        if file_type.is_dir() {
            walk_search_tree(&path, on_file)?;
        } else if file_type.is_file() {
            on_file(&path)?;
        }
    }

    Ok(())
}

pub(super) fn normalized_relative_path(base_root: &Path, path: &Path) -> Result<String, String> {
    let relative = path
        .strip_prefix(base_root)
        .map_err(|error| format!("path could not be normalized: {} ({error})", path.display()))?;
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

pub(super) fn resolve_existing_workspace_file(
    workspace_root: &Path,
    file_path: &str,
    tool_name: &str,
) -> Result<PathBuf, String> {
    let canonical_root = canonical_workspace_root(workspace_root, tool_name)?;
    let resolved = resolve_input_path(&canonical_root, file_path);
    if !resolved.exists() {
        return Err(format!("File not found: {file_path}"));
    }

    let canonical = resolved
        .canonicalize()
        .map_err(|error| format!("{tool_name} path could not be resolved: {file_path} ({error})"))?;
    ensure_path_within_workspace(&canonical_root, &canonical, tool_name, file_path)?;

    if !fs::metadata(&canonical)
        .map_err(|error| format!("{tool_name} failed to inspect {file_path}: {error}"))?
        .is_file()
    {
        return Err(format!("File not found: {file_path}"));
    }

    Ok(canonical)
}

pub(super) fn resolve_workspace_write_path(
    workspace_root: &Path,
    file_path: &str,
    tool_name: &str,
) -> Result<PathBuf, String> {
    let canonical_root = canonical_workspace_root(workspace_root, tool_name)?;
    let resolved = resolve_input_path(&canonical_root, file_path);
    ensure_write_path_within_workspace(&canonical_root, &resolved, tool_name, file_path)?;
    Ok(resolved)
}

fn ensure_write_path_within_workspace(
    workspace_root: &Path,
    candidate: &Path,
    tool_name: &str,
    user_path: &str,
) -> Result<(), String> {
    let mut probe = candidate;

    while !probe.exists() {
        probe = probe
            .parent()
            .ok_or_else(|| format!("{tool_name} path escapes workspace root: {user_path}"))?;
    }

    let canonical_probe = probe
        .canonicalize()
        .map_err(|error| format!("{tool_name} path could not be resolved: {user_path} ({error})"))?;
    ensure_path_within_workspace(workspace_root, &canonical_probe, tool_name, user_path)
}
