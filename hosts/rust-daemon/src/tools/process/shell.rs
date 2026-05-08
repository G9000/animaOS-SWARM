use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::super::workspace::{
    canonical_workspace_root, resolve_workspace_search_root, workspace_root_path,
};

const BASH_MAX_OUTPUT_CHARS: usize = 30_000;
const BASH_MAX_OUTPUT_LINES: usize = 500;
/// Hard cap on bytes captured per stream during process execution. Prevents a
/// runaway subprocess from OOM-ing the daemon before `truncate_shell_output`
/// runs at the end. Set comfortably above `BASH_MAX_OUTPUT_CHARS` so genuine
/// long output still survives post-truncation.
const BASH_MAX_CAPTURE_BYTES: usize = 256 * 1024;

/// Environment variables that are safe and necessary to pass through to the
/// shell tool subprocess. Anything not on this list — including LLM provider
/// API keys, DATABASE_URL, etc. — is stripped via `Command::env_clear`.
const SHELL_ENV_ALLOWLIST: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "USERNAME",
    "LOGNAME",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TERM",
    "TZ",
    "TMPDIR",
    "TEMP",
    "TMP",
    "SHELL",
    "PWD",
    "OLDPWD",
    // Windows-specific
    "SystemRoot",
    "SystemDrive",
    "ComSpec",
    "PATHEXT",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "ProgramFiles",
    "ProgramFiles(x86)",
    "ProgramData",
    "windir",
    "OS",
    "PROCESSOR_ARCHITECTURE",
    "NUMBER_OF_PROCESSORS",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in super::super) struct BashCommandResult {
    pub(in super::super) status: &'static str,
    pub(in super::super) output: String,
}

pub(super) fn execute_bash_command(
    command: &str,
    timeout_ms: u64,
    cwd: &str,
) -> Result<BashCommandResult, String> {
    let workspace_root = workspace_root_path("bash")?;
    execute_bash_command_from_root(&workspace_root, command, timeout_ms, cwd)
}

pub(in super::super) fn execute_bash_command_from_root(
    workspace_root: &Path,
    command: &str,
    timeout_ms: u64,
    cwd: &str,
) -> Result<BashCommandResult, String> {
    let cwd = resolve_workspace_search_root(
        &canonical_workspace_root(workspace_root, "bash")?,
        cwd,
        "bash",
    )?;
    let (executable, flags) = resolve_shell_launcher()?;

    let mut builder = Command::new(&executable);
    builder
        .args(&flags)
        .arg(command)
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_clear();
    for name in SHELL_ENV_ALLOWLIST {
        if let Ok(value) = std::env::var(name) {
            builder.env(name, value);
        }
    }
    let mut child = builder
        .spawn()
        .map_err(|error| format!("bash failed to start command: {error}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "bash stdout could not be captured".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "bash stderr could not be captured".to_string())?;

    let stdout_buffer = Arc::new(Mutex::new(String::new()));
    let stderr_buffer = Arc::new(Mutex::new(String::new()));

    let stdout_thread = spawn_output_capture(stdout, Arc::clone(&stdout_buffer), false);
    let stderr_thread = spawn_output_capture(stderr, Arc::clone(&stderr_buffer), true);

    let start = Instant::now();
    let exit_status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("bash failed while waiting for command: {error}"))?
        {
            break Some(status);
        }

        if start.elapsed() >= Duration::from_millis(timeout_ms) {
            child
                .kill()
                .map_err(|error| format!("bash failed to stop timed out command: {error}"))?;
            let _ = child.wait();
            break None;
        }

        thread::sleep(Duration::from_millis(10));
    };

    let _ = stdout_thread.join();
    let _ = stderr_thread.join();

    let stdout_text = stdout_buffer
        .lock()
        .map_err(|_| "bash stdout lock poisoned".to_string())?
        .clone();
    let stderr_text = stderr_buffer
        .lock()
        .map_err(|_| "bash stderr lock poisoned".to_string())?
        .clone();

    if exit_status.is_none() {
        return Ok(BashCommandResult {
            status: "error",
            output: format!("Command timed out after {timeout_ms}ms"),
        });
    }

    let exit_code = exit_status.and_then(|status| status.code()).unwrap_or(-1);
    let combined = if stdout_text.trim().is_empty() {
        stderr_text.clone()
    } else if stderr_text.trim().is_empty() {
        stdout_text.clone()
    } else {
        format!("{}\n{}", stdout_text.trim_end(), stderr_text.trim_end())
    };

    Ok(BashCommandResult {
        status: if exit_code == 0 { "success" } else { "error" },
        output: truncate_shell_output(&combined),
    })
}

pub(in super::super) fn truncate_shell_output(text: &str) -> String {
    truncate_text_with_limits(text, BASH_MAX_OUTPUT_CHARS, BASH_MAX_OUTPUT_LINES)
}

