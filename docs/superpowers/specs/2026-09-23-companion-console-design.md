# Companion console: sessions, live runs, and assistant operations

Status: design approved in brainstorming on 2026-09-23; pending written-spec review.
Scope: one build delivered as ordered milestones on `feat/companion-console`.

Turn the single-companion web console (`apps/web`) into an OpenClaw/Hermes-class assistant console, backed by daemon-owned contracts in `hosts/rust-daemon` and reusable logic in `packages/core-rust`. The build adds separate chat sessions with one history sidebar (web chats, Telegram, check-ins, jobs, helpers), live streaming runs with tool cards, Stop/queue/steer, parallel sessions, per-tool approvals, owner-approved skills, automations with cron and run history, a memory editor, usage/cost, logs, health, attachments, voice input, and AI titles. Existing routes, the CLI, and the TUI keep working.

## 1. Goals and non-goals

Goals:

- Every conversation is a session. New chats start with empty context; long-term memory and a conversation-search tool carry continuity.
- Runs are asynchronous, observable live over one event stream per companion, stoppable, queueable, and steerable. Different sessions of one companion run concurrently.
- The daemon stays production-safe: bounded snapshot growth, crash-safe history, restart recovery that never replays side effects, owner authorization on every new route, and the existing VPS gateway unchanged.
- Reusable, host-agnostic logic (streaming step frames, cancellation, steering, context selection, model metadata, cost estimation, attachment rendering) lives in `packages/core-rust`; hosts own HTTP, persistence, and process concerns.

Non-goals (explicitly excluded):

- Online skill store or remote skill installation. OS-level shell sandboxing (approvals are the control). Multi-user or multi-tenant access. New channels. Approving tool calls from Telegram. Server-side speech-to-text or text-to-speech. PDF/Office text extraction (the document engine stays a planned extension). Parallel runs for helper agents. Changes to job review semantics beyond stopping a running attempt. Owner authorization on the pre-existing memory routes (the CLI `launch` command writes memories without a browser origin or admin token; hardening them needs CLI token support first and is a follow-up).

## 2. Architecture overview

| Layer | Adds |
|---|---|
| `anima-core` | Streaming step frames through a non-recorded run observer; cooperative cancellation; steering inbox; pure context-window selection; event-log cap; optional attachment MIME type; optional cached/reasoning token fields |
| `anima-model-adapters` | Model table (context window, max output, vision, prices); cost estimation; Google and native Ollama streaming; OpenAI-compatible stream usage; Google thinking-token accounting; cached-token parsing; image rendering for every provider |
| `anima-memory` | Update a memory with re-indexing; delete an entity with cascade; citation cleanup on forget |
| `rust-daemon` | Parallel-safe run coordinator; run ledger; async runs, queue, steer, stop for every source; sessions; history store with outbox; agent event stream; approvals; skills; automations (cron, one-time, active hours, run now, history, tools); memory edit routes; usage and pricing; logs ring buffer and stream; status aggregate; attachments; titles; compaction; conversation search; migration; security fixes |
| `packages/sdk` | Typed clients for every new route and the event stream |
| `apps/web` | New shell and hash routing; sessions sidebar; live session view; composer with attachments, voice, slash commands, queue/steer/stop; Approvals, Automations, Memory, Skills, Usage, Logs, and Health pages |
| `deploy/vps` | History store path, backup and upgrade documentation |

Daemon storage splits into two tiers:

- **Control plane** (existing whole-snapshot JSON file or Postgres `host_snapshots` row): current state only. It gains sessions, active and recent runs, pending approvals, approval policies and rules, the skills registry and drafts, pricing overrides, and new schedule fields. Its message history is bounded to a hot tail (section 13).
- **History store** (new, row-based, append-only): SQLite file, Postgres tables, or in-memory for ephemeral mode. It mirrors every committed message and holds terminal runs, per-call usage, decided approvals, automation run history, and attachment records.

## 3. Sessions

### 3.1 Identity and kinds

A session is one conversation room of one agent plus a session record. The session id equals the room id, is opaque to clients, matches `^[A-Za-z0-9._:-]{1,200}$`, and is percent-encoded in URL paths.

| Kind | Room ids | Origin |
|---|---|---|
| `chat` | `chat:<uuid-v4>` for new web chats; legacy `direct:<agentId>`; generated `room-*` rooms on the companion from API/CLI runs without a room | `web` or `api` |
| `telegram` | `telegram:<connectorId>` | `telegram` |
| `checkin` | `schedule:<scheduleId>` | `schedule` |
| `job` | `job:<jobId>` | `job` |
| `helper` | Rooms on helper or specialist agents created by delegation, and `peer:<a>:<b>` rooms | `delegation` or `peer` |

`POST /api/agents/{id}/run` rejects `roomId` values starting with `telegram:`, `schedule:`, `job:`, or `peer:` (today only `peer:` is blocked).

### 3.2 Session record

Stored in the control plane, keyed by `(agentId, id)`:

- `id`, `agentId`, `kind`, `origin`.
- `title` (1–120 characters, stored as plain text) and `titleSource`: `first_message`, `generated`, `owner`, or `system`.
- `createdAtMs`, `lastActivityAtMs`, `lastReadAtMs` (nullable), `archived`.
- `parentSessionId`, `parentRunId`, `parentAgentId` for helper sessions (nullable).
- `summary` (nullable): `{ text ≤ 8 KiB, throughMessageId, createdAtMs, sourceMessageCount }`.
- `contextTrimmed` (nullable): `{ droppedThroughMessageId, atMs }`, the latest turn outside the model's view.
- `sessionAllowances`: approval allowances granted "for this session" (section 7).

Derived per response, never stored: `messageCount`, `preview` (last visible message, ≤ 160 characters), `activeRuns`, `pendingApprovals`, `unread`, `usage` totals, and `capabilities`. `unread` is true when an assistant or inbound message is newer than `lastReadAtMs`; the owner's own web messages advance `lastReadAtMs`.

Capabilities by kind:

