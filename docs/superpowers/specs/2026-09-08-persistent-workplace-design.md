# Persistent workplace foundation

## Approved direction

Anima is a daemon-backed workplace for persistent agents. Human owners define goals and maximum authority; agents own tasks, coordinate, use native coding/document tools, and execute explicitly enabled routines. The web is the management surface. Camera, voice, and device support are future extensions. Model improvements must not require replacing storage, permissions, or execution contracts.

## Delivery boundaries

Preserve existing runtime primitives and workspace resume behavior. First trace actual daemon wiring for tasks, schedules, communication, permissions, memory, native tools, and recovery. Fix demonstrated defects with regression coverage. Publish an honest capability inventory through the daemon and SDK, and surface it in the web. Scaffold extensions through existing portable capability contracts rather than another incompatible plugin registry. Do not pretend that an inventory is a dynamic extension loader.

Rework the web into a practical workplace: clear overview, work, people, files, capabilities, and conversation. Onboarding leads with purpose and useful work; technical setup remains available. No auto-enabled schedules or expanded permissions. Unimplemented camera/voice/browser/document engines must remain explicitly unavailable.

## Persistence and authority

Distinguish durable configuration, memories, task records, and running execution. A saved task does not imply automatic restart continuation. Ambiguous side effects require review rather than blind replay. Delegated tool authority cannot exceed the delegator's current authority. Agent messages must have bounded depth and resource use.

## Validation

Use focused regression tests during implementation and one complete required validation pass after integration. Rerun only changed or failing checks. Do not use paid model calls or external side effects for tests. Record failures and missing capabilities without masking them. Existing memory is evaluated before considering dependencies; no speculative library installation.
