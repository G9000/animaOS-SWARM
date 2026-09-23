# Companion Console M0: Security and Groundwork Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the confirmed workspace write escape, cap the stored event log, record cached and reasoning token usage accurately, and add a verified model table with cost estimation.

**Architecture:** All changes are local to existing modules: the daemon's workspace path helpers, `anima-core`'s runtime and `TokenUsage`, and `anima-model-adapters`' usage parsing plus a new `models.rs`. No new dependencies.

**Tech Stack:** Rust 2021, tokio, axum 0.8 test servers, serde.

**Spec:** `docs/superpowers/specs/2026-09-23-companion-console-design.md` (§14 writer fix, §13.2 event cap, §11.1 usage, §5.1 model table). Master plan: `docs/superpowers/plans/2026-09-23-companion-console.md`.

## Global Constraints

- Master plan Global Constraints apply.
- Disk is tight on this machine: during iteration run only the crate under change with `cargo test -p <crate> <filter>` (shared `target/`). Run `bun x nx run rust-daemon:test --skipNxCache` only for the milestone-end verification, and only with at least 12 GB free (`df -h /System/Volumes/Data`).
- Error strings in this plan are exact; tests assert them.

---

### Task 1: Harden workspace writes

**Files:**
- Modify: `hosts/rust-daemon/src/tools/workspace.rs` (imports; `resolve_workspace_write_path`; new helpers)
- Modify: `hosts/rust-daemon/src/tools/filesystem/edit.rs:1-37` (`write_workspace_file_from_root` delegates)
- Modify: `hosts/rust-daemon/src/tools.rs:34-37` (re-export)
- Test: `hosts/rust-daemon/src/tools/tests.rs` (append after `write_workspace_file_creates_parent_directories`)

**Interfaces:**
- Consumes: `canonical_workspace_root`, `resolve_input_path`, `ensure_path_within_workspace`, `ensure_write_path_within_workspace` (existing, same file).
- Produces: `pub(crate) fn write_workspace_bytes(workspace_root: &Path, file_path: &str, bytes: &[u8], tool_name: &str) -> Result<PathBuf, String>` re-exported as `crate::tools::write_workspace_bytes`; `resolve_workspace_write_path` keeps its signature and now rejects `..` and escaping or dangling final symlinks.

- [ ] **Step 1: Write the failing tests**

Append to `hosts/rust-daemon/src/tools/tests.rs` (after `write_workspace_file_creates_parent_directories`):

```rust
#[test]
fn write_workspace_file_rejects_parent_components() {
    for (index, user_path) in [
        "newdir/../../escaped.txt",
        "../escaped.txt",
        "a/b/../../../escaped.txt",
        "nested/../notes.txt",
    ]
    .into_iter()
    .enumerate()
    {
        let sandbox = create_temp_workspace(&format!("write-parent-{index}"));
        let workspace = sandbox.join("workspace");
        fs::create_dir_all(&workspace).expect("create workspace");

        let error = write_workspace_file_from_root(&workspace, user_path, "escaped")
            .expect_err("parent components must be rejected");

        assert_eq!(
            error,
            format!("write_file path must not contain '..': {user_path}")
        );
        assert!(!sandbox.join("escaped.txt").exists(), "{user_path}");
        assert!(!workspace.join("newdir").exists(), "{user_path}");
        assert!(!workspace.join("a").exists(), "{user_path}");
        assert!(!workspace.join("nested").exists(), "{user_path}");
        fs::remove_dir_all(sandbox).expect("remove sandbox");
    }
}

#[test]
fn resolve_workspace_write_path_rejects_parent_components_for_every_writer() {
    let workspace = create_temp_workspace("write-parent-agency");

    let error = super::resolve_workspace_write_path(&workspace, "../agency", "agency_create")
        .expect_err("agency output must stay inside the workspace");

    assert_eq!(error, "agency_create path must not contain '..': ../agency");
    fs::remove_dir_all(workspace).expect("remove workspace");
}

#[test]
fn write_workspace_bytes_creates_parents_and_returns_the_target() {
    let workspace = create_temp_workspace("write-bytes");

    let target = super::write_workspace_bytes(
        &workspace,
        "uploads/2026-09-23/data.bin",
        &[0, 1, 2],
        "upload",
    )
    .expect("write bytes");

    assert!(target.ends_with("uploads/2026-09-23/data.bin"));
    assert_eq!(fs::read(&target).expect("read bytes"), vec![0, 1, 2]);
    fs::remove_dir_all(workspace).expect("remove workspace");
}

#[cfg(unix)]
#[test]
fn write_workspace_file_rejects_dangling_symlink_target() {
    let sandbox = create_temp_workspace("write-dangling-symlink");
    let workspace = sandbox.join("workspace");
    fs::create_dir_all(&workspace).expect("create workspace");
    let outside = sandbox.join("outside.txt");
    std::os::unix::fs::symlink(&outside, workspace.join("link.txt")).expect("create symlink");

    let error = write_workspace_file_from_root(&workspace, "link.txt", "escaped")
        .expect_err("dangling symlink must be rejected");

    assert_eq!(error, "write_file path is a dangling symbolic link: link.txt");
    assert!(!outside.exists());
    fs::remove_dir_all(sandbox).expect("remove sandbox");
}

#[cfg(unix)]
#[test]
fn write_workspace_file_rejects_symlink_to_outside_file() {
    let sandbox = create_temp_workspace("write-outside-symlink");
    let workspace = sandbox.join("workspace");
    fs::create_dir_all(&workspace).expect("create workspace");
    let outside = sandbox.join("outside.txt");
    fs::write(&outside, "original").expect("write outside file");
    std::os::unix::fs::symlink(&outside, workspace.join("link.txt")).expect("create symlink");

    let error = write_workspace_file_from_root(&workspace, "link.txt", "escaped")
        .expect_err("escaping symlink must be rejected");

    assert_eq!(error, "write_file path escapes workspace root: link.txt");
    assert_eq!(fs::read_to_string(&outside).expect("read outside"), "original");
    fs::remove_dir_all(sandbox).expect("remove sandbox");
}

#[cfg(unix)]
#[test]
fn write_workspace_file_writes_through_symlink_inside_workspace() {
    let workspace = create_temp_workspace("write-inside-symlink");
    fs::write(workspace.join("real.txt"), "original").expect("write real file");
    std::os::unix::fs::symlink(workspace.join("real.txt"), workspace.join("link.txt"))
        .expect("create symlink");

    write_workspace_file_from_root(&workspace, "link.txt", "updated")
        .expect("internal symlink stays writable");

    assert_eq!(
        fs::read_to_string(workspace.join("real.txt")).expect("read real file"),
        "updated"
    );
    fs::remove_dir_all(workspace).expect("remove workspace");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p anima-daemon --lib tools::tests::write_workspace -- --nocapture` and `cargo test -p anima-daemon --lib tools::tests::resolve_workspace_write_path`
