use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use super::super::workspace::{
    canonical_workspace_root, resolve_workspace_search_root, workspace_root_path,
};
use super::shell::{resolve_shell_launcher, truncate_shell_output};

const MAX_PROCESS_OUTPUT_LINES: usize = 2_000;
pub(crate) const DEFAULT_MAX_BACKGROUND_PROCESSES: usize = 8;

pub(crate) type SharedProcessManager = Arc<Mutex<ProcessManager>>;

#[derive(Default)]
pub(crate) struct ProcessManager {
    processes: HashMap<String, ManagedProcess>,
    next_id: u64,
    max_running_processes: usize,
}

struct ManagedProcess {
    id: String,
    command: String,
    child: Child,
    output_state: Arc<Mutex<ManagedProcessOutput>>,
    started_at: Instant,
}

#[derive(Default)]
struct ManagedProcessOutput {
    lines: Vec<String>,
    output_cursor: usize,
    exit_code: Option<i32>,
}

#[cfg(test)]
pub(crate) fn new_shared_process_manager() -> SharedProcessManager {
    new_shared_process_manager_with_limit(DEFAULT_MAX_BACKGROUND_PROCESSES)
}

pub(crate) fn new_shared_process_manager_with_limit(
    max_running_processes: usize,
) -> SharedProcessManager {
    Arc::new(Mutex::new(ProcessManager::new(max_running_processes)))
}

#[cfg(test)]
pub(crate) fn set_background_process_limit(
    process_manager: &SharedProcessManager,
    max_running_processes: usize,
) -> Result<(), String> {
    let mut manager = process_manager
        .lock()
        .map_err(|_| "background process manager lock poisoned".to_string())?;
    manager.max_running_processes = max_running_processes;
    Ok(())
}

pub(crate) fn background_process_count(
    process_manager: &SharedProcessManager,
) -> Result<usize, String> {
    let mut manager = process_manager
        .lock()
        .map_err(|_| "background process manager lock poisoned".to_string())?;
    running_process_count(&mut manager)
}

impl ProcessManager {
    fn new(max_running_processes: usize) -> Self {
        Self {
            processes: HashMap::new(),
            next_id: 1,
            max_running_processes,
        }
    }
}

impl Drop for ProcessManager {
    fn drop(&mut self) {
        for managed in self.processes.values_mut() {
            let _ = managed.child.kill();
            let _ = managed.child.wait();
        }
    }
}

pub(super) fn start_background_process(
    process_manager: &SharedProcessManager,
    command: &str,
    cwd: &str,
) -> Result<String, String> {
    let workspace_root = workspace_root_path("bg_start")?;
    start_background_process_from_root(process_manager, &workspace_root, command, cwd)
}

