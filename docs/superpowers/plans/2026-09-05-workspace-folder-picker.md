# Workspace folder picker implementation plan

Goal: Let returning users open an existing workspace using the native Windows folder dialog.
Architecture: A local-owner-protected POST endpoint opens one native dialog on a blocking thread. It returns rootPath or null on cancellation. The web reuses inspection and resume; selecting a folder never adopts it until the user confirms the preview. Manual path input remains available on other hosts.

- [x] Add regression coverage for Browse selection, cancellation, errors and stale results.
- [x] Add a Windows-only rfd dependency and isolated folder-picker route with a single-dialog guard, outside the ordinary request timeout and inside API authentication.
- [x] Expose Open existing workspace prominently, Browse in the folder step, inspect selections automatically in resume mode, and hide onboarding progress while resuming.
- [x] Run web tests/typecheck and rust-daemon tests through Nx. Review the final diff.

Files: hosts/rust-daemon/Cargo.toml, Cargo.lock, hosts/rust-daemon/src/routes/folder_picker.rs, hosts/rust-daemon/src/routes/mod.rs, apps/web/src/lib/daemon-api.ts, apps/web/src/components/onboarding/{OnboardingFlow,WorkspaceStep}.tsx and OnboardingFlow.test.tsx.

Validation: 232 full web tests passed; the updated onboarding suite passed 33 tests. Web typecheck and full rust-daemon/core tests passed. Native Windows smoke test confirmed selected path, null cancellation, 409 for a second dialog, and selection surviving the configured one-second HTTP timeout. The dialog permit stays inside the blocking operation across disconnects. Final independent code review found no material issues.

