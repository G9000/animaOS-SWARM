// @animaOS-SWARM/tools — tools/read.ts
// File reading tool with line numbering and offset/limit support.

import { readFileSync, existsSync } from "node:fs";
import type { Action } from "@animaOS-SWARM/core";

export interface ReadArgs {
  file_path: string;
  offset?: number;
  limit?: number;
}

export function executeRead(args: ReadArgs): {
  status: "success" | "error";
  result: string;
} {
  const { file_path, offset = 0, limit = 2000 } = args;
  if (!existsSync(file_path)) {
    return { status: "error", result: `File not found: ${file_path}` };
  }
  const content = readFileSync(file_path, "utf-8");
  const lines = content.split("\n");
  const sliced = lines.slice(offset, offset + limit);
  const numbered = sliced.map(
    (line, i) => `${String(offset + i + 1).padStart(6)}| ${line}`,
  );
  return { status: "success", result: numbered.join("\n") };
}

export const readAction: Action = {
  name: "read_file",
  description: "Read a file and return its contents with line numbers.",
  parameters: {
    type: "object",
    properties: {
      file_path: {
        type: "string",
        description: "Absolute path to the file",
      },
      offset: {
        type: "number",
        description: "Line offset to start reading from",
      },
      limit: {
        type: "number",
        description: "Max lines to read (default: 2000)",
      },
    },
    required: ["file_path"],
  },
  handler: async (_runtime, _message, args) => {
    const result = executeRead(args as unknown as ReadArgs);
    return {
      status: result.status,
      data: result.result,
      durationMs: 0,
    };
  },
};