| Kind | Send | Steer | Stop | Rename | Archive | Delete | Compact | Export |
|---|---|---|---|---|---|---|---|---|
| chat | yes | yes | yes | yes | yes | yes | yes | yes |
| telegram | yes, as a Telegram owner turn | no | yes | yes | yes | no | yes | yes |
| checkin | yes, a reply in the thread | yes | yes | yes | yes | only after its schedule is deleted | yes | yes |
| job | no; links to Work › Runs | no | yes | no | yes | no | no | yes |
| helper | no | no | yes | no | yes | no | no | yes |

### 3.3 Routes

All session routes require owner authorization: reads use `authorize_read` with `Cache-Control: no-store`; mutations use `authorize`.

- `GET /api/agents/{id}/sessions?kind=&archived=false&q=&cursor=&limit=50&includeHelpers=true` returns `{ sessions, nextCursor }` sorted by `lastActivityAtMs` descending. `limit` is 1–200. `q` matches titles and message text across the hot tail and the history store and adds `match: { messageId, snippet }`. With `includeHelpers`, the list includes sessions of the agent's helpers and delegated rooms whose `parentAgentId` is the agent; each item carries its own `agentId`.
- `GET /api/agents/{id}/sessions/{sid}` returns one session.
- `GET /api/agents/{id}/sessions/{sid}/messages?before=<messageId>&limit=50&includeHidden=false` returns `{ messages, nextBefore }`, oldest to newest within the page, merging the history store with not-yet-mirrored hot messages (deduplicated by id). Hidden messages are silent check-in pairs. Each message has `id`, `role`, `text`, `attachments` (metadata only), `metadata` (`toolCalls`, `toolCallId`, `stepId`, `runId`, `stopped`, `revised`, `steer`, `skill`, `clientRequestId`, `communication`), and `createdAtMs`.
- `POST /api/agents/{id}/sessions` with `{ title? }` returns 201 with a new `chat` session.
- `PATCH /api/agents/{id}/sessions/{sid}` with `{ title?, archived?, lastReadAtMs? }`. Setting `title` sets `titleSource: owner`.
- `DELETE /api/agents/{id}/sessions/{sid}` is allowed only where the capability table allows it. It returns 409 while a run in the session is queued, running, or awaiting approval. It removes the record, hot messages, mirrored messages, and attachment records; uploaded files stay in the workspace `uploads/` folder; usage rows keep their `sessionId` and render as "Deleted chat"; memories are kept.
- `POST /api/agents/{id}/sessions/{sid}/compact` returns 202 and summarizes (section 5.4).
- `GET /api/agents/{id}/sessions/{sid}/export` returns `text/markdown` with the full transcript, including archived messages.
- `GET /api/agents?view=summary` returns agents without `messages`; the default response is unchanged.

## 4. Runs

### 4.1 Run ledger

Every run executed by the coordinator, from any source, has a ledger record in the control plane:

- `id` (`run_<uuid-v4>`), `agentId`, `sessionId`, `source` (`web`, `api`, `telegram`, `schedule`, `job`, `delegation`, `peer`), `sourceRef` (schedule id, job id and attempt, or Telegram inbound id).
- `status`: `queued`, `running`, `awaiting_approval`, `completed`, `failed`, `cancelled`, or `interrupted`.
- `idempotencyKey`, `input` (`{ text ≤ 32 KiB, attachmentIds ≤ 10, skill? }`), `createdAtMs`, `startedAtMs`, `finishedAtMs`, `error` (`{ code, message }`), `stop` (`{ requestedAtMs }`), `toolsStarted` (distinct names, ≤ 50), `steps` (per-model-call usage, ≤ 50), `usage` totals, `model`, `provider`, `parentRunId`, `mirrored`.

The control plane keeps non-terminal runs plus, per agent, terminal runs from the last 24 hours up to 50; terminal runs are also written to the history store and pruned from the control plane only once `mirrored`. For Telegram, schedule, job, delegation, and peer runs the ledger entry is written in the run-start save that already happens, so no extra save is added. Web runs add one save at acceptance so the queue survives restarts.

### 4.2 Starting a run

`POST /api/agents/{id}/sessions/{sid}/runs` requires an `Idempotency-Key` header (1–128 characters) and takes `{ text, attachmentIds?, skill?, mode?: "queue" | "steer" }`.

- 202 with `{ run }` when accepted as queued or running. For `steer` into an active run: 202 with the active run and `steer: { status: "pending" }`.
- The same key within 24 hours returns 200 with the original run and creates nothing.
- 400: empty text with no attachments, text over 32 KiB, unknown or foreign attachment ids, unknown skill, or `steer` on a kind that cannot steer. 403: owner authorization. 404: agent or session. 409: kind cannot send, or the agent is being deleted. 429: the agent already has 8 queued runs. 503: persistence unavailable.
- For `telegram` sessions the route performs the existing connector owner-turn flow with the same idempotency key and returns the run it creates.

### 4.3 Concurrency

- Runs in the same session are serialized in acceptance order.
- Runs in different sessions of one agent run concurrently, up to `ANIMAOS_RS_MAX_RUNS_PER_AGENT` (default 3). Helper agents are fixed at 1.
- The global `ANIMAOS_RS_MAX_CONCURRENT_RUNS` still applies. Async runs wait for a global permit; the legacy blocking route keeps its fail-fast 503.
- A run acquires, in order: its session lock, an agent slot, and a global permit. Nested delegated and peer runs try-acquire and fail fast with "Specialist is busy", as today.
- The scheduler's one-run-per-agent rule becomes one run per automation. The job dispatcher keeps one running job per agent but is no longer blocked by chat runs in other sessions.

### 4.4 Parallel-safe execution

Replaces checking the single runtime out of `DaemonState` for the whole run:

