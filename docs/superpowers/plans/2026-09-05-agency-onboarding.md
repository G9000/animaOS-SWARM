# Agency Onboarding Implementation Plan

## Follow-up: Automatic team sizing

- [x] Replace onboarding's fixed four-agent request with an AI-selected size and a configurable maximum of 2–10 total agents, including the manager.
- [x] Add optional maxTeamSize to generation/create contracts. Preserve exact teamSize requests, reject conflicting fields and invalid limits, and validate generated counts on the daemon.
- [x] Report actual generated size; show Team not generated yet before custom generation; keep oversized previews editable after lowering the limit.
- [x] Validate with the full Rust daemon test target, 37 onboarding tests, web typecheck and production build.

## Follow-up: Guided setup UI

- [x] Use Workspace → Model → Team → Manager → Launch; skip Team for manager-only setup.
- [x] Add a responsive progress sidebar and setup summary, compact mobile progress, and sticky navigation.
- [x] Collapse selected templates into a summary with Change template; expand only the specialist being edited.
- [x] Separate team generation/editing from manager behavior and put explicit access controls alongside manager preferences.
- [x] Verify all 259 web tests, web/e2e typechecks, production build, and Chromium flows at 1280px and 390px. Inspect screenshots and correct sticky footer spacing.
- [x] Fix review finding: Team validates specialists only so an empty manager name cannot block returning to Manager for correction. Check cross-team uniqueness on Manager.

**Goal:** Let users start from Marketing Agency, Creator Studio, Life Agency, AI generation, or a blank setup, reviewing an editable full team before creation.

**Architecture:** Reuse the existing five-step onboarding and agency generation endpoint. Add template selection to Workspace and team editing to Agent. Extend workspace bootstrap with optional workers so creation, YAML persistence, rollback and restoration cover the entire team. Existing single-agent and resume flows remain supported. Model and access selection apply to every team member; generated tool grants are never adopted.

**Tech Stack:** React, TypeScript, Vitest, Rust daemon, Nx.

- [x] Add daemon bootstrap worker contract, validation, persistence and failure/restart tests; run rust-daemon:test with isolated validation target if needed.
- [x] Add typed agency generation client and curated templates including reusable starter workflow instructions.
- [x] Add template cards, generation intent, editable/removable specialists, team review and bootstrap integration. Generation runs only after model selection, remains editable, and failure leaves templates available.
- [x] Test template creation payload, worker edits/removal, generation failures/stale results, permissions, and single-agent compatibility.
- [x] Run web tests, typecheck and build, daemon tests, review changes and record verification.

## Acceptance details

Marketing: lead, strategist, copywriter, analyst; campaign brief starter.
Creator Studio: lead, content planner, scriptwriter, community manager; content calendar starter.
Life: chief of staff, planner, research assistant; weekly review starter.
Users may remove every specialist and keep the lead. Names and instructions are required and names must be unique. AI generation is a preview only, never creation. Startup with an already-configured empty workspace retains the existing single-agent recovery flow. No posting, schedules, or external integrations are automatically activated.

## Verification

## Follow-up: Predefined Workspace Manager

The owner requested replacing the generic Agent/personality-generation step with a predefined workspace manager. Keep the main identity Anima (editable), a calm/organized/transparent system role, and configurable Guided/Balanced/Proactive initiative plus Concise/Detailed communication. Preferences supplement the base role; the complete prompt is composed locally and visible in read-only details. The existing Access step alone determines tool grants. Agency generation supplies specialist roles and contextual responsibilities without replacing the manager identity or preferences. Template starters remain included. This changes new onboarding only; existing agent settings are not migrated.

- [x] Replace generic profile form with manager preferences and fixed-role preview.
- [x] Compose and persist manager instructions on both fresh and existing-workspace setup paths.
- [x] Keep agency templates, specialists, starter material and permission selection integrated.
- [x] Verify updated integration tests, desktop/mobile browser presentation, typecheck and build.

Follow-up verification: 254 web tests across 25 files passed. Both 1280px and 390px Chromium flows passed and manager screenshots were inspected. Web and e2e typechecks and production build passed. Review found no blocking issue; the existing-workspace action was clarified to Create manager. No Rust changes in this follow-up.

### Initial agency implementation verification

- Web: 262 tests across 24 files passed with `bun x nx run @animaOS-SWARM/web:test --run --skipNxCache`.
- Web typecheck and production build passed through Nx.
- Rust daemon full Nx test target passed with `CI=1` and `CARGO_TARGET_DIR=target/validation-rust-daemon`; workspace API has 38 passing tests including team restart, YAML resume, rollback, concurrency, limits, and lead identity persistence.
- Chromium browser review flow passed at 1280px and 390px via `bun x nx run @animaOS-SWARM/web-e2e:e2e --args='src/agency-onboarding.spec.ts --project=chromium' --skipNxCache`. Screenshots inspected and horizontal overflow checked. Browser/API generation tests use fixtures; no live paid model call performed.
- Code review findings addressed: lead profile validation, starter retention on profile regeneration, and durable lead selection via settings metadata rather than creation-time ties.