Expected: compile error `cannot find function write_workspace_bytes` (first build), then after Step 3's re-export stub the parent-component tests FAIL because the escape write succeeds.

- [ ] **Step 3: Implement the hardened resolver and writer**

In `hosts/rust-daemon/src/tools/workspace.rs`, change the import line to:

```rust
use std::fs;
use std::path::{Component, Path, PathBuf};
```

Replace `resolve_workspace_write_path` and add the helpers below it:

```rust
pub(crate) fn resolve_workspace_write_path(
    workspace_root: &Path,
    file_path: &str,
    tool_name: &str,
) -> Result<PathBuf, String> {
    reject_parent_components(file_path, tool_name)?;
    let canonical_root = canonical_workspace_root(workspace_root, tool_name)?;
    let resolved = resolve_input_path(&canonical_root, file_path);
    ensure_write_path_within_workspace(&canonical_root, &resolved, tool_name, file_path)?;
    ensure_symlink_target_within_workspace(&canonical_root, &resolved, tool_name, file_path)?;
    Ok(resolved)
}

/// Writes bytes to a workspace path with the same checks as `write_file`,
/// re-verifying the parent after directories are created.
pub(crate) fn write_workspace_bytes(
    workspace_root: &Path,
    file_path: &str,
    bytes: &[u8],
    tool_name: &str,
) -> Result<PathBuf, String> {
    let target = resolve_workspace_write_path(workspace_root, file_path, tool_name)?;
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!("{tool_name} failed to create directories for {file_path}: {error}")
        })?;
        let canonical_root = canonical_workspace_root(workspace_root, tool_name)?;
        let canonical_parent = parent.canonicalize().map_err(|error| {
            format!("{tool_name} path could not be resolved: {file_path} ({error})")
        })?;
        ensure_path_within_workspace(&canonical_root, &canonical_parent, tool_name, file_path)?;
    }
    fs::write(&target, bytes)
        .map_err(|error| format!("{tool_name} failed to write {file_path}: {error}"))?;
    Ok(target)
}

fn reject_parent_components(user_path: &str, tool_name: &str) -> Result<(), String> {
    if Path::new(user_path)
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        Err(format!("{tool_name} path must not contain '..': {user_path}"))
    } else {
        Ok(())
    }
}

fn ensure_symlink_target_within_workspace(
    workspace_root: &Path,
    target: &Path,
    tool_name: &str,
    user_path: &str,
) -> Result<(), String> {
    let Ok(metadata) = fs::symlink_metadata(target) else {
        return Ok(());
    };
    if !metadata.file_type().is_symlink() {
        return Ok(());
    }
    let canonical = target
        .canonicalize()
        .map_err(|_| format!("{tool_name} path is a dangling symbolic link: {user_path}"))?;
    ensure_path_within_workspace(workspace_root, &canonical, tool_name, user_path)
}
```

In `hosts/rust-daemon/src/tools/filesystem/edit.rs`, change the workspace import to:

```rust
use super::super::workspace::{
    resolve_existing_workspace_file, workspace_root_path, write_workspace_bytes,
};
```

and replace `write_workspace_file_from_root` with:

```rust
pub(in super::super) fn write_workspace_file_from_root(
    workspace_root: &Path,
    file_path: &str,
    content: &str,
) -> Result<String, String> {
    write_workspace_bytes(workspace_root, file_path, content.as_bytes(), "write_file")?;
    Ok(format!(
        "Wrote {} chars to {}",
        content.chars().count(),
        file_path
    ))
}
```

In `hosts/rust-daemon/src/tools.rs`, extend the re-export:

```rust
pub(crate) use workspace::{
    canonical_workspace_root, normalized_relative_path, resolve_workspace_write_path,
    workspace_root_path, write_workspace_bytes,
};
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p anima-daemon --lib tools::tests`
Expected: all `tools::tests` pass, including the existing `write_workspace_file_creates_parent_directories`. If `write_workspace_bytes` is reported unused outside tests, keep the re-export; it is consumed in M5 and M9 (add `#[allow(unused_imports)]` on the `pub(crate) use` only if the build fails with `-D warnings`).

- [ ] **Step 5: Commit**

```bash
git add hosts/rust-daemon/src/tools/workspace.rs hosts/rust-daemon/src/tools/filesystem/edit.rs hosts/rust-daemon/src/tools.rs hosts/rust-daemon/src/tools/tests.rs
git commit -m "fix(daemon): reject workspace writes that escape through '..' or symlinks"
```

---

### Task 2: Cap the stored event log

**Files:**
- Modify: `packages/core-rust/crates/anima-core/src/runtime.rs` (struct field, constructors, `snapshot`, `events`, `record_event`, new constants)
- Test: `packages/core-rust/crates/anima-core/src/runtime/tests.rs` (append)

