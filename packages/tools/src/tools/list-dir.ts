// @animaOS-SWARM/tools — tools/list-dir.ts
// Directory listing tool.

import { readdirSync, statSync, existsSync } from "node:fs";
import { join } from "node:path";
import type { Action } from "@animaOS-SWARM/core";

export interface ListDirArgs {
  path: string;
}

export function executeListDir(args: ListDirArgs): {
  status: "success" | "error";
  result: string;
} {
  const { path } = args;
  if (!existsSync(path)) {
    return { status: "error", result: `Directory not found: ${path}` };
  }
  const entries = readdirSync(path);
  const lines = entries.map((name) => {
    const stat = statSync(join(path, name));
    const prefix = stat.isDirectory() ? "[dir]  " : "[file] ";
    return `${prefix}${name}`;
  });
  return { status: "success", result: lines.join("\n") };
}

export const listDirAction: Action = {
  name: "list_dir",
  description: "List contents of a directory.",
  parametersSchema: {
    type: "object",
    properties: {
      path: { type: "string", description: "Directory path to list" },
    },
    required: ["path"],
  },
  handler: async (_runtime, _message, args) => {
    const result = executeListDir(args as unknown as ListDirArgs);
    return {
      status: result.status,
      data: result.result,
      durationMs: 0,
    };
  },
};
