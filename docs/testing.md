# Testing Commands

Run commands from the repository root. The workspace package manager is Bun, so Nx commands should be prefixed with `bun x`.

## Memory validation

Fast deterministic memory regression gate:

```bash
NX_TUI=false bun x nx run core-rust:memory-eval --skipNxCache
```

Focused temporal fact/relationship CRUD and supersession checks:

```bash
cargo test --manifest-path Cargo.toml --target-dir target/core-rust/memory-temporal -p anima-memory temporal -- --nocapture
```

LOCOMO-style long-memory smoke benchmark:

```bash
NX_TUI=false bun x nx run core-rust:memory-locomo --skipNxCache
```

Fetch the public LOCOMO CSV into the local `.cache/locomo` directory:

```bash
NX_TUI=false bun x nx run core-rust:memory-locomo-fetch --skipNxCache
```

The fetch target writes `.cache/locomo/locomo.csv`, `.cache/locomo/locomo_dataset.json`, and `.cache/locomo/manifest.json`. The cache directory is git-ignored. Use `LOCOMO_DATASET_URL` to point at a different upstream CSV artifact, `LOCOMO_BENCHMARK_DATASET_URL` to point at a different labeled benchmark JSON artifact, and `LOCOMO_CACHE_DIR` to choose a different local cache directory. The default Hugging Face CSV does not declare a redistribution license in its dataset card, so keep it as a local cache unless upstream terms are confirmed.

Production-grade LOCOMO retrieval benchmark over the labeled dataset:

```bash
NX_TUI=false bun x nx run core-rust:memory-locomo-dataset --skipNxCache
```

That command fetches the dataset if missing, populates `MemoryManager` with all conversation turns from `locomo_dataset.json`, evaluates category 1-4 questions against official evidence turn IDs, and prints retrieval profiles: `core` (`MemoryManager::new()`, multilingual analyzer, no expansion), `locomo-tuned` (`MemoryManager::with_query_expander(locomo_query_expander())`), `locomo-temporal-*` profiles that seed temporal facts from conversation-only profile/preference cues and sweep temporal recall weight, and `locomo-temporal-rerank-*` profiles that retrieve a broader candidate pool before reranking temporal evidence back down to `LOCOMO_TOP_K`. The default quality thresholds apply only to `locomo-tuned`; the printed delta shows how much the LOCOMO-specific expansion changes hit rate and MRR, and the temporal profiles show whether seeded temporal facts improve category 3 without hurting the other categories. Tunables:

```bash
LOCOMO_TOP_K=20
LOCOMO_MIN_HIT_RATE=0.70
LOCOMO_MIN_CATEGORY_HIT_RATE=0.50
LOCOMO_MIN_MRR=0.40
LOCOMO_MIN_QUESTIONS=1500
LOCOMO_MIN_TURNS=5000
LOCOMO_TEMPORAL_WEIGHT_SWEEP=0.025,0.05,0.075,0.10
LOCOMO_TEMPORAL_RERANK_WEIGHT_SWEEP=0.075
LOCOMO_TEMPORAL_RERANK_BONUS=0.02
LOCOMO_MISS_REPORT_CATEGORY=3
LOCOMO_MISS_REPORT_LIMIT=5
LOCOMO_MISS_REPORT_TOP_K=20
```

Set `LOCOMO_MIN_CORE_HIT_RATE` or `LOCOMO_MIN_CORE_MRR` when you want the pure core profile to be a hard gate. Set `LOCOMO_MISS_REPORT_CATEGORY=3` or `LOCOMO_CATEGORY3_MISS_REPORT=1` to print a bounded miss report for the best temporal profile in that category, including official evidence turn snippets, question relation labels, matched relation coverage, retrieved rows, resolved entity IDs, and whether seeded temporal facts referenced the expected evidence turns. The deterministic LOCOMO targets compile the `anima-memory/locomo-eval` feature and opt into `locomo_query_expander()` explicitly. Default production BM25 search uses the multilingual analyzer and remains domain-neutral unless a host passes a `QueryExpander`; core search does not remove stop words or stem terms by language.

End-to-end LOCOMO agent benchmark with real model calls:

```bash
LOCOMO_AGENT_PROVIDER=openai \
LOCOMO_AGENT_MODEL=gpt-4.1-mini \
LOCOMO_JUDGE_PROVIDER=openai \
LOCOMO_JUDGE_MODEL=gpt-4.1 \
OPENAI_API_KEY=sk-... \
NX_TUI=false bun x nx run rust-daemon:memory-locomo-agent --skipNxCache
```

