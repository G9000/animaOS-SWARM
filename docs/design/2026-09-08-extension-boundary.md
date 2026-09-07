# Extension boundary for the persistent workplace

## What exists today

`packages/core-rust/crates/anima-core/src/capability.rs` defines the portable `CapabilityManifest`, `CapabilityManifestInput`, `ManifestCatalog`, and execution interfaces. The manifest includes input/output schemas, compatibility versions, declared host permissions, secret reference names, side effects, timeout, cancellation, retries, recovery mode, and artifact/citation support. Its constructor derives a schema digest; callers must not invent one.

`hosts/rust-daemon/src/tools.rs` separately owns the daemon's registered native tool handlers. The new `GET /api/capabilities` lists these real registrations. It is a read-only discovery API, not installation, permission assignment, connection validation, or invocation of the portable registry.

`packages/mod-sdk` is the existing TypeScript mod authoring helper. Its `defineModTool` types a TypeScript handler; it is not a loader for the Rust daemon. Do not present a TypeScript mod as daemon-installed until a real execution bridge exists.

## The next executable extension slice

Build the bridge inside the daemon while reusing the portable manifest:

1. **Discover and validate.** Read an owner-selected package, validate manifest schemas and compatibility, and identify its exact version. Treat package text as untrusted content.
2. **Register, without granting.** Store the manifest and bind an executor. Installation must not add any tool to an agent's allowed tools, enable a schedule, or request credentials implicitly.
3. **Configure prerequisites.** Resolve named secrets through daemon-owned credential storage. Show missing connections separately from permission denials. Do not include secret values in inventory, browser state, manifests, or diagnostics.
4. **Authorize each invocation.** Intersect human grants, current agent grants, and any delegator's current authority. A manager may narrow/delegate its authority; it cannot mint owner authority. Device/resource scope needs enforcement beneath model prompts.
5. **Execute with bounds.** Bind the existing portable registry to a host executor with a deadline, cancellation, output limits, and declared recovery. Arbitrary third-party native code needs a process boundary and a concrete sandbox; a manifest does not provide isolation.
6. **Record results durably.** Bind the lineage and result-recorder interfaces to durable host storage. A restart must preserve uncertain outcomes for reconciliation or review instead of automatically repeating side effects.
7. **Disable and remove.** Prevent new calls immediately, define active-call cancellation behavior, retain audit evidence according to policy, and remove credentials only through their owned lifecycle.

These are implementation tasks, not capabilities delivered by the inventory endpoint.

## First module contract: camera observation

Use a read-only, bounded snapshot capability before continuous streaming. Input identifies an authorized device; output references an artifact rather than embedding an unlimited image stream. Camera viewing, recording/retention, microphone capture, and speaking are separate grants. No facial identity assumption is needed for the initial capability. A disabled module must stop producing observations. A future voice module can consume approved observations without inheriting camera authority.

The camera, voice, smart-home, browser-control, document-engine, and external coding-agent entries are roadmap declarations only. Do not add an Install button until installation and execution are implemented.

## Native tools and document boundary

The daemon already registers workspace search/read/write, exact and multi-edit, foreground/background shell tools, memory tools, team tools, fetch/search, and productivity connectors. Native means first-party supported execution; tools still need grants and their documented prerequisites. Shell access grants the process the daemon user's OS authority: workspace cwd is not an OS sandbox.

Text reading and file creation do not imply format-aware PDF, Word, or spreadsheet support. A dedicated document engine needs bounded extraction, page/sheet references, explicit scanned-file/OCR handling, output validation, and artifact ownership. Until then, arbitrary installed terminal utilities are user-configured tooling rather than a guaranteed document API.

## Capability discovery API

`GET /api/capabilities` requires the same local-owner read authorization as workspace files, plus global API authentication when configured. Responses use `Cache-Control: no-store` and contain no paths, credentials, or agent memory. Tools are sorted by name and taken from `ToolRegistry`; category and requirements are presentation metadata.

- `schemaVersion`: currently 1.
- `tools`: registered native handlers, descriptions, category, and human-readable prerequisites. No grant or readiness boolean is inferred.
- `persistence.controlPlane` / `memory`: configured storage labels (`ephemeral`, `json`, `sqlite`, or `postgres`), not health probes.
- `persistence.executionJournal`: a database step adapter is configured. This does not certify full durable-engine integration or automatic resume.
- `extensions`: explicitly planned modules.
- `limitations`: known execution/document/installation boundaries.

Success is 200. Owner authorization failure is 403; global authentication may return 401 before the handler. Discovery has no writes and never enables proactive work. Unknown future category values should be handled defensively by the UI.

## Startup acceptance scenario

Use deterministic test adapters and a temporary workspace first. Create an operator and builder with bounded native grants. Assign a small source-file change, require a saved artifact and test evidence, and record the result. Verify that peer/delegated requests cannot amplify privileges or loop without limit. Interrupt a scheduled execution and confirm it becomes reviewable rather than silently replayed. Verify saved tasks/configuration and memories independently across a reload. Publishing, customer messaging, and spending require the explicitly configured owner authority.

A complete unattended startup demonstration still needs durable job ownership, artifact review, real external integrations, and restart acceptance beyond the scaffold. Passing unit tests does not establish that broader scenario.