fn truncate_text_with_limits(text: &str, max_chars: usize, max_lines: usize) -> String {
    let mut lines = text.lines().map(ToString::to_string).collect::<Vec<_>>();
    if lines.len() > max_lines {
        let head = max_lines / 2;
        let tail = max_lines - head;
        let omitted = lines.len().saturating_sub(max_lines);
        let mut selected = Vec::new();
        selected.extend(lines.iter().take(head).cloned());
        selected.push(format!("... [{} lines omitted] ...", omitted));
        selected.extend(lines.iter().skip(lines.len().saturating_sub(tail)).cloned());
        lines = selected;
    }

    let content = lines.join("\n");
    if content.chars().count() <= max_chars {
        return content;
    }

    let half = max_chars / 2;
    let start = content.chars().take(half).collect::<String>();
    let end = content
        .chars()
        .rev()
        .take(half)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("{}\n... [output truncated] ...\n{}", start, end)
}

pub(in super::super) fn resolve_shell_launcher() -> Result<(String, Vec<String>), String> {
    let candidates = shell_launcher_candidates();
    let fallback = candidates
        .first()
        .cloned()
        .ok_or_else(|| "No shell candidates available".to_string())?;

    for (executable, flags) in &candidates {
        if shell_candidate_exists(executable) {
            return Ok((executable.clone(), flags.clone()));
        }
    }

    Ok(fallback)
}

fn shell_launcher_candidates() -> Vec<(String, Vec<String>)> {
    // We always invoke with `-c` (non-login, non-interactive) so the shell does
    // not source profile scripts — this keeps the env we curate via the
    // `SHELL_ENV_ALLOWLIST` from being repopulated with operator secrets.
    if cfg!(windows) {
        windows_shell_candidates()
    } else if cfg!(target_os = "macos") {
        vec![
            ("/bin/zsh".to_string(), vec!["-c".to_string()]),
            ("/bin/bash".to_string(), vec!["-c".to_string()]),
        ]
    } else {
        let mut candidates = Vec::new();
        if let Ok(user_shell) = std::env::var("SHELL") {
            if !user_shell.trim().is_empty() {
                candidates.push((user_shell.clone(), shell_flags(&user_shell)));
            }
        }
        candidates.push(("/bin/bash".to_string(), vec!["-c".to_string()]));
        candidates.push(("/usr/bin/bash".to_string(), vec!["-c".to_string()]));
        candidates.push(("/bin/zsh".to_string(), vec!["-c".to_string()]));
        candidates.push(("/bin/sh".to_string(), vec!["-c".to_string()]));
        candidates
    }
}

fn windows_shell_candidates() -> Vec<(String, Vec<String>)> {
    let mut candidates = Vec::new();
    for path in [
        "C:\\Program Files\\Git\\bin\\bash.exe",
        "C:\\Program Files (x86)\\Git\\bin\\bash.exe",
    ] {
        if Path::new(path).exists() {
            candidates.push((path.to_string(), vec!["-c".to_string()]));
        }
    }
    candidates.push(("bash".to_string(), vec!["-c".to_string()]));
    candidates.push((
        "powershell.exe".to_string(),
        vec!["-NoProfile".to_string(), "-Command".to_string()],
    ));
    candidates.push((
        "pwsh".to_string(),
        vec!["-NoProfile".to_string(), "-Command".to_string()],
    ));
    candidates.push((
        std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".to_string()),
        vec!["/d".to_string(), "/s".to_string(), "/c".to_string()],
    ));
    candidates
}

fn shell_flags(_shell: &str) -> Vec<String> {
    // Always non-login: we explicitly do NOT want the shell sourcing rc/profile
    // files that would re-leak operator env into the subprocess.
    vec!["-c".to_string()]
}

fn shell_candidate_exists(executable: &str) -> bool {
    if executable.starts_with('/') || executable.contains(':') {
        return Path::new(executable).exists();
    }

    let probe = if cfg!(windows) { "where" } else { "which" };
    Command::new(probe)
        .arg(executable)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn spawn_output_capture<R>(
    reader: R,
    buffer: Arc<Mutex<String>>,
    stderr: bool,
) -> thread::JoinHandle<()>
where
    R: std::io::Read + Send + 'static,
{
    thread::spawn(move || {
        let mut local = String::new();
        let mut truncated = false;
        let reader = BufReader::new(reader);
        for line in reader.lines() {
            let Ok(line) = line else {
                break;
            };
            if truncated {
                continue;
            }
            let prefix_len = if stderr { "[stderr] ".len() } else { 0 };
            if local.len() + prefix_len + line.len() + 1 > BASH_MAX_CAPTURE_BYTES {
                local.push_str("\n[output capture limit reached; further bytes discarded]\n");
                truncated = true;
                continue;
            }
            if stderr {
                local.push_str("[stderr] ");
            }
            local.push_str(&line);
            local.push('\n');
        }

        if let Ok(mut output) = buffer.lock() {
            output.push_str(&local);
        }
    })
}
