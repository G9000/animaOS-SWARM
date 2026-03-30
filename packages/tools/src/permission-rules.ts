// @animaOS-SWARM/tools — permission-rules.ts
// Load user-defined permission rules from project-local or global config.
//
// File locations (checked in order, merged):
//   .animaos-swarm/permissions.json   (project-local)
//   ~/.animaos-swarm/permissions.json (global)
//
// Format:
// {
//   "allow": [
//     "write_file",
//     "bash:npm test",
//     "bash:bun *"
//   ],
//   "deny": [
//     "bash:rm *",
//     "bash:sudo *"
//   ]
// }
//
// Glob-style "*" at the end of bash patterns means "starts with".

import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { homedir } from "node:os";

export interface PermissionRules {
  allow: string[];
  deny: string[];
}

const EMPTY: PermissionRules = { allow: [], deny: [] };

function loadFile(path: string): PermissionRules {
  if (!existsSync(path)) return EMPTY;
  try {
    const raw = JSON.parse(readFileSync(path, "utf-8"));
    return {
      allow: Array.isArray(raw.allow) ? raw.allow.filter((v: unknown) => typeof v === "string") : [],
      deny: Array.isArray(raw.deny) ? raw.deny.filter((v: unknown) => typeof v === "string") : [],
    };
  } catch {
    return EMPTY;
  }
}

/**
 * Load and merge permission rules from project-local and global configs.
 * Deny rules from either source take precedence over allow rules.
 */
export function loadPermissionRules(cwd: string = process.cwd()): PermissionRules {
  const local = loadFile(join(cwd, ".animaos-swarm", "permissions.json"));
  const global = loadFile(join(homedir(), ".animaos-swarm", "permissions.json"));

  return {
    allow: [...local.allow, ...global.allow],
    deny: [...local.deny, ...global.deny],
  };
}

/**
 * Check if a tool+args combo matches a rule pattern.
 *
 * Patterns:
 *   "write_file"       -> matches tool name exactly
 *   "bash:npm test"    -> matches bash with exact command
 *   "bash:npm *"       -> matches bash commands starting with "npm "
 *   "bash:git *"       -> matches bash commands starting with "git "
 */
export function matchesRule(
  rule: string,
  toolName: string,
  args: Record<string, unknown>,
): boolean {
  // Tool-level rule (e.g. "write_file")
  if (!rule.includes(":")) {
    return rule === toolName;
  }

  // Tool:pattern rule (e.g. "bash:npm *")
  const colonIdx = rule.indexOf(":");
  const ruleTool = rule.slice(0, colonIdx);
  const rulePattern = rule.slice(colonIdx + 1);

  if (ruleTool !== toolName) return false;

  if (toolName === "bash") {
    const command = ((args.command as string) || "").trim();
    if (rulePattern.endsWith("*")) {
      const prefix = rulePattern.slice(0, -1);
      return command.startsWith(prefix);
    }
    return command === rulePattern;
  }

  return false;
}