1. The canonical `AgentRuntime` stays in `DaemonState`. Each run builds an isolated runtime from the canonical record (`from_snapshot`) with the target session's selected context and the standard providers, evaluators, and database wiring, without the restore-time Running→Failed conversion.
2. Per-run configuration (roster prompt, tools, skills index, run-origin notes, skill body) is applied only to the isolated runtime. The canonical configuration is never rewritten during a run; a PATCH applies to later runs.
3. Commit, under the control-plane transaction and state write lock: append the run's new messages and events to the canonical record, add the run's token delta and step delta, set `last_task`, update the ledger, call the source's commit hook with `RunOutcome { runId, sessionId, replyMessageId, result, status }`, and save. Hooks use `replyMessageId` instead of scanning the transcript backwards.
4. Rollback removes exactly the run's message and event ids and subtracts its token delta. The whole-snapshot `rollback_agent_runtime(baseline)` used by connector and schedule hooks is removed.
5. Agent status is derived: `Running` while any run of the agent is running or awaiting approval; otherwise the last finished run's status, or `Idle`.
6. Deleting an agent returns 409 while any of its runs is running or awaiting approval; queued runs are cancelled. A commit for an agent deleted in between is discarded, which replaces the tombstone.
7. `spawn_helper` reuse reserves the helper's only slot. `is_agent_busy` means "no free slot". The owner PUT-tasks guard uses the in-flight count.
8. `todo_write` becomes compare-and-swap on the tasks revision and returns a conflict message the model can retry.
9. Workspace files, mail drafts, and calendar writes remain last-writer-wins across sessions; `edit_file` already fails on stale exact matches.
10. Per-run step idempotency keys include the run id so concurrent runs never collide in `step_log`.

### 4.5 Streaming

- The runtime calls `ModelAdapter::stream`. A new non-recorded `RunObserver` receives `StepStarted { stepId }`, `TextDelta { stepId, text }`, `StepUsage { stepId, usage }`, and `StepFinished { stepId, messageId }`. Deltas never enter the stored event log.
- `stepId` is `<runId>:<n>`. Every model call is a step and ends as an ordinary recorded message tagged with its `stepId`. An evaluator revision records the earlier step with `metadata.revised: true`; nothing streamed is ever retracted.
- The daemon's `RuntimeModelAdapter::stream` routes every provider to its streaming adapter.
- The daemon coalesces deltas and flushes every 50 ms or 512 bytes, whichever comes first.

### 4.6 Stop

`POST /api/agents/{id}/runs/{runId}/stop` returns 202 with the run and is idempotent.

- Queued: becomes `cancelled` and never starts.
- Running or awaiting approval: the stop is persisted, then the run's cancellation signal is set. The runtime checks it before each model call, while streaming (dropping the in-flight request), before each tool batch, and while waiting for an approval (which resolves as stopped). The bash polling loop kills its child process when the signal is set. Tools already executing finish; background processes keep running.
- Tool calls the model requested but that never ran receive the result "Cancelled before running (stopped by owner)" with error status, so no tool call is left without a result.
- Partial streamed text is recorded as an assistant message with `metadata.stopped: true`. The result is an error with code `stopped`; the ledger status is `cancelled`; the stop does not make the agent `Failed`.
- Helper runs started by the stopped run are stopped too.
- Source-specific outcomes:
  - Schedule: new outcome `stopped`; the schedule stays enabled.
  - Telegram: the inbound record gets a new durable `Stopped` state, persisted before signalling. The commit hook treats `Stopped` as a normal finish with no reply, and it is never re-run. A reply already committed but not delivered becomes a new `Suppressed` outbound state, excluded from delivery and from backpressure counts. Compaction, deletion filters, and snapshot validation accept both states.
  - Job: a durable stop marker is persisted before signalling. The attempt is recorded with a new `stopped` history status, the job moves to `NeedsReview`, and retry requires `acknowledgeUncertain`. The commit hook, rollback, and restart recovery respect the marker.

### 4.7 Steering

- `mode: "steer"` while the session has a running run adds the text to that run's steering inbox. Before the next model call the runtime drains the inbox, records each item as a user message with `metadata.steer: true`, and appends it to the conversation. The stream emits `run.steered`.
- If the run finishes without another model call, pending steers become queued runs in order.
- Without an active run, `steer` behaves like `queue`. While awaiting approval, steers wait for the next model call.

### 4.8 Restart recovery

On boot, ledger runs that were `queued` become `interrupted` with code `restart_before_start` (safe to resend). Runs that were `running` or `awaiting_approval` become `interrupted` with code `restart_during_run`, keeping `toolsStarted` so the UI can warn that effects may have happened. Pending approvals become `expired`. Each source's existing recovery is unchanged: Telegram re-runs `Processing` inbound records under a new ledger run, jobs go to `NeedsReview`, and interrupted schedules auto-disable.

### 4.9 Legacy blocking route

`POST /api/agents/{id}/run` keeps its request and response contract and fail-fast saturation behavior. It runs on the new coordinator with source `api`, streams to subscribers, and appears in sessions.

## 5. Context

### 5.1 Budget

A model table in `anima-model-adapters` (`models.rs`) maps `(provider, model prefix)` to `ModelInfo { contextWindow, maxOutput, vision, pricing }`, using alias resolution and the longest matching prefix. A run's context budget is the agent setting `contextBudgetTokens` if set, otherwise 60% of the model's context window, otherwise 32,000 tokens. The reply reserve is the agent's `maxTokens` setting or 4,096.

### 5.2 Selection

A pure function in `anima-core` takes the session's model-visible messages, an optional summary, the budget, and an estimator, and returns the selected messages, the dropped count, and the estimate.

- It keeps whole turns, newest first; a turn starts at a user message. It never separates an assistant tool-call message from its tool results, and it always includes the current user message.
- It excludes silent check-in pairs and includes steer messages.
- At most 4 images in the window are sent as images; older image attachments become `[image: <name> (<path>)]` text.
- Estimates are characters ÷ 4, plus 8 tokens per message, plus serialized tool arguments and results ÷ 4, multiplied by a per-session calibration factor: the previous run's provider-reported prompt tokens divided by its estimate, clamped to 0.5–2.0.

### 5.3 Trimmed indicator

When turns are dropped and no summary covers them, the session's `contextTrimmed` is updated and the session view shows "Earlier messages are outside the companion's view".

### 5.4 Compaction

