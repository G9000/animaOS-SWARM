// @animaOS-SWARM/tools — main entry point
// Exports all tools, utilities, and infrastructure.

// ── Tools (execution functions + Action objects) ──
export * from "./tools/index.js";

// ── Hook system ──
export { HookRegistry, hooks } from "./hooks.js";
export type {
  HookEvent,
  SessionStartData,
  SessionEndData,
  ToolBeforeData,
  ToolAfterData,
  MessageReceivedData,
  ErrorData,
} from "./hooks.js";

// ── Executor (tool dispatcher) ──
export { executeTool } from "./executor.js";
export type { ToolExecuteInput, ExecutionResult, ApprovalCallback } from "./executor.js";

// ── Registry (schemas + lookup maps) ──
export {
  ALL_TOOL_ACTIONS,
  ACTION_TOOL_SCHEMAS,
  TOOL_SCHEMA_MAP,
  TOOL_ACTION_MAP,
} from "./registry.js";
export type { ToolSchema } from "./registry.js";

// ── Permissions ──
export {
  checkPermission,
  addSessionRule,
  clearSessionRules,
  reloadFileRules,
} from "./permissions.js";
export type { PermissionDecision } from "./permissions.js";
export { loadPermissionRules, matchesRule } from "./permission-rules.js";
export type { PermissionRules } from "./permission-rules.js";

// ── Secrets ──
export {
  loadSecrets,
  clearSecretsCache,
  substituteSecrets,
  substituteSecretsInArgs,
  redactSecrets,
} from "./secrets.js";

// ── Validation ──
export { validateArgs } from "./validation.js";

// ── Truncation ──
export {
  truncateOutput,
  truncateItems,
  writeOverflow,
  cleanupOverflow,
  LIMITS,
} from "./truncation.js";
export type { TruncateResult, TruncateOpts, ToolLimitKey } from "./truncation.js";

// ── Shell ──
export { getShellLauncher } from "./shell.js";
export type { ShellLauncher } from "./shell.js";

// ── Edit hints ──
export {
  normalizeLineEndings,
  unescapeOverEscaped,
  buildNotFoundError,
} from "./edit-hints.js";
