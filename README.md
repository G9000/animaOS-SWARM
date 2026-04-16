# animaOS-SWARM

Agent swarm framework. Command and control your AI agents -- spawn, coordinate, and manage swarms that get things done.

## Quick Start (CLI + TUI)

Create and run an agency in 3 steps - **no code required**:

```bash
# 1. Build the CLI
bun run build:cli-sdk

# 2. Create an agency (no daemon needed)
export OPENAI_API_KEY="sk-..."
bun run animaos create my-agency --provider openai --model gpt-4o-mini

# 3. Start daemon & launch tasks in the TUI
bun run daemon          # In a new terminal
cd my-agency
bun run animaos launch "Write a blog post about AI"
```

See [docs/SDK_USAGE.md](docs/SDK_USAGE.md) for full documentation and SDK examples.

The terminal UI is the primary local operator surface. The web UI is secondary and currently less mature.

## Runtime Architecture

The canonical runtime lives in Rust under `hosts/rust-daemon`.

- `anima-core`, `anima-swarm`, `anima-memory`, and `anima-daemon` own execution, coordination, memory, and the HTTP/SSE boundary.
- `packages/sdk` is the public TypeScript client for that runtime.
- `packages/core` is shared TypeScript support used by the SDK, CLI, and UI. It is not the source of truth for execution behavior.

## Development

```bash
# Build all packages
bun run build

# Run tests
bun run test

# Run only what changed on the current branch
bun run affected:build
bun run affected:test

# Open the project graph
bun run graph

# Start the selected host and UI together
bun dev --host rust

# Start the daemon only for local launch/chat workflows
bun run daemon

# Start the server app
bun run server

# Start the web UI (secondary surface)
bun run ui
```

## Local CLI

Prefer the repo-local CLI while developing. Bun is only the local script runner here; the actual CLI being executed is this workspace's `animaos` build. A globally linked `animaos` can work too, but only if it is linked to this workspace's CLI package; otherwise it may resolve to a different binary.

`create` is a local CLI flow and does not require the Rust daemon. `launch`, `run`, and `chat` are daemon-backed. `launch` is the primary TUI flow; use `--no-tui` only for automation or plain-text execution.

Plain-text launch (`--no-tui`) supports both single-shot and interactive use:

- `bun run animaos launch --no-tui "your task"` runs one daemon-backed task with plain-text output.
- `bun run animaos launch --no-tui` opens an interactive prompt with `exit`, `/help`, and `/health`.
- If daemon health is unavailable at startup, launch prints a warning that tasks will fail until the daemon is reachable; once connectivity returns, it prints a one-line recovery message before continuing.

```bash
# Build the local SDK/CLI runtime used by the workspace package entrypoints
bun run build:cli-sdk

# Set your provider key
export OPENAI_API_KEY=...

# Create an agency locally (no daemon required)
bun run animaos create content-team --provider openai --model gpt-4o-mini

# Equivalent direct invocation without Bun as the runner
node packages/cli/dist/index.js create content-team --provider openai --model gpt-4o-mini

# Start the Rust daemon in another terminal for launch/run/chat
bun run daemon

cd content-team
bun run animaos launch "your task"

# Plain-text single-shot execution
bun run animaos launch --no-tui "your task"

# Plain-text interactive prompt (/help, /health, exit)
bun run animaos launch --no-tui
```

## Provider Support

`create`, `launch`, `run`, and `chat` now share the same main provider family.

| Provider      | Accepted names     | Key env vars                                                                                      | Base URL env var      | Notes                                         |
| ------------- | ------------------ | ------------------------------------------------------------------------------------------------- | --------------------- | --------------------------------------------- |
| OpenAI        | `openai`           | `OPENAI_API_KEY`, `OPENAI_KEY`, `OPENAI_TOKEN`                                                    | `OPENAI_BASE_URL`     | Native OpenAI chat completions                |
| Anthropic     | `anthropic`        | `ANTHROPIC_API_KEY`, `ANTHROPIC_KEY`, `ANTHROPIC_TOKEN`, `CLAUDE_API_KEY`                         | `ANTHROPIC_BASE_URL`  | Native Anthropic Messages API                 |
| Google Gemini | `google`, `gemini` | `GOOGLE_API_KEY`, `GOOGLE_KEY`, `GOOGLE_AI_KEY`, `GEMINI_API_KEY`, `GOOGLE_GENERATIVE_AI_API_KEY` | `GOOGLE_BASE_URL`     | Native Google `generateContent` API           |
| Ollama        | `ollama`           | `OLLAMA_API_KEY`                                                                                  | `OLLAMA_BASE_URL`     | Local inference; OpenAI-compatible HTTP shape |
| Groq          | `groq`             | `GROQ_API_KEY`, `GROQ_KEY`, `GROQ_TOKEN`                                                          | `GROQ_BASE_URL`       | OpenAI-compatible                             |
| xAI           | `xai`, `grok`      | `XAI_API_KEY`, `XAI_KEY`, `GROK_API_KEY`                                                          | `XAI_BASE_URL`        | OpenAI-compatible                             |
| OpenRouter    | `openrouter`       | `OPENROUTER_API_KEY`, `OPENROUTER_KEY`, `OPENROUTER_TOKEN`                                        | `OPENROUTER_BASE_URL` | OpenAI-compatible                             |
| Mistral       | `mistral`          | `MISTRAL_API_KEY`, `MISTRAL_KEY`, `MISTRAL_TOKEN`                                                 | `MISTRAL_BASE_URL`    | OpenAI-compatible                             |
| Together      | `together`         | `TOGETHER_API_KEY`, `TOGETHER_KEY`, `TOGETHER_TOKEN`                                              | `TOGETHER_BASE_URL`   | OpenAI-compatible                             |
| DeepSeek      | `deepseek`         | `DEEPSEEK_API_KEY`                                                                                | `DEEPSEEK_BASE_URL`   | OpenAI-compatible                             |
| Fireworks     | `fireworks`        | `FIREWORKS_API_KEY`                                                                               | `FIREWORKS_BASE_URL`  | OpenAI-compatible                             |
| Perplexity    | `perplexity`       | `PERPLEXITY_API_KEY`                                                                              | `PERPLEXITY_BASE_URL` | OpenAI-compatible                             |

Notes:

- `--api-key` overrides the environment for the current CLI invocation.
- For daemon-backed commands, credentials are forwarded per request; changing shells or keys does not require restarting the daemon.
- The alias handling follows the same general pattern ElizaOS uses for provider secret names.

## Tech Stack

Bun, TypeScript, Nx, Vitest, Vite, React

## License

MIT
