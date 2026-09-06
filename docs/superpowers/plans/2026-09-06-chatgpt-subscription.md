# ChatGPT Subscription Implementation Plan

**Goal:** Sign in to ChatGPT from Anima's web settings and use subscription inference with Anima's existing agent loop and tools.

**Architecture:** A daemon-owned OAuth service performs device authorization, stores credentials in the existing secure host vault, refreshes tokens, and exposes redacted status. A reusable Responses adapter normalizes subscription inference into Anima model responses. The web exposes connect, cancel, disconnect, account state, and subscription model selection. Subscription requests never fall back to API-key billing.

**Tech stack:** Rust, Axum, reqwest, existing vault, TypeScript SDK, React, Nx.

## Tasks
- [x] Verify direct subscription Responses protocol against upstream sources; keep auth and host lifecycle outside reusable core.
- [x] Add isolated OAuth lifecycle tests, implement device sign-in and secure persistence, guard all account routes with local-owner authorization and no-store.
- [x] Add reusable Responses mapping and stream tests covering text, tool calls/results, malformed streams, errors, and usage; implement adapter.
- [x] Integrate shared auth into runtime model selection and provider catalog, keeping deterministic test routers isolated.
- [x] Add typed SDK and web connection controls, pending/expired/error states, and model selection in settings and onboarding; test interactions.
- [x] Run Rust host tests and relevant SDK/web Nx checks; independent review and browser QA. Real account authorization is completed by the user.

## Contract
Provider id: `chatgpt`. GET `/api/providers/chatgpt/status` returns `{connected:boolean, accountId:string|null, planType:string|null, login:{userCode:string,verificationUrl:string,expiresAtMs:number}|null,error:string|null}`. POST `/api/providers/chatgpt/login` starts or returns active device login and returns status. DELETE `/api/providers/chatgpt/login` cancels pending login and returns status. DELETE `/api/providers/chatgpt` disconnects and returns status. All routes return no-store; errors use `{error:string}`. No tokens returned to the browser. Device polling is owned by the daemon and expires after 15 minutes.

## Validation
`bun x nx run rust-daemon:test --skipNxCache` with `CI=1` and `CARGO_TARGET_DIR=target/validation-rust-daemon` when the running daemon locks the default target. Discover SDK/web target definitions with `nx show project --json` and run their test/typecheck or build targets. Do not claim a live subscription turn was verified before account authorization.

## Results and activation

Rust host/core Nx suite passed with `RUST_TEST_THREADS=1`; the first parallel run hit an existing swarm fixture's very short creation timeout. SDK 31 tests and web 283 tests passed; SDK/web build and typecheck passed. Browser QA against an isolated daemon successfully started real device authorization and cancelled it. No live account or paid inference was used.

Independent review led to fixes for same-origin status authorization, login/refresh races, deterministic network isolation, Windows keyring payload limits, and retryable chunk cleanup. Credential chunks are committed through an atomic manifest with a bounded cleanup journal.

The user subsequently activated the integration and connected their account. Blank replies exposed a stream assembly bug: terminal events can omit output already delivered by `response.output_item.done`. The adapter now retains completed items in output order, avoids duplicating full terminal output, and rejects genuinely empty responses. Regression coverage includes text, tool calls, empty/null terminal output, and truncated streams. An explicit ignored live smoke test returned a nonempty reply using the saved subscription; normal tests never use real account credentials.

The existing daemon at 8080 reports ephemeral control-plane state. Loading the repaired adapter requires restarting that process, which would discard its current in-memory state. The running process has been preserved; code is on `codex/chatgpt-subscription`.
