# AnimaOS Kit Agent Guide

This repo is a host-agnostic agent runtime workspace. Keep the engine/runtime packages separate from runnable backend hosts.

# Repo Boundaries

- `packages/core-rust` is the Rust host-agnostic core workspace. It contains reusable Rust engine crates such as `anima-core`, `anima-memory`, and `anima-swarm`.
- `packages/core-ts` is the TypeScript host-agnostic core package. Its npm package name is still `@animaOS-SWARM/core`.
- `packages/*` is for reusable libraries, SDKs, runtime ports, and shared tooling.
- `hosts/*` is for runnable backend host processes only. Hosts wrap a core implementation and expose it over a runtime boundary such as HTTP, WebSocket, SSE, jobs, or another process interface.
- `hosts/rust-daemon` is the ready Rust host. `hosts/elixir-phoenix` and `hosts/python-service` are placeholder host projects until implemented.
- `apps/*` is for user-facing app surfaces such as the UI, UI e2e app, and the legacy local TypeScript server.
- `tools/*` is for workspace tooling such as the `workspace-dev` launcher.

# Architecture Rules

- Do not move reusable engine/runtime library code into `hosts/*`.
- If code is intended to be reused by multiple hosts, put it under `packages/*`.
- Do not make `anima-core` depend on an HTTP framework, DB driver, or host-specific runtime.
- Do not remove or retarget `apps/server` unless explicitly asked. It is retained during the restructuring even though the long-term backend host boundary is `hosts/*`.
- Keep host selection centralized in `tools/workspace-dev` rather than hardcoding host ports or project names in UI/client packages.

# Dev Workflow

- Use `bun dev --host rust` for the normal local workflow. It runs the selected host plus the web UI through `workspace-dev`.
- The current supported host keys are `rust`, `elixir`, and `python`; only `rust` is production-ready today.
- Use Nx project targets for host work: `bun x nx run rust-daemon:dev`, `bun x nx run rust-daemon:build`, `bun x nx run rust-daemon:test`, and `bun x nx run rust-daemon:lint`.
- Use `bun x nx test workspace-dev` when changing host selection, process orchestration, or dev launcher behavior.
- Use `bun x nx show projects --json` to confirm project names before assuming target names or paths.
- Prefer `bun x nx ...` over direct tool commands for build/test/lint tasks when an Nx target exists.

# Verification

- For Rust host/core changes, run `bun x nx run rust-daemon:test --skipNxCache`.
- For dev launcher changes, run `bun x nx test workspace-dev --runInBand --skipNxCache`.
- For TypeScript package changes, use the relevant Nx `test`, `build`, or `typecheck` target.
- Do not claim completion unless the relevant verification commands have passed in the current tree.

<!-- nx configuration start-->
<!-- Leave the start & end comments to automatically receive updates. -->

# General Guidelines for working with Nx

- For navigating/exploring the workspace, invoke the `nx-workspace` skill first - it has patterns for querying projects, targets, and dependencies
- When running tasks (for example build, lint, test, e2e, etc.), always prefer running the task through `nx` (i.e. `nx run`, `nx run-many`, `nx affected`) instead of using the underlying tooling directly
- Prefix nx commands with the workspace's package manager (e.g., `bun x nx build`) - avoids using globally installed CLI
- You have access to the Nx MCP server and its tools, use them to help the user
- For Nx plugin best practices, check `node_modules/@nx/<plugin>/PLUGIN.md`. Not all plugins have this file - proceed without it if unavailable.
- NEVER guess CLI flags - always check nx_docs or `--help` first when unsure

## Scaffolding & Generators

- For scaffolding tasks (creating apps, libs, project structure, setup), ALWAYS invoke the `nx-generate` skill FIRST before exploring or calling MCP tools

## When to use nx_docs

- USE for: advanced config options, unfamiliar flags, migration guides, plugin configuration, edge cases
- DON'T USE for: basic generator syntax (`nx g @nx/react:app`), standard commands, things you already know
- The `nx-generate` skill handles generator discovery internally - don't call nx_docs just to look up generator syntax

<!-- nx configuration end-->