- Triggered automatically before a model call when the selection would drop turns not covered by the summary (agent setting `autoCompact`, default on), and manually by `/compact` or the compact route.
- A secondary call to the agent's provider and model (no tools, `maxTokens` 1,024, temperature 0.2) summarizes the previous summary plus the dropped turns into at most 8 KiB. The summary is stored on the session and injected as a context part labelled as data, not instructions. The stream shows `run.progress { phase: "compacting" }`.
- If summarizing fails, the run continues with the trimmed context and the session records the error. Usage is recorded with source `compaction`.

## 6. Event stream

`GET /api/agents/{id}/events` is Server-Sent Events with owner read authorization, `Cache-Control: no-store`, and `X-Accel-Buffering: no`. It covers the agent and its helpers.

- Each agent has its own fanout with capacity `ANIMAOS_RS_SESSION_EVENT_BUFFER` (default 1,024). At most 16 subscribers per agent; the 17th receives 429.
- The first event is `stream.snapshot` with active runs (run record, current `stepId`, text so far up to 64 KiB, tool calls in progress) and pending approvals.
- Every event carries `type`, `agentId`, optional `sessionId` and `runId`, a per-stream `seq` (also the SSE `id`), and `at`.
- Types: `session.created|updated|deleted`; `run.queued|started|progress|awaiting_approval|steered|completed|failed|cancelled|interrupted`; `step.delta`; `message.created`; `tool.started` (name, arguments preview ≤ 2 KiB); `tool.finished` (status, duration, result preview ≤ 2 KiB, truncation flag); `approval.requested|resolved`; `automation.updated`; `skill.updated`; `stream.resync`.
- Keep-alive comments every 15 seconds. A lagging subscriber receives `stream.resync` and continues with live events; the client refetches what it shows. There is no replay buffer.
- The VPS gateway already streams (`flush_interval -1`) and injects owner credentials; no gateway change is needed.

## 7. Approvals

### 7.1 Risk classes

A static table in the daemon assigns every tool a class. Unknown tools are `exec`.

- `read` (never asks): file reads and search, `todo_read`, memory search and recent, time, `calculate`, `bg_list`, `bg_output`, the roster, calendar and mail list tools, `search_conversations`, `load_skill`, `list_automations`.
- `write`: `write_file`, `edit_file`, `multi_edit`, `todo_write`, `memory_add`, `propose_skill`, `create_automation`, `pause_automation`, `mail_create_draft`, and the calendar write tools (the latter two still only create records that the owner approves in Connectors).
- `exec`: `bash`, `bg_start`, `bg_stop`.
- `network`: `web_fetch`, `exa_search`.
- `delegate`: `delegate_to_agent`, `spawn_helper`, `send_message`, `broadcast_message`.

### 7.2 Policy and rules

- Each agent has a control-plane policy mapping `write`, `exec`, `network`, and `delegate` to `allow`, `ask`, or `deny`. The default is `exec: ask` and all others `allow`. Policies are never writable by tools.
- Rules (`{ id, agentId, tool, matcher: { kind: command_prefix | path_glob | domain | any, value }, createdAtMs, fromApprovalId }`) are created by "Always allow" and managed on the Approvals page.
- Evaluation order: class `deny` → denied; matching rule or session allowance → allowed; class `ask` → ask; otherwise allowed. Denials return the tool result "Denied by owner policy".

### 7.3 Flow

- The gate sits in `execute_tool` after the existing live checks and before dispatch; no global lock is held while waiting. After approval the live checks run again before dispatch.
- Asking persists `ApprovalRequest { id, agentId, sessionId, runId, toolCallId, tool, class, arguments ≤ 16 KiB, suggestedMatcher, createdAtMs, expiresAtMs, status: pending, revision }`, sets the run to `awaiting_approval`, and emits `approval.requested`.
- `POST /api/approvals/{id}/decision` with `{ decision: allow_once | allow_session | allow_always | deny, note? ≤ 1,000 characters, matcher?, revision }` requires owner authorization, is idempotent, resolves the waiter, moves the record to the history store, and emits `approval.resolved`. `allow_session` adds a session allowance; `allow_always` creates a rule from `matcher` or the suggestion. A denial returns "Denied by owner: <note>" to the model, which continues.
- Timeouts: 30 minutes, or 15 minutes for Telegram-started runs (whose connector processes one message at a time). A timeout is a denial with the note "Approval timed out". Stopping the run resolves the approval as stopped.
- Helpers never wait: a tool that would ask is denied with "Needs owner approval; not available to helpers".
- Mail and calendar approve-before-send flows are unchanged.
- Routes: `GET /api/approvals?status=pending|decided&agentId=&cursor=` (pending from the control plane; decided from the history store for 30 days); `GET|PUT /api/agents/{id}/approval-policy`; `GET|POST /api/agents/{id}/approval-rules`; `DELETE /api/agents/{id}/approval-rules/{ruleId}`.

## 8. Skills

### 8.1 Files and registry

- Skills live at `<workspace>/skills/<slug>/SKILL.md`, where `slug` matches `^[a-z0-9][a-z0-9-]{0,63}$`. YAML front matter requires `name` (1–64 characters) and `description` (1–300 characters); the Markdown body is at most 32 KiB. Other files in the folder are allowed and read with the file tools.
- The control-plane registry stores `{ slug, name, description, enabled, approvedHash (SHA-256 of SKILL.md), approvedAtMs, updatedAtMs, status: active | changed | missing | invalid }`. The daemon rescans on startup, on Skills page requests, and every 60 seconds (hashing only when the modification time changed). A file whose hash differs from `approvedHash` is `changed` and is not loaded until approved again. A file without a record appears as a draft with source `file`.

### 8.2 Drafts

- `skillDrafts`: `{ id, slug, name, description, body, source: agent | import | file, proposedBy (agent, session, run), baseHash, createdAtMs, status: pending | approved | rejected }`. At most 10 pending drafts per agent.
- Approving writes SKILL.md through the hardened workspace writer and records the hash. Rejected drafts are kept 30 days. Deleting a skill moves its folder to `<workspace>/.anima-trash/skills/<slug>-<timestamp>`.