This is the expensive full-stack benchmark. It starts an isolated Rust daemon when `LOCOMO_DAEMON_URL` is not set, ingests LOCOMO turns through the SDK/daemon memory API, retrieves evidence through daemon recall, asks a real answer model through daemon agent runs, judges answers with a real judge model through daemon agent runs, writes a detailed JSON report under `.cache/locomo-agent`, and fails if thresholds are not met. It intentionally fails without real provider configuration; it does not use the deterministic test model as a fallback.

The auto-started daemon sets `ANIMAOS_RS_MEMORY_QUERY_EXPANDER=locomo` so LOCOMO-specific query expansion is scoped to the benchmark. If `LOCOMO_DAEMON_URL` points at an already running daemon, start that daemon with `ANIMAOS_RS_MEMORY_QUERY_EXPANDER=locomo` to reproduce the same retrieval profile, or leave it unset to measure the domain-neutral default.

Required model settings:

```bash
LOCOMO_AGENT_PROVIDER=openai
LOCOMO_AGENT_MODEL=gpt-4.1-mini
LOCOMO_JUDGE_PROVIDER=openai
LOCOMO_JUDGE_MODEL=gpt-4.1
OPENAI_API_KEY=sk-...
```

Useful tunables:

```bash
LOCOMO_AGENT_TOP_K=40
LOCOMO_AGENT_MIN_QUESTIONS=1500
LOCOMO_AGENT_MIN_ACCURACY=0.45
LOCOMO_AGENT_MIN_RETRIEVAL_HIT_RATE=0.70
LOCOMO_AGENT_MIN_CATEGORY_ACCURACY=0.20
LOCOMO_AGENT_MAX_JUDGE_FAILURE_RATE=0
LOCOMO_AGENT_CONCURRENCY=1
LOCOMO_INGEST_CONCURRENCY=16
LOCOMO_AGENT_ANSWER_MAX_TOKENS=180
LOCOMO_AGENT_JUDGE_MAX_TOKENS=240
LOCOMO_AGENT_ANSWER_EVIDENCE_LIMIT=18
LOCOMO_AGENT_ANSWER_EVIDENCE_CHAR_BUDGET=5600
LOCOMO_AGENT_REPORT_EVIDENCE=0
ANIMAOS_RS_MEMORY_QUERY_EXPANDER=locomo
ANIMAOS_RS_MEMORY_TEXT_ANALYZER=multilingual
```

`LOCOMO_AGENT_TOP_K` controls broad candidate recall. The answer prompt receives a smaller reranked evidence window controlled by `LOCOMO_AGENT_ANSWER_EVIDENCE_LIMIT` and `LOCOMO_AGENT_ANSWER_EVIDENCE_CHAR_BUDGET`, so production-style runs can preserve recall without flooding the answer model with every candidate.

Some local reasoning-heavy models, including larger Gemma variants served through Ollama, may need a larger answer or judge token budget to emit final visible text instead of stopping after internal reasoning. For those models, try `LOCOMO_AGENT_ANSWER_MAX_TOKENS=512` and `LOCOMO_AGENT_JUDGE_MAX_TOKENS=512`.

Validated local Gemma/Ollama 100-question command:

```bash
LOCOMO_AGENT_PROVIDER=ollama \
LOCOMO_AGENT_MODEL=gemma4:31b \
LOCOMO_JUDGE_PROVIDER=ollama \
LOCOMO_JUDGE_MODEL=gemma4:31b \
LOCOMO_AGENT_MAX_QUESTIONS=100 \
LOCOMO_AGENT_MIN_QUESTIONS=100 \
LOCOMO_AGENT_MIN_ACCURACY=0.80 \
LOCOMO_AGENT_ANSWER_MAX_TOKENS=512 \
LOCOMO_AGENT_JUDGE_MAX_TOKENS=512 \
NX_TUI=false bun x nx run rust-daemon:memory-locomo-agent --skipNxCache
```

The May 2026 validated run reached `0.900` answer accuracy, `0.940` retrieval hit rate, `0.860` all-evidence hit rate, `0.900` answer-evidence hit rate, `0.161` labeled answer-evidence precision, `7.7` average answer evidence lines, and `0.000` judge failure rate on 100 questions. Category accuracies were: category 1 `0.806`, category 2 `0.919`, category 3 `1.000`, and category 4 `0.952` after the precision reranker, relation-specific extraction prompts, and the explicit LOCOMO query expander.

If a local model still burns output budget without producing visible answer text, reduce the reranked answer prompt size with `LOCOMO_AGENT_ANSWER_EVIDENCE_LIMIT` or `LOCOMO_AGENT_ANSWER_EVIDENCE_CHAR_BUDGET`.