**Interfaces:**
- Produces: `pub const MAX_RETAINED_EVENTS: usize = 500;` exported from `anima_core` (add to the crate root re-exports where `MAX_TOOL_ITERATIONS` is exported, if it is; otherwise `pub use runtime::MAX_RETAINED_EVENTS;` in `lib.rs`). `AgentRuntimeSnapshot.event_count` becomes the running total; `events` holds at most the newest 500.

- [ ] **Step 1: Write the failing tests**

Append to `packages/core-rust/crates/anima-core/src/runtime/tests.rs` (add `use crate::events::{EngineEvent, EventType};` to the imports at the top if not present):

```rust
#[test]
fn event_log_retains_newest_events_and_counts_every_event() {
    let mut runtime = runtime();
    let before = runtime.snapshot().event_count;
    let recorded = super::MAX_RETAINED_EVENTS + 100;
    for _ in 0..recorded {
        runtime.record_event(EventType::AgentTokens, DataValue::Null);
    }

    let snapshot = runtime.snapshot();

    assert_eq!(snapshot.events.len(), super::MAX_RETAINED_EVENTS);
    assert_eq!(runtime.events().len(), super::MAX_RETAINED_EVENTS);
    assert_eq!(snapshot.event_count, before + recorded);
    assert_eq!(
        snapshot.events.last().map(|event| &event.id),
        runtime.events().last().map(|event| &event.id)
    );
}

#[test]
fn restoring_an_oversized_snapshot_trims_events_and_keeps_the_total() {
    let mut snapshot = runtime().snapshot();
    snapshot.events = (0..1_000u64)
        .map(|index| EngineEvent {
            id: format!("legacy-{index}"),
            event_type: EventType::AgentTokens,
            agent_id: None,
            timestamp_ms: index,
            data: DataValue::Null,
        })
        .collect();
    snapshot.event_count = 1_000;

    let restored = AgentRuntime::from_snapshot(snapshot, Arc::new(StaticModelAdapter));
    let restored_snapshot = restored.snapshot();

    assert_eq!(restored_snapshot.events.len(), super::MAX_RETAINED_EVENTS);
    assert_eq!(restored_snapshot.events[0].id, "legacy-500");
    assert_eq!(restored_snapshot.event_count, 1_000);
}
```

