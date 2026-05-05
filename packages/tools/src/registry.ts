// @animaOS-SWARM/tools — registry.ts
// Tool schemas and registry map.

import type { Action } from "@animaOS-SWARM/core";

import { bashAction } from "./tools/bash.js";
import { readAction } from "./tools/read.js";
import { writeAction } from "./tools/write.js";
import { editAction } from "./tools/edit.js";
import { multiEditAction } from "./tools/multi-edit.js";
import { grepAction } from "./tools/grep.js";
import { globAction } from "./tools/glob.js";
import { listDirAction } from "./tools/list-dir.js";
import { todoWriteAction, todoReadAction } from "./tools/todo.js";
import {
  bgStartAction,
  bgOutputAction,
  bgStopAction,
  bgListAction,
} from "./tools/process-manager.js";
import { webFetchAction } from "./tools/web-fetch.js";

/** All built-in tool actions. */
export const ALL_TOOL_ACTIONS: Action[] = [
  bashAction,
  readAction,
  writeAction,
  editAction,
  multiEditAction,
  grepAction,
  globAction,
  listDirAction,
  todoWriteAction,
  todoReadAction,
  bgStartAction,
  bgOutputAction,
  bgStopAction,
  bgListAction,
  webFetchAction,
];

/** Tool schema type serialized for daemon/model APIs as `parameters`. */
export interface ToolSchema {
  name: string;
  description: string;
  parameters: Record<string, unknown>;
}

/** All tool schemas extracted from actions. */
export const ACTION_TOOL_SCHEMAS: ToolSchema[] = ALL_TOOL_ACTIONS.map((a) => ({
  name: a.name,
  description: a.description,
  parameters: a.parametersSchema,
}));

/** Lookup map built from ACTION_TOOL_SCHEMAS for O(1) access by tool name. */
export const TOOL_SCHEMA_MAP = new Map(
  ACTION_TOOL_SCHEMAS.map((s) => [s.name, s]),
);

/** Lookup map from tool name to Action for O(1) dispatch. */
export const TOOL_ACTION_MAP = new Map(
  ALL_TOOL_ACTIONS.map((a) => [a.name, a]),
);

import type { ModToolHandler } from '@animaOS-SWARM/core';

/** Runtime registry for mod-contributed tools. Keyed by tool name. */
export const MOD_TOOL_MAP = new Map<string, ModToolHandler>();

export function registerModTool(tool: ModToolHandler): void {
  if (TOOL_ACTION_MAP.has(tool.name)) {
    process.stderr.write(`[mod-registry] Mod tool "${tool.name}" conflicts with built-in tool — skipping\n`);
    return;
  }
  if (MOD_TOOL_MAP.has(tool.name)) {
    process.stderr.write(`[mod-registry] Overwriting existing mod tool: "${tool.name}"\n`);
  }
  MOD_TOOL_MAP.set(tool.name, tool);
}

export function registerModTools(tools: ModToolHandler[]): void {
  for (const tool of tools) {
    registerModTool(tool);
  }
}