Set `LOCOMO_AGENT_REPORT_EVIDENCE=1` when debugging a run. The report will include retrieved evidence snippets and the exact answer-evidence window for each question; leave it disabled for large production runs to keep report files smaller. Reports also include answer-evidence hit rate, labeled answer-evidence precision, average answer-evidence size, and token cost per correct answer to track context precision separately from broad retrieval recall.

To isolate lexical recall from embedding effects in a local smoke run, set `ANIMAOS_RS_MEMORY_EMBEDDINGS=disabled`. Do not use that as the only production signal; it is a comparison tool for checking whether a local embedding model is adding useful signal or ranking noise.

For a paid smoke run, cap the questions and lower the minimum question threshold to the same value so the command remains honest about the smaller sample:

```bash
LOCOMO_AGENT_MAX_QUESTIONS=20 LOCOMO_AGENT_MIN_QUESTIONS=20 NX_TUI=false bun x nx run rust-daemon:memory-locomo-agent --skipNxCache
```

Full Rust core tests:

```bash
NX_TUI=false bun x nx run core-rust:test --skipNxCache
```

Core memory portability check without optional features:

```bash
CARGO_TARGET_DIR=target/validation-memory-no-default cargo test -p anima-memory --no-default-features
```

## Daemon and SDK validation

Rust daemon tests. The isolated target directory avoids Windows file-lock issues when a daemon binary is already running:

```bash
CI=1 CARGO_TARGET_DIR=target/validation-rust-daemon NX_TUI=false bun x nx run rust-daemon:test --skipNxCache
```

TypeScript SDK tests:

```bash
NX_TUI=false bun x nx run @animaOS-SWARM/sdk:test --skipNxCache
```

## Browser playground validation

Playground typecheck:

```bash
NX_TUI=false bun x nx run @animaOS-SWARM/playground:typecheck --skipNxCache
```

Playground build:

```bash
NX_TUI=false bun x nx run @animaOS-SWARM/playground:build --skipNxCache
```

Web UI typecheck:

```bash
NX_TUI=false bun x nx run @animaOS-SWARM/web:typecheck --skipNxCache
```

Web UI build:

```bash
NX_TUI=false bun x nx run @animaOS-SWARM/web:build --skipNxCache
```

## Diff hygiene

```bash
git --no-pager diff --check
```

On Windows this may print LF-to-CRLF warnings. Those warnings are not whitespace failures; conflict markers and trailing whitespace errors are the blocking problems.

## PowerShell environment form

Git Bash accepts inline environment variables like `NX_TUI=false command`. In PowerShell, set environment variables before running the command:

```powershell
$env:NX_TUI="false"
$env:CI="1"
$env:CARGO_TARGET_DIR="target/validation-rust-daemon"
bun x nx run rust-daemon:test --skipNxCache
```

Clear a temporary PowerShell environment variable when you are done:

```powershell
Remove-Item Env:CI
Remove-Item Env:CARGO_TARGET_DIR
```

## Local provider embedding run

Real local multilingual embeddings through `fastembed` download and run an ONNX
model in the Rust daemon process. The default model is
`intfloat/multilingual-e5-small` with 384 dimensions; set
`ANIMAOS_RS_MEMORY_EMBEDDINGS_CACHE_DIR` to control where model files are cached.

Git Bash:

```bash
ANIMAOS_RS_MEMORY_EMBEDDINGS=fastembed \
ANIMAOS_RS_MEMORY_EMBEDDING_MODEL=intfloat/multilingual-e5-small \
bun dev --host rust
```

PowerShell:

```powershell
$env:ANIMAOS_RS_MEMORY_EMBEDDINGS="fastembed"
$env:ANIMAOS_RS_MEMORY_EMBEDDING_MODEL="intfloat/multilingual-e5-small"
bun dev --host rust
```

Ollama remains useful when you want embeddings served by a separate local model
server instead of in-process ONNX inference.

Git Bash:

```bash
ANIMAOS_RS_MEMORY_EMBEDDINGS=ollama \
ANIMAOS_RS_MEMORY_EMBEDDING_MODEL=nomic-embed-text \
ANIMAOS_RS_MEMORY_EMBEDDINGS_BASE_URL=http://127.0.0.1:11434/v1 \
bun dev --host rust
```
PowerShell:

```powershell
$env:ANIMAOS_RS_MEMORY_EMBEDDINGS="ollama"
$env:ANIMAOS_RS_MEMORY_EMBEDDING_MODEL="nomic-embed-text"
$env:ANIMAOS_RS_MEMORY_EMBEDDINGS_BASE_URL="http://127.0.0.1:11434/v1"
bun dev --host rust
```
