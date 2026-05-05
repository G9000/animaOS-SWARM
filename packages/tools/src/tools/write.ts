// @animaOS-SWARM/tools — tools/write.ts
// File writing tool with automatic directory creation.

import { writeFileSync, mkdirSync, existsSync } from "node:fs";
import { dirname } from "node:path";
import type { Action } from "@animaOS-SWARM/core";

export interface WriteArgs {
  file_path: string;
  content: string;
}

export function executeWrite(args: WriteArgs): {
  status: "success" | "error";
  result: string;
} {
  const { file_path, content } = args;
  const dir = dirname(file_path);
  if (!existsSync(dir)) {
    mkdirSync(dir, { recursive: true });
  }
  writeFileSync(file_path, content, "utf-8");
  return {
    status: "success",
    result: `Wrote ${content.length} chars to ${file_path}`,
  };
}

export const writeAction: Action = {
  name: "write_file",
  description: "Write content to a file, creating directories as needed.",
  parametersSchema: {
    type: "object",
    properties: {
      file_path: {
        type: "string",
        description: "Absolute path to the file",
      },
      content: { type: "string", description: "Content to write" },
    },
    required: ["file_path", "content"],
  },
  handler: async (_runtime, _message, args) => {
    const result = executeWrite(args as unknown as WriteArgs);
    return {
      status: result.status,
      data: result.result,
      durationMs: 0,
    };
  },
};
