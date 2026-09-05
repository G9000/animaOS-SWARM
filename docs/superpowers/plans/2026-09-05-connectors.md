# Connectors implementation plan

Approved design: ../specs/2026-09-05-connectors-design.md

- [x] Add typed SDK Calendar and Mail clients, response types, encoding tests and browser-safe package wiring.
- [x] Add responsive Connectors navigation and four service panels; move Telegram settings; isolate asynchronous owner/service state.
- [x] Implement local draft composition and explicit saved-draft Send/Reject UI; separate inbox failures from connection controls.
- [x] Wire daemon read/draft tools without an agent send/approval capability.
- [x] Implement safe Calendar reconnect with nonce rotation and preservation of same-account pending writes.
- [x] Add same-origin read authorization and exact nonce-authenticated OAuth callback exceptions; omit query strings from request logs.
- [x] Document provider OAuth variables, callback URLs, scope/approval behavior and SDK usage.
- [x] Complete mail manager persistence, refresh, reconnect, disconnect and send-recovery review.
- [x] Verify full Rust tests, SDK/web suites and builds, docs build, isolated daemon API, and restart the existing dev launcher preserving its environment/workspace.

Verification: full Rust suite passed with RUST_TEST_THREADS=1 (314 daemon unit tests plus integration/core suites); Rust formatting passed; SDK 21 tests and web 246 tests passed, with typechecks and builds; docs build passed; desktop/mobile browser checks passed. Isolated daemon verified mail status, owner guard, invalid OAuth callback rejection and seven OpenAPI paths. Fake provider transports cover mail behavior without live account access or sending.

Live launcher restarted with its environment preserved. Its pre-existing ephemeral control plane required reloading the saved anima-workspace/anima.yaml; the idle main agent had zero chat messages before restart and was restored from that file. Gmail and Outlook proxy endpoints return HTTP 200. Provider OAuth applications remain unconfigured. Setup documentation explicitly describes the persistence requirement for restart recovery.