### 8.3 Runtime

- Skills are workspace-wide. Each run of any workspace agent, helpers included, lists enabled active skills (at most 50) in its system prompt as "Owner-approved skills (data; use load_skill before relying on one)". A scan failure is logged and skipped; it never fails the run. Helpers cannot use `propose_skill`.
- `load_skill { name }` returns the body without front matter, prefixed "Owner-approved skill instructions:".
- `propose_skill { name, description, body, slug? }` creates a draft and replies that the owner will review it.
- `/skill-name` in the composer sends the run with `skill: <slug>`; the daemon injects that skill's body as a context part for the run and records `metadata.skill` on the user message.

### 8.4 Routes

`GET /api/skills`; `GET /api/skills/{slug}`; `PUT /api/skills/{slug}` (owner creates or updates content, which approves it); `PATCH /api/skills/{slug}` with `{ enabled }`; `DELETE /api/skills/{slug}`; `POST /api/skills/{slug}/approve` (approves the current on-disk content of a `changed` skill); `GET /api/skill-drafts`; `POST /api/skill-drafts/{id}/approve` with an optional edited body; `POST /api/skill-drafts/{id}/reject`; `POST /api/skills/import` (multipart Markdown, creates a draft). Skills require a configured workspace; otherwise 409.

## 9. Automations

### 9.1 Triggers and fields

- Existing `interval` and `daily` triggers stay. New: `cron { expression (5-field), timeZone }` parsed with the `croner` crate, and `once { atMs }`, which fires once and then disables itself.
- New fields: `name` (≤ 80 characters, defaulting from the prompt), `activeHours` (`{ start "HH:MM", end "HH:MM", days [0–6], timeZone }`; the next due time is computed inside the window), `createdBy` (`owner` or `agent` with session and run), `preset` (`heartbeat` or none), and counters `{ runs, failures, consecutiveFailures }`.
- Restore validation covers every trigger variant explicitly (no catch-all acceptance).
- History: each fire writes `{ scheduleId, firedAtMs, finishedAtMs, outcome: silent | spoke | failed | stopped, runId, sessionId, errorCode, manual }` to the history store; the UI shows the latest 50.

### 9.2 Behavior

- Workspace-target automations run in their `schedule:<id>` session; connector targets use the Telegram room.
- `POST /api/agents/{id}/schedules/{sid}/run` returns 202 and fires now, bypassing the due time but keeping per-automation single-flight and global caps; it is recorded as manual.
- `POST /api/schedules/preview` with a trigger returns the next 3 fire times, so the browser never computes schedules itself.
- Silent outcomes store no task-result memory and no evaluator reflection. Today every successful run stores its reply as a 0.8-importance memory, and only the 3 newest memories reach a run, so silent check-ins crowd real memories out.
- The heartbeat preset runs every 30 minutes between 08:00 and 22:00 local time with an editable prompt that reviews open tasks, goals, and recent messages and replies `CHECKIN_OK` when nothing needs attention.

### 9.3 Companion tools

- `create_automation { name?, prompt, schedule, target?: thread | telegram, activeHours? }` returns the automation and its next run times. `list_automations {}` and `pause_automation { id }` are also provided.
- Limits: 20 automations per agent; agent-created automations must be at least 5 minutes apart, checked over their next 10 fire times.
- The web shows created automations as a notice card with Undo (delete).

## 10. Memory

- `PATCH /api/memories/{id}` with `{ content?, importance? (0–1), tags? }` uses a new `MemoryManager::update_memory` that re-indexes, then re-embeds.
- `DELETE /api/memories/{id}` forgets the memory, removes its id from the evidence of facts and relationships that cite it, and removes its embedding.
- `GET /api/memories/facts?agentId=&subject=&includeInactive=&limit=` lists temporal facts. `PATCH /api/memories/facts/{id}` supersedes a fact with a new value. `DELETE /api/memories/facts/{id}` forgets it.
- `DELETE /api/memories/entities/{id}` uses a new `MemoryManager::delete_entity` that removes relationships referencing it.
- These new routes require owner authorization. Existing memory routes keep their current authorization (section 1).
- "Save to memory" on a chat message calls `POST /api/memories` with type `Fact`, importance 0.75, tag `saved-from-chat`, and the session id.

## 11. Usage, logs, health

### 11.1 Usage records

- One history-store row per model call: `{ id, agentId, sessionId, runId, source (chat | telegram | automation | job | helper | api | title | compaction | profile | agency), provider, model, promptTokens, completionTokens, cachedPromptTokens, reasoningTokens, totalTokens, costMicros (nullable), pricingSource, durationMs, createdAtMs }`. Run steps are recorded from `StepUsage`; secondary calls (titles, compaction, profile and agency generation) are recorded explicitly.
- Cost is tokens × the model table's prices, with owner overrides via `PUT /api/usage/pricing` stored in the control plane. Unknown pricing yields null ("—"); the ChatGPT subscription yields null with `billing: subscription`; Ollama, vLLM, and other local providers yield 0.
- Adapter fixes: OpenAI-compatible streaming requests `stream_options.include_usage`; cached and reasoning token details are parsed where providers report them; Google counts thinking tokens in completion tokens; Anthropic cache read and creation tokens are parsed. New `TokenUsage` fields default to 0 so existing snapshots load.
- Routes: `GET /api/usage/summary?from=&to=&agentId=&groupBy=day|model|source|session`, `GET /api/usage/records?cursor=&limit=`, and `GET /api/usage/export.csv?from=&to=`. Session responses include usage totals. `/usage` shows the current session and today's totals.

### 11.2 Logs

A tracing layer copies formatted lines into a global ring buffer of 2,000 lines, each at most 4 KiB, with `{ seq, at, level, target, message }`. `GET /api/logs?level=&q=&after=&limit=` and `GET /api/logs/stream` (SSE) require owner read authorization. The buffer only captures what is already logged; request bodies and secrets are not logged.

### 11.3 Status and metrics