pub(in super::super) fn start_background_process_from_root(
    process_manager: &SharedProcessManager,
    workspace_root: &Path,
    command: &str,
    cwd: &str,
) -> Result<String, String> {
    let cwd_path = resolve_workspace_search_root(
        &canonical_workspace_root(workspace_root, "bg_start")?,
        cwd,
        "bg_start",
    )?;

    let mut manager = process_manager
        .lock()
        .map_err(|_| "background process manager lock poisoned".to_string())?;
    let running_processes = running_process_count(&mut manager)?;
    if running_processes >= manager.max_running_processes {
        return Err(format!(
            "bg_start limit reached: {} running background processes (max {}).",
            running_processes, manager.max_running_processes
        ));
    }

    let (executable, flags) = resolve_shell_launcher()?;

    let mut child = Command::new(&executable)
        .args(&flags)
        .arg(command)
        .current_dir(&cwd_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("bg_start failed to start command: {error}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "bg_start stdout could not be captured".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "bg_start stderr could not be captured".to_string())?;

    let output_state = Arc::new(Mutex::new(ManagedProcessOutput::default()));
    spawn_process_output_reader(stdout, Arc::clone(&output_state), false);
    spawn_process_output_reader(stderr, Arc::clone(&output_state), true);

    let id = format!("bg-{}", manager.next_id);
    manager.next_id += 1;
    manager.processes.insert(
        id.clone(),
        ManagedProcess {
            id: id.clone(),
            command: command.to_string(),
            child,
            output_state,
            started_at: Instant::now(),
        },
    );

    Ok(format!(
        "Started background process {}: {}\nUse bg_output(id: \"{}\") to read output, bg_stop(id: \"{}\") to kill.",
        id, command, id, id
    ))
}

pub(in super::super) fn read_background_process_output(
    process_manager: &SharedProcessManager,
    id: &str,
    all: bool,
) -> Result<String, String> {
    let mut manager = process_manager
        .lock()
        .map_err(|_| "background process manager lock poisoned".to_string())?;
    let managed = manager.processes.get_mut(id).ok_or_else(|| {
        format!(
            "No background process with id \"{}\". Use bg_list to see active processes.",
            id
        )
    })?;

    sync_managed_process_exit(managed)?;

    let mut output_state = managed
        .output_state
        .lock()
        .map_err(|_| "background process output lock poisoned".to_string())?;
    let lines = if all {
        output_state.lines.clone()
    } else {
        output_state.lines[output_state.output_cursor..].to_vec()
    };
    output_state.output_cursor = output_state.lines.len();
    let exit_code = output_state.exit_code;

    if lines.is_empty() {
        return Ok(if exit_code.is_none() {
            format!("[{}] No new output. Process still running.", id)
        } else {
            format!(
                "[{}] No new output. Process exited with code {}.",
                id,
                exit_code.unwrap_or(-1)
            )
        });
    }

    let raw = lines.join("\n");
    let status = if exit_code.is_none() {
        "(running)".to_string()
    } else {
        format!("(exited: {})", exit_code.unwrap_or(-1))
    };

    Ok(format!(
        "[{}] {}\n{}",
        id,
        status,
        truncate_shell_output(&raw)
    ))
}

pub(in super::super) fn stop_background_process(
    process_manager: &SharedProcessManager,
    id: &str,
) -> Result<String, String> {
    let mut manager = process_manager
        .lock()
        .map_err(|_| "background process manager lock poisoned".to_string())?;
    let mut managed = manager
        .processes
        .remove(id)
        .ok_or_else(|| format!("No background process with id \"{}\".", id))?;

    sync_managed_process_exit(&mut managed)?;
    let exit_code = managed
        .output_state
        .lock()
        .map_err(|_| "background process output lock poisoned".to_string())?
        .exit_code;

    if exit_code.is_none() {
        managed
            .child
            .kill()
            .map_err(|error| format!("bg_stop failed to kill {}: {error}", id))?;
        let status = managed
            .child
            .wait()
            .map_err(|error| format!("bg_stop failed while waiting for {}: {error}", id))?;
        if let Ok(mut output_state) = managed.output_state.lock() {
            output_state.exit_code = status.code();
        }
    }

    Ok(format!("Stopped and removed {}.", id))
}

pub(in super::super) fn list_background_processes(
    process_manager: &SharedProcessManager,
) -> Result<String, String> {
    let mut manager = process_manager
        .lock()
        .map_err(|_| "background process manager lock poisoned".to_string())?;

    if manager.processes.is_empty() {
        return Ok("No background processes running.".to_string());
    }

    let mut lines = Vec::new();
    for managed in manager.processes.values_mut() {
        sync_managed_process_exit(managed)?;
        let exit_code = managed
            .output_state
            .lock()
            .map_err(|_| "background process output lock poisoned".to_string())?
            .exit_code;
        let elapsed = managed.started_at.elapsed().as_secs();
        let status = if let Some(code) = exit_code {
            format!("exited({})", code)
        } else {
            "running".to_string()
        };
        lines.push(format!(
            "{}  {}  {}s  {}",
            managed.id, status, elapsed, managed.command
        ));
    }
    lines.sort();

    Ok(lines.join("\n"))
}

fn running_process_count(manager: &mut ProcessManager) -> Result<usize, String> {
    let mut running_processes = 0;
    for managed in manager.processes.values_mut() {
        sync_managed_process_exit(managed)?;
        let exit_code = managed
            .output_state
            .lock()
            .map_err(|_| "background process output lock poisoned".to_string())?
            .exit_code;
        if exit_code.is_none() {
            running_processes += 1;
        }
    }

    Ok(running_processes)
}

fn sync_managed_process_exit(managed: &mut ManagedProcess) -> Result<(), String> {
    let exit_code = managed
        .child
        .try_wait()
        .map_err(|error| format!("background process status check failed: {error}"))?
        .and_then(|status| status.code());

    if let Some(code) = exit_code {
        let mut output_state = managed
            .output_state
            .lock()
            .map_err(|_| "background process output lock poisoned".to_string())?;
        output_state.exit_code = Some(code);
    }

    Ok(())
}

fn spawn_process_output_reader<R>(
    reader: R,
    output_state: Arc<Mutex<ManagedProcessOutput>>,
    stderr: bool,
) where
    R: std::io::Read + Send + 'static,
{
    thread::spawn(move || {
        let reader = BufReader::new(reader);
        for line in reader.lines() {
            let Ok(line) = line else {
                break;
            };
            let rendered = if stderr {
                format!("[stderr] {}", line)
            } else {
                line
            };

            let Ok(mut output) = output_state.lock() else {
                break;
            };
            if output.lines.len() >= MAX_PROCESS_OUTPUT_LINES {
                output.lines.remove(0);
                if output.output_cursor > 0 {
                    output.output_cursor -= 1;
                }
            }
            output.lines.push(rendered);
        }
    });
}
