# Independent agent conversations and peer communication

Each workspace agent retains its own identity, model, permissions, and messages. The manager is the default UI selection, not a required relay for every conversation.

- [x] Add a host-neutral bounded peer route in Rust core with cycle/depth validation.
- [x] Enable workspace send_message/broadcast_message tools and authenticated sender attribution through the daemon coordinator. Preserve per-target serialization and durable completion. Fail explicitly on busy targets, cycles, capacity, and permission escalation.
- [x] Add stable direct room support and a peer-message HTTP contract; expose both through the SDK with wire tests.
- [x] Add agent switching and independently scoped drafts, histories, settings, and requests to the web UI.
- [x] Verify core/daemon, SDK, and web regression suites and inspect desktop/mobile UI.

Peer communication executes bounded requests and records their results. It does not create an endless autonomous chat loop or automatically grant a recipient capabilities that the originating request lacks. The UI can invoke every agent directly with that agent's own configured permissions.

Verification: full Rust daemon Nx target passed, including core dependencies, 344 daemon unit tests and integration suites. SDK source tests (23), web tests (263), web/SDK/e2e typechecks, web build, and desktop/mobile Chromium peer UI flows passed. Screenshots inspected at 1280px and 390px. Test fixtures made no paid model calls. Fixed a pre-existing restart-test clock mismatch by sharing a manual clock between its store and engine.

Peer routes allow three hops and twelve total requests per originating run. Replies return to callers; busy targets fail promptly. Legacy manager delegation cannot forward through peer tools, preserving its original permission lineage. Restart the daemon to load the new tools and endpoints.
