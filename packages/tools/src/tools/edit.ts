// @animaOS-SWARM/tools — tools/edit.ts
// File editing tool with exact string replacement, over-escape auto-fix,
// and diagnostic error messages.

import { readFileSync, writeFileSync, existsSync } from "node:fs";
import type { Action } from "@animaOS-SWARM/core";
import {
  normalizeLineEndings,
  unescapeOverEscaped,
  buildNotFoundError,
} from "../edit-hints.js";

export interface EditArgs {
  file_path: string;
  old_string: string;
  new_string: string;
}

export function executeEdit(args: EditArgs): {
  status: "success" | "error";
  result: string;
} {
  const { file_path, new_string } = args;
  if (!existsSync(file_path)) {
    return { status: "error", result: `File not found: ${file_path}` };
  }

  // Normalize line endings for cross-platform compatibility
  const content = normalizeLineEndings(readFileSync(file_path, "utf-8"));
  let old_string = normalizeLineEndings(args.old_string);

  // Check for ambiguous matches first
  const occurrences = content.split(old_string).length - 1;
  if (occurrences > 1) {
    return {
      status: "error",
      result: `old_string matches ${occurrences} locations in ${file_path}. Provide more context to disambiguate.`,
    };
  }

  // Exact match — apply it
  if (occurrences === 1) {
    const updated = content.replace(old_string, () => new_string);
    writeFileSync(file_path, updated, "utf-8");
    return { status: "success", result: `Edited ${file_path}` };
  }

  // Not found — try over-escape auto-fix
  const unescaped = unescapeOverEscaped(old_string);
  if (unescaped !== old_string && content.includes(unescaped)) {
    const unescapedOccurrences = content.split(unescaped).length - 1;
    if (unescapedOccurrences === 1) {
      const updated = content.replace(unescaped, () => new_string);
      writeFileSync(file_path, updated, "utf-8");
      return { status: "success", result: `Edited ${file_path}` };
    }
    if (unescapedOccurrences > 1) {
      return {
        status: "error",
        result: `old_string (after fixing escaping) matches ${unescapedOccurrences} locations in ${file_path}. Provide more context to disambiguate.`,
      };
    }
  }

  // Still not found — return diagnostic hint
  return { status: "error", result: buildNotFoundError(file_path, old_string, content) };
}

export const editAction: Action = {
  name: "edit_file",
  description: "Edit a file by replacing old_string with new_string.",
  parameters: {
    type: "object",
    properties: {
      file_path: {
        type: "string",
        description: "Absolute path to the file",
      },
      old_string: {
        type: "string",
        description: "Exact string to find and replace",
      },
      new_string: {
        type: "string",
        description: "Replacement string",
      },
    },
    required: ["file_path", "old_string", "new_string"],
  },
  handler: async (_runtime, _message, args) => {
    const result = executeEdit(args as unknown as EditArgs);
    return {
      status: result.status,
      data: result.result,
      durationMs: 0,
    };
  },
};
