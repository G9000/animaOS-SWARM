// @animaOS-SWARM/tools — tools/grep.ts
// Content search tool using ripgrep (with grep fallback).

import { execFileSync } from "node:child_process";
import type { Action } from "@animaOS-SWARM/core";

export interface GrepArgs {
  pattern: string;
  path?: string;
  include?: string;
}

let _hasRg: boolean | null = null;

function hasRipgrep(): boolean {
  if (_hasRg !== null) return _hasRg;
  try {
    execFileSync("rg", ["--version"], { encoding: "utf-8", timeout: 5000 });
    _hasRg = true;
  } catch {
    _hasRg = false;
  }
  return _hasRg;
}

export function executeGrep(args: GrepArgs): {
  status: "success" | "error";
  result: string;
} {
  const { pattern, path = ".", include } = args;

  // Try ripgrep first, fall back to grep
  const useRg = hasRipgrep();

  try {
    if (useRg) {
      const rgArgs = ["--line-number", "--no-heading"];
      if (include) {
        rgArgs.push("--glob", include);
      }
      rgArgs.push(pattern, path);
      const output = execFileSync("rg", rgArgs, {
        encoding: "utf-8",
        maxBuffer: 1024 * 1024,
        timeout: 30000,
      });
      return { status: "success", result: output.slice(0, 50000) };
    }

    // Fallback: grep -rn
    const grepArgs = ["-rn"];
    if (include) {
      grepArgs.push("--include", include);
    }
    grepArgs.push(pattern, path);
    const output = execFileSync("grep", grepArgs, {
      encoding: "utf-8",
      maxBuffer: 1024 * 1024,
      timeout: 30000,
    });
    return { status: "success", result: output.slice(0, 50000) };
  } catch (err: unknown) {
    const execErr = err as { status?: number; message?: string };
    if (execErr.status === 1)
      return { status: "success", result: "No matches found" };
    return { status: "error", result: execErr.message ?? String(err) };
  }
}

export const grepAction: Action = {
  name: "grep",
  description: "Search for a regex pattern across files.",
  parametersSchema: {
    type: "object",
    properties: {
      pattern: {
        type: "string",
        description: "Regex pattern to search for",
      },
      path: {
        type: "string",
        description: "Directory to search in (default: cwd)",
      },
      include: {
        type: "string",
        description: "Glob to filter files (e.g. '*.ts')",
      },
    },
    required: ["pattern"],
  },
  handler: async (_runtime, _message, args) => {
    const result = executeGrep(args as unknown as GrepArgs);
    return {
      status: result.status,
      data: result.result,
      durationMs: 0,
    };
  },
};
