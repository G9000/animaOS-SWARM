// @animaOS-SWARM/tools — permissions.ts
// Permission checking for tool execution.

import { resolve, relative } from "node:path";
import { loadPermissionRules, matchesRule, type PermissionRules } from "./permission-rules.js";

export type PermissionDecision = "allow" | "deny" | "ask";

const READ_ONLY_TOOLS = new Set(["read_file", "grep", "glob", "list_dir"]);
const WRITE_TOOLS = new Set(["write_file", "edit_file", "multi_edit"]);

const SAFE_BASH_PATTERNS = [
  /^(ls|pwd|echo|cat|head|tail|wc|date|whoami|which|type|file)\b/,
  /^git\s+(status|log|diff|branch|show|remote|tag)\b/,
  /^(node|python|bun|npm|pip)\s+--version$/,
];

const DANGEROUS_BASH_PATTERNS = [
  /^(rm|rmdir)\s/,
  /^sudo\b/,
  /^git\s+(push|reset|rebase|force)/,
  /^(chmod|chown)\s/,
  /\|\s*sh\b/,
  />\s*\/dev\/sd/,
];

const sessionRules: Set<string> = new Set();

export function addSessionRule(rule: string): void {
  sessionRules.add(rule);
}

export function clearSessionRules(): void {
  sessionRules.clear();
}

// Lazily loaded file-based rules (loaded once per process)
let _fileRules: PermissionRules | null = null;

function getFileRules(): PermissionRules {
  if (!_fileRules) {
    _fileRules = loadPermissionRules();
  }
  return _fileRules;
}

/** Force-reload file rules (useful after config changes) */
export function reloadFileRules(): void {
  _fileRules = null;
}

export function checkPermission(
  toolName: string,
  args: Record<string, unknown>,
): PermissionDecision {
  if (toolName === "ask_user") return "allow";
  if (READ_ONLY_TOOLS.has(toolName)) return "allow";
  if (sessionRules.has(toolName)) return "allow";

  const rules = getFileRules();

  // File-based deny rules take highest priority (after read-only)
  if (rules.deny.some((r) => matchesRule(r, toolName, args))) {
    return "deny";
  }

  // File-based allow rules
  if (rules.allow.some((r) => matchesRule(r, toolName, args))) {
    return "allow";
  }

  if (WRITE_TOOLS.has(toolName)) {
    const filePath = args.file_path as string | undefined;
    if (filePath) {
      const rel = relative(process.cwd(), resolve(filePath));
      if (rel.startsWith("..")) return "ask";
    }
    return "allow";
  }

  if (toolName === "bash") {
    const command = ((args.command as string) || "").trim();
    if (sessionRules.has(`bash:${command}`)) return "allow";
    if (SAFE_BASH_PATTERNS.some((p) => p.test(command))) return "allow";
    if (DANGEROUS_BASH_PATTERNS.some((p) => p.test(command))) return "ask";
    return "ask";
  }

  return "ask";
}