- `GET /api/status` (owner read) returns version and build revision when available, start time and uptime, readiness and issues, storage labels, history store health and pending flush count, configured providers, connector states, automation totals and failures, pending approvals, running and queued runs, event subscribers, and limits.
- `/metrics` adds running and queued runs, runs by terminal status, pending approvals, event subscribers, lagged events, history flush backlog and errors, and pruned messages.

## 12. Attachments, voice, titles, and providers

### 12.1 Attachments

- `POST /api/agents/{id}/sessions/{sid}/attachments` (multipart, one file per request, owner authorization) returns `{ attachment: { id, name, mime, size, kind: image | text | document, path, createdAtMs } }`. Attachment records live in the history store.
- Limits: images up to 10 MiB (PNG, JPEG, WebP, GIF, validated by magic bytes); text-like files up to 1 MiB (valid UTF-8; Markdown, text, CSV, JSON, YAML, XML, and source code); documents up to 25 MiB (PDF, Word, Excel, PowerPoint), saved but not read. At most 10 attachments per message. Uploads require a configured workspace; otherwise 409.
- Files are saved to `<workspace>/uploads/<YYYY-MM-DD>/<sanitized name>` with a collision suffix, through the hardened writer, and appear in Files.
- `GET /api/agents/{id}/attachments/{attachmentId}` serves the bytes with `X-Content-Type-Options: nosniff`; images are inline with their validated type; everything else is `Content-Disposition: attachment`.
- `Attachment` in `anima-core` gains an optional `mime_type`. The transcript stores references, never file bytes; image data is read from the workspace when a request is built. Every adapter renders images: Anthropic `image` blocks, OpenAI-compatible `image_url` data-URI parts, Google `inline_data` parts, Ollama `images`, ChatGPT `input_image`, and a text placeholder for the deterministic adapter. Models whose table entry has `vision: false` or no entry receive a text reference noting that the image cannot be viewed.
- Text-like attachments add a note to the user message with the saved path, telling the companion to read it with `read_file`. Documents add a note that their contents cannot be read automatically yet.

### 12.2 Voice

The composer's microphone button uses the existing `useBrowserDictation` hook, shown only where the browser supports speech recognition, with interim preview and inline errors. Replies get a Read aloud action using the browser's speech synthesis, hidden where unsupported.

### 12.3 Titles

