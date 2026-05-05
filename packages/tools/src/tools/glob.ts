// @animaOS-SWARM/tools — tools/glob.ts
// File pattern matching tool using Node.js fs.globSync (Node 22+)
// or recursive readdir fallback.

import { readdirSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import type { Action } from "@animaOS-SWARM/core";

export interface GlobArgs {
  pattern: string;
  path?: string;
}

/**
 * Simple glob matching: supports *, **, and ? patterns.
 * Not a full glob implementation but covers the most common cases.
 */
function globMatch(pattern: string, filePath: string): boolean {
  // Convert glob pattern to regex
  let regex = pattern
    .replace(/\./g, "\\.")           // escape dots
    .replace(/\*\*/g, "___GLOBSTAR___") // placeholder
    .replace(/\*/g, "[^/]*")         // * = anything except /
    .replace(/\?/g, "[^/]")          // ? = any single char except /
    .replace(/___GLOBSTAR___/g, ".*"); // ** = anything including /

  // Anchor the pattern
  regex = `^${regex}$`;

  return new RegExp(regex).test(filePath);
}

/**
 * Recursively walk a directory and collect file paths matching the pattern.
 */
function walkDir(dir: string, base: string, pattern: string, results: string[], maxDepth = 20, depth = 0): void {
  if (depth > maxDepth) return;

  let entries: string[];
  try {
    entries = readdirSync(dir);
  } catch {
    return; // permission denied or not a directory
  }

  for (const entry of entries) {
    // Skip hidden dirs and node_modules for performance
    if (entry.startsWith(".") || entry === "node_modules") continue;

    const fullPath = join(dir, entry);
    const relPath = relative(base, fullPath).replace(/\\/g, "/");

    try {
      const stat = statSync(fullPath);
      if (stat.isDirectory()) {
        // Check if directory itself matches (for patterns ending with /)
        if (globMatch(pattern, relPath + "/") || globMatch(pattern, relPath)) {
          // Don't add directories as matches for file globs
        }
        walkDir(fullPath, base, pattern, results, maxDepth, depth + 1);
      } else if (stat.isFile()) {
        if (globMatch(pattern, relPath)) {
          results.push(relPath);
        }
      }
    } catch {
      // stat failed — skip
    }
  }
}

export function executeGlob(args: GlobArgs): {
  status: "success" | "error";
  result: string;
} {
  const { pattern, path = "." } = args;

  const matches: string[] = [];
  walkDir(path, path, pattern, matches);

  if (matches.length === 0) {
    return { status: "success", result: "No files found" };
  }

  matches.sort();
  return { status: "success", result: matches.join("\n") };
}

export const globAction: Action = {
  name: "glob",
  description: "Find files matching a glob pattern.",
  parametersSchema: {
    type: "object",
    properties: {
      pattern: {
        type: "string",
        description: "Glob pattern (e.g. '**/*.ts')",
      },
      path: {
        type: "string",
        description: "Base directory (default: cwd)",
      },
    },
    required: ["pattern"],
  },
  handler: async (_runtime, _message, args) => {
    const result = executeGlob(args as unknown as GlobArgs);
    return {
      status: result.status,
      data: result.result,
      durationMs: 0,
    };
  },
};