If `EngineEvent.timestamp_ms` is not `u64`, cast `index` to its type.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p anima-core --lib runtime::tests::event_log_retains runtime::tests::restoring_an_oversized`
Expected: compile error `cannot find value MAX_RETAINED_EVENTS`.

- [ ] **Step 3: Implement the cap**

In `runtime.rs`, next to `pub const MAX_TOOL_ITERATIONS`:

```rust
/// Newest engine events kept in memory and in snapshots; `event_count` keeps the running total.
pub const MAX_RETAINED_EVENTS: usize = 500;
/// Extra events tolerated before trimming, so trimming is amortized.
const EVENT_TRIM_SLACK: usize = 64;
```

Add `event_total: usize,` to `AgentRuntime` after `events`. In `new_with_id` set `event_total: 0,`. In `from_snapshot`:

```rust
        let event_total = snapshot.event_count.max(snapshot.events.len());
        let mut events = snapshot.events;
        if events.len() > MAX_RETAINED_EVENTS {
            events.drain(..events.len() - MAX_RETAINED_EVENTS);
        }
        Self {
            state: snapshot.state,
            messages: snapshot.messages,
            last_task: snapshot.last_task,
            events,
            event_total,
            // remaining fields unchanged
```

In `snapshot()` and `events()` return only the retained window:

```rust
    pub fn snapshot(&self) -> AgentRuntimeSnapshot {
        AgentRuntimeSnapshot {
            state: self.state(),
            message_count: self.messages.len(),
            messages: self.messages.clone(),
            event_count: self.event_total,
            events: self.events().to_vec(),
            last_task: self.last_task.clone(),
            step_count: self.step_counter,
        }
    }

    pub fn events(&self) -> &[EngineEvent] {
        let start = self.events.len().saturating_sub(MAX_RETAINED_EVENTS);
        &self.events[start..]
    }
```

In `record_event`, after `self.events.push(event.clone());`:

```rust
        self.event_total += 1;
        if self.events.len() > MAX_RETAINED_EVENTS + EVENT_TRIM_SLACK {
            let excess = self.events.len() - MAX_RETAINED_EVENTS;
            self.events.drain(..excess);
        }
```

Export the constant from the crate root next to the existing runtime exports in `packages/core-rust/crates/anima-core/src/lib.rs`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p anima-core --lib runtime::tests`
Expected: PASS, including existing event tests (for example `runtime_notifies_event_listener_with_live_events`).

- [ ] **Step 5: Commit**

```bash
git add packages/core-rust/crates/anima-core/src/runtime.rs packages/core-rust/crates/anima-core/src/runtime/tests.rs packages/core-rust/crates/anima-core/src/lib.rs
git commit -m "feat(core): cap retained engine events at 500 while counting all events"
```

---

### Task 3: Record cached and reasoning tokens

**Files:**
- Modify: `packages/core-rust/crates/anima-core/src/agent.rs:116-121` (`TokenUsage`), `packages/core-rust/crates/anima-core/src/runtime.rs:935-939` (`apply_token_usage`)
- Modify: `packages/core-rust/crates/anima-model-adapters/src/common.rs:178-188`, `anthropic.rs:191-200`, `google.rs:244-252`, `stream.rs:102-107,355-417`, `adapter.rs:188-224,316-323`, `chatgpt.rs:228-245`, `ollama.rs:88`
- Modify every other `TokenUsage { .. }` literal (list: `anima-swarm/tests/message_bus.rs`, `anima-swarm/tests/coordinator.rs`, `anima-core/tests/durable_engine_contract.rs`, `anima-core/tests/support/mod.rs`, `anima-core/src/runtime/tests.rs`, `hosts/rust-daemon/src/model.rs`, `hosts/rust-daemon/src/state/swarm_runtime.rs`, `hosts/rust-daemon/src/routes/agents.rs`, `hosts/rust-daemon/src/routes/mod.rs`) by appending `..TokenUsage::default()` (or `..anima_core::TokenUsage::default()`).
- Test: `packages/core-rust/crates/anima-core/src/agent.rs` (new test module), `packages/core-rust/crates/anima-model-adapters/src/tests.rs`, `packages/core-rust/crates/anima-model-adapters/src/chatgpt/tests.rs`

**Interfaces:**
- Produces: `TokenUsage { prompt_tokens, completion_tokens, total_tokens, cached_prompt_tokens, reasoning_tokens }` where cached tokens are included in `prompt_tokens` and reasoning tokens are included in `completion_tokens`; both new fields `#[serde(default)]`.

- [ ] **Step 1: Write the failing tests**

Append to `packages/core-rust/crates/anima-core/src/agent.rs`:

```rust
#[cfg(test)]
mod token_usage_tests {
    use super::TokenUsage;

    #[test]
    fn usage_saved_before_detail_fields_loads_with_zero_details() {
        let usage: TokenUsage = serde_json::from_str(
            r#"{"prompt_tokens":3,"completion_tokens":4,"total_tokens":7}"#,
        )
        .expect("legacy usage deserializes");

        assert_eq!(
            usage,
            TokenUsage {
                prompt_tokens: 3,
                completion_tokens: 4,
                total_tokens: 7,
                cached_prompt_tokens: 0,
                reasoning_tokens: 0,
            }
        );
    }
}
```

(If `serde_json` is not a dependency of `anima-core`, add it under `[dev-dependencies]` in `packages/core-rust/crates/anima-core/Cargo.toml`; it is already in the lockfile.)

Append to `packages/core-rust/crates/anima-model-adapters/src/tests.rs`:

```rust
#[tokio::test]
async fn openai_stream_requests_usage_and_parses_token_details() {
    let app = Router::new().route(
        "/v1/chat/completions",
        post(|Json(body): Json<Value>| async move {
            assert_eq!(body["stream"], true);
            assert_eq!(body["stream_options"]["include_usage"], true);
            (
                [("content-type", "text/event-stream")],
                concat!(
                    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\n",
                    "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":6,\"total_tokens\":16,\"prompt_tokens_details\":{\"cached_tokens\":4},\"completion_tokens_details\":{\"reasoning_tokens\":2}}}\n\n",
                    "data: [DONE]\n\n"
                ),
            )
        }),
    );
    let base_url = spawn_server(app).await;
    let adapter = adapter_with(&[("openai", Some("key"), &format!("{base_url}/v1"))]);
    let sink = FrameSink(Mutex::new(Vec::new()));

    adapter
        .stream(&agent_config("openai", false), &request(), &sink)
        .await
        .unwrap();

    let frames = sink.0.lock().unwrap().clone();
    let Some(ModelStreamFrame::Final(response)) = frames.last() else {
        panic!("expected final response")
    };
    assert_eq!(response.usage.prompt_tokens, 10);
    assert_eq!(response.usage.completion_tokens, 6);
    assert_eq!(response.usage.total_tokens, 16);
    assert_eq!(response.usage.cached_prompt_tokens, 4);
    assert_eq!(response.usage.reasoning_tokens, 2);
}

#[tokio::test]
async fn providers_without_documented_stream_usage_option_do_not_receive_it() {
    let app = Router::new().route(
        "/v1/chat/completions",
        post(|Json(body): Json<Value>| async move {
            assert!(body.get("stream_options").is_none());
            (
                [("content-type", "text/event-stream")],
                concat!(
                    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1,\"total_tokens\":2}}\n\n",
                    "data: [DONE]\n\n"
                ),
            )
        }),
    );
    let base_url = spawn_server(app).await;
    let adapter = adapter_with(&[("mistral", Some("key"), &format!("{base_url}/v1"))]);
    let sink = FrameSink(Mutex::new(Vec::new()));

    adapter
        .stream(&agent_config("mistral", false), &request(), &sink)
        .await
        .unwrap();

    let frames = sink.0.lock().unwrap().clone();
    let Some(ModelStreamFrame::Final(response)) = frames.last() else {
        panic!("expected final response")
    };
    assert_eq!(response.usage.total_tokens, 2);
}

#[tokio::test]
async fn openai_generate_parses_cached_and_reasoning_tokens() {
    let app = Router::new().route(
        "/v1/chat/completions",
        post(|| async {
            Json(json!({
                "choices":[{"message":{"content":"ok"},"finish_reason":"stop"}],
                "usage":{"prompt_tokens":20,"completion_tokens":9,"total_tokens":29,
                         "prompt_tokens_details":{"cached_tokens":12},
                         "completion_tokens_details":{"reasoning_tokens":5}}
            }))
        }),
    );
    let base_url = spawn_server(app).await;

    let response = adapter_with(&[("openai", Some("key"), &format!("{base_url}/v1"))])
        .generate(&agent_config("openai", false), &request())
        .await
        .expect("openai response");

    assert_eq!(response.usage.cached_prompt_tokens, 12);
    assert_eq!(response.usage.reasoning_tokens, 5);
    assert_eq!(response.usage.total_tokens, 29);
}

#[tokio::test]
async fn anthropic_usage_counts_cache_tokens_as_prompt_tokens() {
    let app = Router::new().route(
        "/v1/messages",
        post(|| async {
            Json(json!({
                "content":[{"type":"text","text":"ok"}],
                "stop_reason":"end_turn",
                "usage":{"input_tokens":5,"cache_read_input_tokens":20,"cache_creation_input_tokens":1,"output_tokens":7}
            }))
        }),
    );
    let base_url = spawn_server(app).await;

    let response = adapter_with(&[("anthropic", Some("key"), &base_url)])
        .generate(&agent_config("anthropic", false), &request())
        .await
        .expect("anthropic response");

    assert_eq!(response.usage.prompt_tokens, 26);
    assert_eq!(response.usage.cached_prompt_tokens, 20);
    assert_eq!(response.usage.completion_tokens, 7);
    assert_eq!(response.usage.total_tokens, 33);
}

#[tokio::test]
async fn anthropic_stream_usage_counts_cache_tokens_as_prompt_tokens() {
    let app = Router::new().route(
        "/v1/messages",
        post(|| async {
            (
                [("content-type", "text/event-stream")],
                concat!(
                    "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":3,\"cache_read_input_tokens\":40,\"cache_creation_input_tokens\":2}}}\n\n",
                    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\n",
                    "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":6}}\n\n",
                    "data: {\"type\":\"message_stop\"}\n\n"
                ),
            )
        }),
    );
    let base_url = spawn_server(app).await;
    let adapter = adapter_with(&[("anthropic", Some("key"), &base_url)]);
    let sink = FrameSink(Mutex::new(Vec::new()));

    adapter
        .stream(&agent_config("anthropic", false), &request(), &sink)
        .await
        .unwrap();

    let frames = sink.0.lock().unwrap().clone();
    let Some(ModelStreamFrame::Final(response)) = frames.last() else {
        panic!("expected final response")
    };
    assert_eq!(response.usage.prompt_tokens, 45);
    assert_eq!(response.usage.cached_prompt_tokens, 40);
    assert_eq!(response.usage.completion_tokens, 6);
    assert_eq!(response.usage.total_tokens, 51);
}

#[tokio::test]
async fn google_usage_counts_thinking_tokens_as_completion_tokens() {
    let app = Router::new().route(
        "/v1beta/models/gemini-2.5-flash:generateContent",
        post(|| async {
            Json(json!({
                "candidates":[{"content":{"role":"model","parts":[{"text":"ok"}]},"finishReason":"STOP"}],
                "usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5,"thoughtsTokenCount":7,
                                 "cachedContentTokenCount":3,"totalTokenCount":22}
            }))
        }),
    );
    let base_url = spawn_server(app).await;
    let mut config = agent_config("google", false);
    config.model = "gemini-2.5-flash".into();

    let response = adapter_with(&[("google", Some("key"), &base_url)])
        .generate(&config, &request())
        .await
        .expect("google response");

    assert_eq!(response.usage.prompt_tokens, 10);
    assert_eq!(response.usage.completion_tokens, 12);
    assert_eq!(response.usage.total_tokens, 22);
    assert_eq!(response.usage.cached_prompt_tokens, 3);
    assert_eq!(response.usage.reasoning_tokens, 7);
}
```

Append to `packages/core-rust/crates/anima-model-adapters/src/chatgpt/tests.rs`:

```rust
#[test]
fn completed_response_parses_cached_and_reasoning_usage() {
    let response = json!({
        "status": "completed",
        "output": [{"type":"message","role":"assistant","content":[{"type":"output_text","text":"done"}]}],
        "usage": {"input_tokens": 12, "output_tokens": 8, "total_tokens": 20,
                  "input_tokens_details": {"cached_tokens": 5},
                  "output_tokens_details": {"reasoning_tokens": 3}}
    });

    let parsed = completed_response(&response).expect("completed response parses");

    assert_eq!(parsed.usage.prompt_tokens, 12);
    assert_eq!(parsed.usage.cached_prompt_tokens, 5);
    assert_eq!(parsed.usage.reasoning_tokens, 3);
    assert_eq!(parsed.usage.total_tokens, 20);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p anima-core --lib token_usage_tests` then `cargo test -p anima-model-adapters`
Expected: compile errors for the missing `cached_prompt_tokens` / `reasoning_tokens` fields.

- [ ] **Step 3: Extend `TokenUsage` and the runtime accumulator**

`packages/core-rust/crates/anima-core/src/agent.rs`:

```rust
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    /// Prompt tokens served from a provider cache; included in `prompt_tokens`.
    #[serde(default)]
    pub cached_prompt_tokens: u64,
    /// Reasoning or thinking tokens; included in `completion_tokens`.
    #[serde(default)]
    pub reasoning_tokens: u64,
}
```

`runtime.rs` `apply_token_usage`:

```rust
    fn apply_token_usage(&mut self, usage: &TokenUsage) {
        let total = &mut self.state.token_usage;
        total.prompt_tokens += usage.prompt_tokens;
        total.completion_tokens += usage.completion_tokens;
        total.total_tokens += usage.total_tokens;
        total.cached_prompt_tokens += usage.cached_prompt_tokens;
        total.reasoning_tokens += usage.reasoning_tokens;
    }
```

Then fix every remaining `TokenUsage { .. }` literal listed under Files by appending `..TokenUsage::default()` (compile errors list each one).

- [ ] **Step 4: Parse details in each adapter**

`common.rs` `response_usage`:

```rust
    TokenUsage {
        prompt_tokens: value_to_u64(usage.get("prompt_tokens")),
        completion_tokens: value_to_u64(usage.get("completion_tokens")),
        total_tokens: value_to_u64(usage.get("total_tokens")),
        cached_prompt_tokens: value_to_u64(
            usage
                .get("prompt_tokens_details")
                .and_then(|details| details.get("cached_tokens")),
        ),
        reasoning_tokens: value_to_u64(
            usage
                .get("completion_tokens_details")
                .and_then(|details| details.get("reasoning_tokens")),
        ),
    }
```

`anthropic.rs` usage block:

```rust
    let usage = if let Some(usage) = payload.get("usage") {
        let cache_read = value_to_u64(usage.get("cache_read_input_tokens"));
        let prompt = value_to_u64(usage.get("input_tokens"))
            + cache_read
            + value_to_u64(usage.get("cache_creation_input_tokens"));
        let completion = value_to_u64(usage.get("output_tokens"));
        TokenUsage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: prompt + completion,
            cached_prompt_tokens: cache_read,
            reasoning_tokens: 0,
        }
    } else {
        TokenUsage::default()
    };
```

`google.rs` usage block:

```rust
    let usage = if let Some(usage) = payload.get("usageMetadata") {
        let prompt = value_to_u64(usage.get("promptTokenCount"))
            + value_to_u64(usage.get("toolUsePromptTokenCount"));
        let thoughts = value_to_u64(usage.get("thoughtsTokenCount"));
        let completion = value_to_u64(usage.get("candidatesTokenCount")) + thoughts;
        TokenUsage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: prompt + completion,
            cached_prompt_tokens: value_to_u64(usage.get("cachedContentTokenCount")),
            reasoning_tokens: thoughts,
        }
    } else {
        TokenUsage::default()
    };
```

`chatgpt.rs` usage literal:

```rust
        usage: TokenUsage {
            prompt_tokens,
            completion_tokens,
            total_tokens: usage["total_tokens"]
                .as_u64()
                .unwrap_or(prompt_tokens.saturating_add(completion_tokens)),
            cached_prompt_tokens: usage["input_tokens_details"]["cached_tokens"]
                .as_u64()
                .unwrap_or(0),
            reasoning_tokens: usage["output_tokens_details"]["reasoning_tokens"]
                .as_u64()
                .unwrap_or(0),
        },
```

`ollama.rs`: append `..TokenUsage::default()` to its literal.

`stream.rs`: extend `StreamUsage` and its merge functions:

```rust
#[derive(Default)]
struct StreamUsage {
    prompt: Option<u64>,
    completion: Option<u64>,
    total: Option<u64>,
    cached_prompt: Option<u64>,
    reasoning: Option<u64>,
}

impl StreamUsage {
    fn merge_openai(&mut self, usage: Option<&Value>) -> Result<(), String> {
        let Some(usage) = usage else { return Ok(()) };
        self.merge(
            usage.get("prompt_tokens"),
            usage.get("completion_tokens"),
            usage.get("total_tokens"),
        )?;
        merge_usage_value(
            &mut self.cached_prompt,
            usage
                .get("prompt_tokens_details")
                .and_then(|details| details.get("cached_tokens")),
        )?;
        merge_usage_value(
            &mut self.reasoning,
            usage
                .get("completion_tokens_details")
                .and_then(|details| details.get("reasoning_tokens")),
        )
    }

    fn merge_anthropic_start(&mut self, usage: Option<&Value>) -> Result<(), String> {
        let Some(usage) = usage else { return Ok(()) };
        let cache_read = optional_usage_value(usage.get("cache_read_input_tokens"))?;
        let cache_write = optional_usage_value(usage.get("cache_creation_input_tokens"))?;
        if let Some(input) = optional_usage_value(usage.get("input_tokens"))? {
            let prompt = input
                .checked_add(cache_read.unwrap_or(0))
                .and_then(|value| value.checked_add(cache_write.unwrap_or(0)))
                .ok_or_else(stream_parse_error)?;
            merge_usage_number(&mut self.prompt, prompt)?;
        }
        if let Some(cache_read) = cache_read {
            merge_usage_number(&mut self.cached_prompt, cache_read)?;
        }
        Ok(())
    }
```

Keep `merge_anthropic_delta` and `merge` as they are. In `finish`, return:

```rust
        Ok(TokenUsage {
            prompt_tokens,
            completion_tokens,
            total_tokens,
            cached_prompt_tokens: self.cached_prompt.unwrap_or(0),
            reasoning_tokens: self.reasoning.unwrap_or(0),
        })
```

Replace `merge_usage_value` with:

```rust
fn optional_usage_value(value: Option<&Value>) -> Result<Option<u64>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value.as_u64().map(Some).ok_or_else(stream_parse_error),
    }
}

fn merge_usage_value(target: &mut Option<u64>, value: Option<&Value>) -> Result<(), String> {
    match optional_usage_value(value)? {
        Some(value) => merge_usage_number(target, value),
        None => Ok(()),
    }
}

fn merge_usage_number(target: &mut Option<u64>, value: u64) -> Result<(), String> {
    if target.is_some_and(|current| current != value) {
        return Err(stream_parse_error());
    }
    *target = Some(value);
    Ok(())
}
```

`adapter.rs`: add the allowlist and pass it through:

```rust
/// Providers whose chat-completions streaming documents `stream_options.include_usage`.
fn stream_usage_option_supported(provider_id: &str) -> bool {
    matches!(provider_id, "openai" | "deepseek" | "vllm")
}
```

Give `stream_openai_compatible` a final parameter `include_usage: bool`, and after `body["stream"] = serde_json::Value::Bool(true);` add:

```rust
            if include_usage {
                body["stream_options"] = serde_json::json!({ "include_usage": true });
            }
```

At the call site pass `stream_usage_option_supported(definition.id)` as the new last argument.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p anima-core` then `cargo test -p anima-model-adapters` then `cargo test -p anima-swarm`
Expected: PASS. Then `cargo build -p anima-daemon` compiles.

- [ ] **Step 6: Commit**

```bash
git add packages/core-rust hosts/rust-daemon/src
git commit -m "feat(adapters): record cached and reasoning tokens and request stream usage"
```

---

### Task 4: Model table and cost estimation

**Files:**
- Create: `packages/core-rust/crates/anima-model-adapters/src/models.rs`
- Modify: `packages/core-rust/crates/anima-model-adapters/src/lib.rs` (module + exports)
- Test: `packages/core-rust/crates/anima-model-adapters/src/models.rs` (inline `#[cfg(test)] mod tests`)
- Data source: `docs/superpowers/plans/data/2026-09-23-model-table.md` (verified 2026-09-23)

**Interfaces:**
- Consumes: `crate::catalog::resolve_provider(id: &str) -> Option<CatalogEntry>` (entry has `.definition.id`), `anima_core::TokenUsage` (Task 3 fields).
- Produces (re-exported from the crate root): `ModelPricing`, `ModelInfo`, `CostEstimate`, `PRICING_TABLE_DATE`, `model_table() -> &'static [ModelInfo]`, `model_info(provider: &str, model: &str) -> Option<&'static ModelInfo>`, `estimate_cost_micros(provider: &str, model: &str, usage: &TokenUsage) -> CostEstimate`, `price_usage(pricing: &ModelPricing, usage: &TokenUsage) -> u64`.

- [ ] **Step 1: Write the module skeleton with failing tests**

Create `models.rs`:

```rust
//! Built-in model metadata for context budgets and cost estimates.
//!
//! Rows are transcribed from `docs/superpowers/plans/data/2026-09-23-model-table.md`,
//! which cites the official page each value was read from on 2026-09-23.
//! Prices are micro-USD per one million tokens at the standard base tier.

use anima_core::TokenUsage;

use crate::catalog::resolve_provider;

pub const PRICING_TABLE_DATE: &str = "2026-09-23";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelPricing {
    pub input_micros_per_mtok: u64,
    pub output_micros_per_mtok: u64,
    /// Cache-read price; `None` bills cached tokens at the input rate.
    pub cached_input_micros_per_mtok: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelInfo {
    pub provider: &'static str,
    /// Lowercase API model id prefix; the longest matching prefix wins.
    pub model_prefix: &'static str,
    pub context_window: u32,
    /// `None` when the provider does not publish a maximum output.
    pub max_output: Option<u32>,
    pub vision: bool,
    pub pricing: Option<ModelPricing>,
    pub source: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CostEstimate {
    Priced { micros: u64 },
    /// Local providers with no per-token charge.
    Free,
    /// ChatGPT subscription sign-in; no per-token charge.
    Subscription,
    Unknown,
}

const FREE_PROVIDERS: &[&str] = &["ollama", "vllm"];

static MODELS: &[ModelInfo] = &[];

pub fn model_table() -> &'static [ModelInfo] {
    MODELS
}

pub fn model_info(provider: &str, model: &str) -> Option<&'static ModelInfo> {
    let provider = canonical_provider(provider)?;
    model_info_in(MODELS, provider, model)
}

fn model_info_in<'a>(table: &'a [ModelInfo], provider: &str, model: &str) -> Option<&'a ModelInfo> {
    let model = model.trim().to_ascii_lowercase();
    table
        .iter()
        .filter(|info| info.provider == provider && model.starts_with(info.model_prefix))
        .max_by_key(|info| info.model_prefix.len())
}

pub fn estimate_cost_micros(provider: &str, model: &str, usage: &TokenUsage) -> CostEstimate {
    let Some(provider) = canonical_provider(provider) else {
        return CostEstimate::Unknown;
    };
    if provider == "chatgpt" {
        return CostEstimate::Subscription;
    }
    if FREE_PROVIDERS.contains(&provider) {
        return CostEstimate::Free;
    }
    match model_info_in(MODELS, provider, model).and_then(|info| info.pricing) {
        Some(pricing) => CostEstimate::Priced {
            micros: price_usage(&pricing, usage),
        },
        None => CostEstimate::Unknown,
    }
}

/// Prices usage in micro-USD, rounding half up.
pub fn price_usage(pricing: &ModelPricing, usage: &TokenUsage) -> u64 {
    let cached = usage.cached_prompt_tokens.min(usage.prompt_tokens);
    let uncached = usage.prompt_tokens - cached;
    let cached_rate = pricing
        .cached_input_micros_per_mtok
        .unwrap_or(pricing.input_micros_per_mtok);
    let total = u128::from(uncached) * u128::from(pricing.input_micros_per_mtok)
        + u128::from(cached) * u128::from(cached_rate)
        + u128::from(usage.completion_tokens) * u128::from(pricing.output_micros_per_mtok);
    u64::try_from((total + 500_000) / 1_000_000).unwrap_or(u64::MAX)
}

fn canonical_provider(provider: &str) -> Option<&'static str> {
    let requested = provider.trim().to_ascii_lowercase();
    if requested == "chatgpt" {
        return Some("chatgpt");
    }
    resolve_provider(&requested).map(|entry| entry.definition.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_definitions;
    use std::collections::BTreeSet;

    const PRICED: ModelPricing = ModelPricing {
        input_micros_per_mtok: 2_000_000,
        output_micros_per_mtok: 10_000_000,
        cached_input_micros_per_mtok: Some(200_000),
    };

    fn row(prefix: &'static str) -> ModelInfo {
        ModelInfo {
            provider: "openai",
            model_prefix: prefix,
            context_window: 100_000,
            max_output: Some(10_000),
            vision: true,
            pricing: Some(PRICED),
            source: "https://example.com",
        }
    }

    #[test]
    fn longest_matching_prefix_wins() {
        let table = [row("o3"), row("o3-pro")];

        assert_eq!(model_info_in(&table, "openai", "o3-pro-2025-06-10").unwrap().model_prefix, "o3-pro");
        assert_eq!(model_info_in(&table, "openai", "O3-2025-04-16").unwrap().model_prefix, "o3");
        assert!(model_info_in(&table, "openai", "gpt-4o").is_none());
        assert!(model_info_in(&table, "anthropic", "o3").is_none());
    }

    #[test]
    fn cost_uses_the_cached_rate_and_rounds_half_up() {
        let usage = TokenUsage {
            prompt_tokens: 1_000,
            completion_tokens: 500,
            total_tokens: 1_500,
            cached_prompt_tokens: 400,
            reasoning_tokens: 100,
        };

        // 600 × 2 + 400 × 0.2 + 500 × 10 micro-USD
        assert_eq!(price_usage(&PRICED, &usage), 6_280);

        let without_cache_price = ModelPricing { cached_input_micros_per_mtok: None, ..PRICED };
        assert_eq!(price_usage(&without_cache_price, &usage), 7_000);

        let tiny = TokenUsage { prompt_tokens: 1, total_tokens: 1, ..TokenUsage::default() };
        assert_eq!(price_usage(&PRICED, &tiny), 2);
    }

    #[test]
    fn local_providers_are_free_chatgpt_is_subscription_and_unknowns_are_unknown() {
        let usage = TokenUsage { prompt_tokens: 10, completion_tokens: 10, total_tokens: 20, ..TokenUsage::default() };

        assert_eq!(estimate_cost_micros("ollama", "llama3.2", &usage), CostEstimate::Free);
        assert_eq!(estimate_cost_micros("vllm", "anything", &usage), CostEstimate::Free);
        assert_eq!(estimate_cost_micros("chatgpt", "gpt-5.5", &usage), CostEstimate::Subscription);
        assert_eq!(estimate_cost_micros("openai", "not-a-real-model", &usage), CostEstimate::Unknown);
        assert_eq!(estimate_cost_micros("no-such-provider", "x", &usage), CostEstimate::Unknown);
    }

    #[test]
    fn every_row_is_well_formed_unique_and_sourced() {
        let known: BTreeSet<&str> = provider_definitions().iter().map(|definition| definition.id).collect();
        let mut seen = BTreeSet::new();

        assert!(!model_table().is_empty(), "the built-in table must not be empty");
        for info in model_table() {
            assert!(known.contains(info.provider), "{} is not a catalog provider", info.provider);
            assert_eq!(info.model_prefix, info.model_prefix.to_ascii_lowercase());
            assert!(info.context_window > 0, "{}", info.model_prefix);
            if let Some(max_output) = info.max_output {
                assert!(max_output <= info.context_window, "{}", info.model_prefix);
            }
            assert!(info.source.starts_with("https://"), "{}", info.model_prefix);
            if let Some(pricing) = info.pricing {
                assert!(pricing.input_micros_per_mtok > 0 && pricing.output_micros_per_mtok > 0, "{}", info.model_prefix);
            }
            assert!(seen.insert((info.provider, info.model_prefix)), "duplicate {}", info.model_prefix);
        }
    }

    #[test]
    fn table_resolves_representative_models_through_aliases() {
        let sonnet = model_info("anthropic", "claude-sonnet-5").expect("sonnet 5 row");
        assert_eq!(sonnet.context_window, 1_000_000);
        assert_eq!(
            sonnet.pricing,
            Some(ModelPricing {
                input_micros_per_mtok: 2_000_000,
                output_micros_per_mtok: 10_000_000,
                cached_input_micros_per_mtok: Some(200_000),
            })
        );
        assert_eq!(model_info("anthropic", "claude-opus-5-5").unwrap().model_prefix, "claude-opus-5-5");
        assert_eq!(model_info("anthropic", "claude-opus-5").unwrap().model_prefix, "claude-opus-5");
        assert_eq!(model_info("openai", "gpt-5.5-pro").unwrap().model_prefix, "gpt-5.5-pro");
        assert_eq!(model_info("openai", "gpt-5.5-2026-04-23").unwrap().model_prefix, "gpt-5.5");
        // `gemini` is a catalog alias of `google`.
        assert_eq!(
            model_info("gemini", "gemini-2.5-flash").map(|info| info.provider),
            Some("google")
        );
    }
}
```

Add to `lib.rs` after `mod catalog;`: `mod models;` and export:

```rust
pub use models::{
    estimate_cost_micros, model_info, model_table, price_usage, CostEstimate, ModelInfo,
    ModelPricing, PRICING_TABLE_DATE,
};
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p anima-model-adapters models::tests`
Expected: `every_row_is_well_formed_unique_and_sourced` and `table_resolves_representative_models_through_aliases` FAIL (empty table); the other three PASS.

- [ ] **Step 3: Transcribe the verified rows**

Fill `MODELS` with one `ModelInfo` per row of every provider table in `docs/superpowers/plans/data/2026-09-23-model-table.md`, in the file's order, using these rules:
- `provider` is the section id (`anthropic`, `openai`, `google`, `deepseek`, `xai`, `mistral`); `model_prefix` is the backticked prefix, lowercase.
- Context like `1M` → `1_000_000`, `200K` → `200_000`, `1,050,000 (max in 922,000)` → `1_050_000`. Max output `unknown` or "no limit" → `None`.
- Prices: `$/M` × 1,000,000 → micro-USD (`0.25` → `250_000`, `12.50` → `12_500_000`). Cached input `none` or `n/a` → `None`. Cache-write columns are not stored.
- If input or output price is `unknown`, set `pricing: None`. For DeepSeek, use the standard (peak) price, not off-peak.
- `source` is the row's pricing-page URL from the file's link definitions.

Do not add models that are not in the file.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p anima-model-adapters`
Expected: PASS (all adapter tests plus the five model tests).

- [ ] **Step 5: Commit**

```bash
git add packages/core-rust/crates/anima-model-adapters/src/models.rs packages/core-rust/crates/anima-model-adapters/src/lib.rs docs/superpowers/plans/data/2026-09-23-model-table.md
git commit -m "feat(adapters): add verified model table and cost estimation"
```

---

## M0 verification

- [ ] `df -h /System/Volumes/Data` shows at least 12 GB free (otherwise run `cargo test -p anima-core -p anima-model-adapters -p anima-swarm -p anima-daemon` in the shared `target/` and record that the Nx run is pending disk space).
- [ ] `bun x nx run rust-daemon:test --skipNxCache` passes (this also runs `core-rust:test`).
- [ ] Update the master plan status row for M0.