After the first completed reply in a `chat` session whose `titleSource` is `first_message`, the daemon makes a background secondary call (agent's provider and model, no tools, `maxTokens` 32, temperature 0.2, first user message and reply truncated to 2 KiB each) asking for a 2–6 word title. The result is stripped of quotes and newlines, capped at 60 characters, saved with `titleSource: generated`, and announced with `session.updated`. It is skipped if the owner renamed the session meanwhile, failures are logged and ignored, usage is recorded with source `title`, and agent setting `autoTitle` (default on) disables it.

### 12.4 Streaming for every provider

- Google uses `:streamGenerateContent?alt=sse`, reusing `consume_sse_events`, keeping raw parts, emitting text deltas for non-thought parts, and assembling the final response for `parse_google_response` so replay metadata survives.
- Native Ollama without tools streams NDJSON from `/api/chat` with `think: false`; Ollama with tools uses the OpenAI-compatible streaming path.
- Unverified risk to check during implementation: OpenAI reasoning models may reject `max_tokens` in favor of `max_completion_tokens`.

## 13. Persistence, migration, and compatibility

### 13.1 History store

- SQLite at `ANIMAOS_RS_HISTORY_SQLITE_FILE`, defaulting to `history.sqlite` beside `ANIMAOS_RS_CONTROL_PLANE_FILE` when that is set; Postgres tables via a new migration in Postgres mode. Ephemeral mode (no control-plane file and not Postgres) uses in-memory tables capped at 100,000 rows each, dropping the oldest; hot-tail pruning is off there, so the control plane still holds every message.
- Tables: `messages` (with FTS5 in SQLite and a `tsvector` index in Postgres), `runs`, `usage`, `approvals`, `schedule_runs`, `attachments`. SQLite uses WAL mode and a dedicated connection driven through `spawn_blocking`. The schema is versioned.
- Outbox: committed messages, terminal runs, usage, decided approvals, and automation history are flushed to the history store within 1 second in batches, idempotent by id, retried with backoff. After 5 minutes of failures, readiness reports an issue. Records stay in the control plane until mirrored; on restart, unmirrored records and hot messages missing from the history store are flushed.

### 13.2 Hot tail

- A message may be pruned from the control plane when it is mirrored, is not among its session's newest 200 messages, is older than 24 hours, and is not referenced by a non-terminal Telegram outbound record, a pending approval, or an active run. Pruning runs every 10 minutes inside a normal transaction and is disabled in ephemeral mode.
- Terminal outbound records whose message was pruned are marked `messagePruned`, and snapshot validation accepts that flag for terminal records only.
- Each agent's stored event log keeps the newest 500 events; `event_count` keeps the running total. No daemon code reads the log beyond its count.

### 13.3 Migration

On the first start of the new version:

1. Write a backup before changing anything: JSON mode copies the snapshot to `<file>.pre-sessions.bak` with fsync; Postgres mode inserts a `control_plane.backup.<version>` row. Then bump the snapshot version.
2. Create session records for existing rooms by the rules in section 3.1. Relabel legacy per-tick check-in rooms (generated rooms whose user message has `metadata.kind = "checkin"` and `metadata.id = <scheduleId>`) to `schedule:<scheduleId>`; nothing else references those rooms. Legacy `direct:<agentId>` stays as a chat titled from its first user message. Agent-to-agent rooms become read-only helper sessions.
3. Mirror existing messages to the history store in the background, resumably. Pruning stays disabled until mirroring completes.
4. Cap event logs. Default all new fields. Existing files under `skills/` appear as drafts for review.
5. Grant the new tools to existing agents the way the web access profiles will: read-class tools (`search_conversations`, `load_skill`, `list_automations`) to every non-helper agent, and write-class tools (`propose_skill`, `create_automation`, `pause_automation`) to agents that already have `write_file`. The web's Observe, Collaborate, and Operate profiles are updated to match.

Nothing is deleted. Rolling back to an older daemon requires restoring the backup, because older daemons refuse newer snapshot versions; the VPS upgrade documentation says so.

### 13.4 Compatibility

- Existing route shapes are unchanged; `GET /api/agents` gains `view=summary`, and after pruning snapshots carry only the hot tail (documented in OpenAPI and the daemon README).
- SDK changes are additive. A web build facing an older daemon (404 from `GET /api/status`) shows "Update the daemon" instead of failing.
- Every new route has an OpenAPI entry. The daemon README route table is regenerated from the router.

## 14. Security

- **Workspace writer fix (prerequisite):** a confirmed escape lets `write_file` write outside the workspace through a path such as `newdir/../../file`, because the check canonicalizes only the nearest existing ancestor and then `create_dir_all` creates the missing directory. Reject `..`, root, and prefix components in relative write paths, re-verify the canonical parent after creating directories, and refuse a symlinked final component. Every workspace writer (tools, skills, uploads, todos) uses this path.
- Owner authorization on every new route; reads are `no-store`.
- Uploads are validated by magic bytes, served with `nosniff`, never rendered as HTML, and stored under sanitized names.
- Skills load only with a matching approved hash; skill text, search results, summaries, and tool previews are framed as data.
- Approval decisions require owner authorization and the current revision, re-run live checks, and are idempotent.
- Rate limits: 60 session creations and 30 uploads per minute per agent, plus the queue caps.

## 15. Web UI

### 15.1 Shell and routing

- Desktop sidebar, top to bottom: identity; New chat and Search; primary destinations (Approvals with a pending badge, Automations, Memory, Skills, Work, Files, Connectors); a collapsible System group (Usage, Logs, Health, Capabilities); the sessions list; Settings.
- The sessions list has a search box and kind filter chips (All, Chats, Telegram, Check-ins, Jobs, Helpers; only kinds that exist); groups by Today, Yesterday, Previous 7 days, and Older; shows archived sessions behind "Show archived"; nests helper sessions under their parent chat; and shows a pulse for active runs, a dot for unread, and an approval badge. Row menu: Rename, Archive, Export Markdown, Delete (with confirmation noting that memories are kept).
- Mobile: a sessions drawer from the top bar and a bottom dock with Chats, Approvals, Automations, Memory, and More.
- Hash routes: `#/s/<sessionId>`, `#/approvals`, `#/automations`, `#/memory`, `#/skills`, `#/work`, `#/files`, `#/connectors`, `#/usage`, `#/logs`, `#/health`, `#/capabilities`. No router library. Reload restores the route and rejoins live runs.
- Removed: the Telegram destination and `TelegramThread` (Telegram is a session; settings stay in Connectors), `ActivityView` and `CheckinsView` (replaced by Automations and Usage), the "Delegated work" drawer, and grey pills for tool messages. The hidden legacy views are untouched.

### 15.2 Session view

- Header: title (rename in place for chats), kind badge (for example "Check-in · every 30 min" or "Job · needs review"), model, usage totals, and context actions (Export, Archive, "Open in Work › Runs", "Edit automation").
- History loads the newest page and older pages on scroll without jumping.
- Live runs: streamed text re-rendered at most once per animation frame; a "Working · N steps · Ns" block with a card per tool step (name, short arguments, spinner then ✓ or ✗, duration, expandable result), collapsing to "Used N tools · Ns"; the same cards for historical tool messages; helper cards with live status and "Open session"; inline approval cards with the four decisions; automation notice cards with Undo; revised drafts collapsed.
- Outcomes: stopped partial text labelled "Stopped"; failures with Retry; interrupted runs with Send again and a warning when tools had started; the context-trimmed divider with a Compact action.
- Message actions: Copy, Save to memory, Read aloud. Attachments render as image thumbnails and file chips.
- Read-only kinds show a footer note instead of the composer.

### 15.3 Composer

- Usable while the companion works. Enter sends or queues; ⌘/Ctrl+Enter steers when a run is active. Queued messages appear as pending bubbles with a cancel control. During this session's run, Send becomes Stop.
- Attachments by paperclip, drag and drop, or paste, with upload progress and removal. Microphone button for dictation.
- Slash commands via autocomplete at the start of the input: `/new`, `/stop`, `/rename`, `/archive`, `/export`, `/search`, `/model`, `/compact`, `/usage`, `/help`, and `/<skill-slug>` for each enabled skill. Text that matches no command is sent as a normal message.
- Labels: "Reply on Telegram" and "Reply to this check-in" in those sessions.
- ⌘K adds New chat, session titles, Review approvals, and the pages, alongside the prompt library.

### 15.4 Pages

- **Approvals:** pending cards, 30-day decided history, and rules per class with policy controls.
- **Automations:** list with next run, last outcome, and failures; create and edit with plain-language entry (a deterministic browser parser for phrases such as "every 2 hours", "weekdays at 9am", "every monday at 8:30", "tomorrow at 15:00", "in 20 minutes"), a cron field, a daemon-computed preview of the next 3 runs, active hours, target, and the heartbeat preset; Run now, pause and resume, delete; a history drawer.
- **Memory:** Memories (search, type filter, sort, edit, delete, evidence trace), About you (facts and preferences), People & things (entities and relationships).
- **Skills:** list with enable toggles and status, drafts with a diff and Approve / Edit / Reject, an editor with preview, New, Delete, and Import.
- **Usage:** 7, 30, or 90 days; totals; a per-day bar chart drawn as inline SVG; tables by model, by source, and top sessions; CSV export.
- **Logs:** live tail with level filter, search, pause, and copy.
- **Health:** cards for readiness, storage and history store, providers, connectors, failing automations, pending approvals, runs, version, and uptime.

### 15.5 Data layer and quality

- All new daemon calls go through the SDK. Hooks: `useAgentEvents` (one SSE connection per companion with reconnect back-off from 1 to 30 seconds with jitter, shared by subscribers), `useCompanionSessions`, `useApprovals`, `useAutomations`, `useSkills`, `useMemory`, `useUsage`, `useLogs`, and `useStatus`. One pure reducer in `lib/session-events.ts` applies stream events and tolerates duplicate and out-of-order events by `seq` and id.
- Once the stream is connected, bootstrap polling switches to `view=summary` every 30 seconds. Drafts are saved per session in session storage; read state is saved on the server.
- Failed sends retry automatically with the same idempotency key, up to 3 times with back-off; the text stays as a pending bubble until accepted.
- Styles reuse the existing palette and `studio-*` components, with new files imported from `styles.css` that must pass `visual-tokens.test.ts`. The sessions list supports arrow keys and `aria-current`; tool cards are expandable buttons; screen readers get one polite announcement when a reply finishes.

## 16. Limits and errors

| Item | Limit or behavior |
|---|---|
| Queued runs per agent | 8; 429 beyond |
| Concurrent runs per agent | 3 (configurable); helpers 1 |
| Run input | 32 KiB text, 10 attachments |
| Stream | 1,024-event buffer, 16 subscribers, 50 ms / 512-byte delta flush, 2 KiB previews |
| Context | 60% of the model window, 32k fallback, 4 images |
| Hot tail | newest 200 per session, 24 hours, mirrored, unreferenced |
| Event log | 500 per agent |
| Approvals | 30-minute timeout, 15 for Telegram-started runs |
| Skills | 32 KiB body, 50 in the index, 10 pending drafts per agent |
| Automations | 20 per agent, 5-minute minimum for agent-created, 50 history entries shown |
| Logs | 2,000 lines of up to 4 KiB |
| Uploads | 10 MiB images, 1 MiB text, 25 MiB documents |
| Titles | 2–6 words, 60 characters |

Errors surface specifically: stream drops show "Reconnecting…" and resume from the snapshot; 429 queue full; 503 save failure returns the text to the composer; a session deleted elsewhere returns the view to the list; deleting a running session is 409; history store failures keep data in the control plane, raise metrics, and appear in readiness; skills scan and title failures never fail a run.

## 17. Testing and verification

- **anima-core:** stream frames per step; stop at every checkpoint including mid-`bash`; no tool call without a result; steering drain and conversion to queued runs; context selection (turn boundaries, tool pairs, budget, calibration, image cap); event-log cap.
- **anima-model-adapters:** Google SSE and Ollama NDJSON streaming with fixtures; usage parsing including cached, reasoning, and thinking tokens; `include_usage`; image rendering per provider; model table lookup and cost estimation.
- **anima-memory:** update with re-indexing; entity deletion cascade; citation cleanup.
- **Daemon:** parallel sessions (cross-session concurrency, same-session order, change-set commit and rollback, derived status, deletion during runs, helper slots, todo compare-and-swap), rewriting the serialization tests identified in the parallel-runs assessment; every new route including 403 for non-owners, paging, idempotency, 429, and queue order; restart recovery for runs and approvals; the event stream (snapshot on connect, deterministic event order, resync on lag, subscriber cap); stop semantics for web, schedule, Telegram (Stopped, Suppressed, never re-run), and jobs (marker, NeedsReview); approvals (policy precedence, rules, session allowances, timeout, helper denial, live re-check); skills (hash pinning, changed-file hold, drafts, index injection, `load_skill`, `/skill`); automations (cron, once, active hours, run now, history, tools and limits, silent-memory skip); memory routes; usage records and pricing; logs; status; attachments (validation, storage, per-provider rendering); titles; compaction; conversation search; history store outbox crash safety at each step; hot-tail pruning and `messagePruned` validation; migration from older snapshot versions including the backup; the workspace writer fix.
- **SDK:** every new client and stream parsing.
- **Web:** the reducer; sidebar grouping, filters, and nesting; session view states; composer queue, steer, stop, slash commands, attachments, and dictation; every page; hash routing; updated `ViewHarness` and `WorkspaceShell` tests.
- **Playwright (simulated APIs and stream):** send → stream → tool card → done; approval card flow; Stop mid-run; reload mid-run and rejoin; the mobile drawer.
- **Deployment:** `docker compose --env-file <test-config> -f deploy/vps/compose.yaml config --quiet` with the new variable, and an isolated container smoke test covering sessions after restart and the stream through Caddy.
- **Commands:** `bun install` first (the checkout's `node_modules` is stale); `bun x nx run rust-daemon:test --skipNxCache`; the SDK and web `test`, `typecheck`, and `build` targets with `--skipNxCache` (confirm names with `bun x nx show projects --json`); the web-e2e target.
- **Manual acceptance (not claimed from automated tests):** streaming, attachments, and titles against a real provider; the VPS deployment.

## 18. Milestones

Delivered in order on one branch, each ending with its relevant checks green and a commit series; no pause for approval between milestones.

1. **M0 Security and groundwork:** workspace writer fix; event-log cap; model table and cost estimation; `TokenUsage` fields and adapter usage fixes.
2. **M1 Run coordinator:** parallel-safe execution, change-set commit and rollback, session locks and agent slots, run ledger, restart recovery, and the legacy route on the coordinator.
3. **M2 Sessions:** registry, migration, routes, history store with outbox, mirroring, paging and search, check-in rooms, helper linkage, `view=summary`; SDK clients; web shell, hash routing, sidebar, and session view on blocking runs.
4. **M3 Live runs:** runtime streaming, streaming for every provider, event stream, async runs, queue, steer, stop for every source, context selection, compaction, and titles; SDK; web live rendering and composer.
5. **M4 Approvals** across daemon, SDK, and web.
6. **M5 Skills** across daemon, SDK, and web.
7. **M6 Automations** across daemon, SDK, and web.
8. **M7 Memory** across crate, daemon, SDK, and web.
9. **M8 Usage, logs, health, status, and metrics** across daemon, SDK, and web.
10. **M9 Attachments and voice:** upload route, per-provider image rendering, composer attachments, dictation, and Read aloud.
11. **M10 Deployment and documentation:** VPS compose and backup/upgrade docs, the daemon README route table, OpenAPI review, the web README, and full verification.
