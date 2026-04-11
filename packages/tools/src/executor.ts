// @animaOS-SWARM/tools — executor.ts
// Tool dispatcher: validates args, checks permissions, substitutes secrets,
// executes tools, and emits lifecycle hooks.

import { executeBash } from "./tools/bash.js";
import { executeRead } from "./tools/read.js";
import { executeWrite } from "./tools/write.js";
import { executeEdit } from "./tools/edit.js";
import { executeMultiEdit } from "./tools/multi-edit.js";
import { executeGrep } from "./tools/grep.js";
import { executeGlob } from "./tools/glob.js";
import { executeListDir } from "./tools/list-dir.js";
import { executeTodoWrite, executeTodoRead } from "./tools/todo.js";
import {
  executeBgStart,
  executeBgOutput,
  executeBgStop,
  executeBgList,
} from "./tools/process-manager.js";
import { checkPermission, type PermissionDecision } from "./permissions.js";
import { substituteSecretsInArgs, redactSecrets } from "./secrets.js";
import { hooks } from "./hooks.js";
import { validateArgs } from "./validation.js";
import { TOOL_SCHEMA_MAP, MOD_TOOL_MAP } from "./registry.js";

export interface ToolExecuteInput {
  tool_call_id: string;
  tool_name: string;
  args: Record<string, unknown>;
}

export interface ExecutionResult {
  tool_call_id: string;
  status: "success" | "error";
  result: string;
  stdout?: string[];
  stderr?: string[];
}

export type ApprovalCallback = (
  toolName: string,
  args: Record<string, unknown>,
) => Promise<PermissionDecision>;

// Tools where we should substitute $SECRET_NAME in args
const SECRET_TOOLS = new Set(["bash", "bg_start"]);
// Tools where we should redact secrets from output
const REDACT_TOOLS = new Set(["bash", "bg_start", "bg_output"]);

export async function executeTool(
  msg: ToolExecuteInput,
  onApproval?: ApprovalCallback,
): Promise<ExecutionResult> {
  const { tool_call_id, tool_name } = msg;
  let { args } = msg;

  // Secret substitution for shell-like tools
  if (SECRET_TOOLS.has(tool_name)) {
    args = substituteSecretsInArgs(args);
  }

  const decision = checkPermission(tool_name, args);
  if (decision === "ask" && onApproval) {
    const userDecision = await onApproval(tool_name, args);
    if (userDecision === "deny") {
      return {
        tool_call_id,
        status: "error",
        result: "User denied tool execution",
      };
    }
  } else if (decision === "deny") {
    return {
      tool_call_id,
      status: "error",
      result: "Tool execution denied by permission policy",
    };
  }

  // Validate args against schema before dispatch
  const schema = TOOL_SCHEMA_MAP.get(tool_name);
  if (schema) {
    const validationError = validateArgs(tool_name, args, schema.parameters as Record<string, unknown>);
    if (validationError) {
      return { tool_call_id, status: "error", result: validationError };
    }
  }

  // Emit tool:before hook
  await hooks.emit("tool:before", { toolName: tool_name, args, toolCallId: tool_call_id });
  const startTime = Date.now();

  try {
    let result: {
      status: "success" | "error";
      result: string;
      stdout?: string[];
      stderr?: string[];
    };

    // Args come as Record<string, unknown>; cast through unknown to satisfy strict TS.
    const a = args as unknown;

    switch (tool_name) {
      case "bash":
        result = await executeBash(a as Parameters<typeof executeBash>[0]);
        break;
      case "read_file":
        result = executeRead(a as Parameters<typeof executeRead>[0]);
        break;
      case "write_file":
        result = executeWrite(a as Parameters<typeof executeWrite>[0]);
        break;
      case "edit_file":
        result = executeEdit(a as Parameters<typeof executeEdit>[0]);
        break;
      case "grep":
        result = executeGrep(a as Parameters<typeof executeGrep>[0]);
        break;
      case "glob":
        result = executeGlob(a as Parameters<typeof executeGlob>[0]);
        break;
      case "list_dir":
        result = executeListDir(a as Parameters<typeof executeListDir>[0]);
        break;
      case "multi_edit":
        result = executeMultiEdit(
          a as Parameters<typeof executeMultiEdit>[0],
        );
        break;
      case "todo_write":
        result = executeTodoWrite(a as Parameters<typeof executeTodoWrite>[0]);
        break;
      case "todo_read":
        result = executeTodoRead();
        break;
      case "bg_start":
        result = executeBgStart(a as Parameters<typeof executeBgStart>[0]);
        break;
      case "bg_output":
        result = executeBgOutput(a as Parameters<typeof executeBgOutput>[0]);
        break;
      case "bg_stop":
        result = executeBgStop(a as Parameters<typeof executeBgStop>[0]);
        break;
      case "bg_list":
        result = executeBgList();
        break;
      default: {
        const modTool = MOD_TOOL_MAP.get(tool_name);
        if (modTool) {
          const validationError = validateArgs(tool_name, args, modTool.parameters as Record<string, unknown>);
          if (validationError) {
            result = { status: 'error', result: validationError };
            break;
          }
          try {
            const data = await modTool.execute(args);
            result = { status: 'success', result: JSON.stringify(data, null, 2) };
          } catch (err) {
            result = { status: 'error', result: err instanceof Error ? err.message : String(err) };
          }
        } else {
          result = { status: 'error', result: `Unknown tool: ${tool_name}` };
        }
        break;
      }
    }

    // Redact secrets from output of shell-like tools
    if (REDACT_TOOLS.has(tool_name)) {
      result.result = redactSecrets(result.result);
      if (result.stdout) result.stdout = result.stdout.map(redactSecrets);
      if (result.stderr) result.stderr = result.stderr.map(redactSecrets);
    }

    // Emit tool:after hook
    await hooks.emit("tool:after", {
      toolName: tool_name,
      args,
      toolCallId: tool_call_id,
      status: result.status,
      durationMs: Date.now() - startTime,
    });

    return { tool_call_id, ...result };
  } catch (err) {
    // Emit tool:after hook for errors too
    await hooks.emit("tool:after", {
      toolName: tool_name,
      args,
      toolCallId: tool_call_id,
      status: "error",
      durationMs: Date.now() - startTime,
    });

    return {
      tool_call_id,
      status: "error",
      result: err instanceof Error ? err.message : String(err),
    };
  }
}
