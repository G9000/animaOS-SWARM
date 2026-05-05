// @animaOS-SWARM/tools — tools/process-manager.ts
// Background process management for long-running commands (dev servers, watchers, etc.)
//
// The agent can:
//  - bg_start: start a command in the background, get a process ID
//  - bg_output: read incremental output from a background process
//  - bg_stop: kill a background process
//  - bg_list: list all running background processes

import { spawn, type ChildProcess } from "node:child_process";
import type { Action } from "@animaOS-SWARM/core";
import { truncateOutput, LIMITS } from "../truncation.js";
import { getShellLauncher } from "../shell.js";

interface ToolResult {
  status: "success" | "error";
  result: string;
}

interface ManagedProcess {
  id: string;
  command: string;
  cwd: string;
  proc: ChildProcess;
  output: string[];       // ring buffer of lines
  outputCursor: number;   // next unread index for incremental reads
  startedAt: number;
  exitCode: number | null;
}

const MAX_OUTPUT_LINES = 2000;
const processes = new Map<string, ManagedProcess>();
let nextId = 1;

// ---------- bg_start ----------

export interface BgStartArgs {
  command: string;
  cwd?: string;
}

export function executeBgStart(args: BgStartArgs): ToolResult {
  const { command, cwd = process.cwd() } = args;
  const id = `bg-${nextId++}`;

  const launcher = getShellLauncher();
  const proc = spawn(launcher[0], [...launcher.slice(1), command], {
    cwd,
    env: { ...process.env },
    stdio: ["pipe", "pipe", "pipe"],
  });

  const managed: ManagedProcess = {
    id,
    command,
    cwd,
    proc,
    output: [],
    outputCursor: 0,
    startedAt: Date.now(),
    exitCode: null,
  };

  // Stream stdout + stderr into ring buffer
  const pushLines = (text: string) => {
    for (const line of text.split("\n")) {
      if (managed.output.length >= MAX_OUTPUT_LINES) {
        managed.output.shift();
        // adjust cursor so incremental reads stay valid
        if (managed.outputCursor > 0) managed.outputCursor--;
      }
      managed.output.push(line);
    }
  };

  // Read streams in background
  if (proc.stdout) {
    proc.stdout.on("data", (chunk: Buffer) => {
      pushLines(chunk.toString());
    });
  }
  if (proc.stderr) {
    proc.stderr.on("data", (chunk: Buffer) => {
      pushLines("[stderr] " + chunk.toString());
    });
  }

  proc.on("close", (code) => {
    managed.exitCode = code;
  });

  proc.on("error", () => {
    managed.exitCode = -1;
  });

  processes.set(id, managed);

  return {
    status: "success",
    result: `Started background process ${id}: ${command}\nUse bg_output(id: "${id}") to read output, bg_stop(id: "${id}") to kill.`,
  };
}

// ---------- bg_output ----------

export interface BgOutputArgs {
  id: string;
  /** If true, return all output. Otherwise return only new lines since last read. */
  all?: boolean;
}

export function executeBgOutput(args: BgOutputArgs): ToolResult {
  const managed = processes.get(args.id);
  if (!managed) {
    return { status: "error", result: `No background process with id "${args.id}". Use bg_list to see active processes.` };
  }

  const lines = args.all
    ? managed.output
    : managed.output.slice(managed.outputCursor);

  // Advance cursor
  managed.outputCursor = managed.output.length;

  if (lines.length === 0) {
    const alive = managed.exitCode === null;
    return {
      status: "success",
      result: alive
        ? `[${args.id}] No new output. Process still running.`
        : `[${args.id}] No new output. Process exited with code ${managed.exitCode}.`,
    };
  }

  const raw = lines.join("\n");
  const { content } = truncateOutput(raw, {
    maxChars: LIMITS.bash.chars,
    maxLines: LIMITS.bash.lines,
    toolName: `bg-${args.id}`,
  });

  const status = managed.exitCode === null ? "(running)" : `(exited: ${managed.exitCode})`;
  return { status: "success", result: `[${args.id}] ${status}\n${content}` };
}

// ---------- bg_stop ----------

export interface BgStopArgs {
  id: string;
}

export function executeBgStop(args: BgStopArgs): ToolResult {
  const managed = processes.get(args.id);
  if (!managed) {
    return { status: "error", result: `No background process with id "${args.id}".` };
  }

  if (managed.exitCode === null) {
    managed.proc.kill();
  }
  processes.delete(args.id);

  return { status: "success", result: `Stopped and removed ${args.id}.` };
}

// ---------- bg_list ----------

export function executeBgList(): ToolResult {
  if (processes.size === 0) {
    return { status: "success", result: "No background processes running." };
  }

  const lines: string[] = [];
  for (const [id, m] of processes) {
    const alive = m.exitCode === null;
    const elapsed = Math.round((Date.now() - m.startedAt) / 1000);
    const status = alive ? "running" : `exited(${m.exitCode})`;
    lines.push(`${id}  ${status}  ${elapsed}s  ${m.command}`);
  }

  return { status: "success", result: lines.join("\n") };
}

// ---------- Cleanup on exit ----------

export function killAllBackground(): void {
  for (const [, m] of processes) {
    if (m.exitCode === null) {
      try { m.proc.kill(); } catch { /* best-effort */ }
    }
  }
  processes.clear();
}

/** Reset for testing. */
export function resetProcesses(): void {
  killAllBackground();
  nextId = 1;
}

// ---------- Actions ----------

export const bgStartAction: Action = {
  name: "bg_start",
  description:
    "Start a command in the background (dev servers, watchers, builds). Returns a process ID for reading output or stopping later.",
  parametersSchema: {
    type: "object",
    properties: {
      command: {
        type: "string",
        description: "The bash command to run in the background",
      },
      cwd: {
        type: "string",
        description: "Working directory (default: cwd)",
      },
    },
    required: ["command"],
  },
  handler: async (_runtime, _message, args) => {
    const result = executeBgStart(args as unknown as BgStartArgs);
    return {
      status: result.status,
      data: result.result,
      durationMs: 0,
    };
  },
};

export const bgOutputAction: Action = {
  name: "bg_output",
  description:
    "Read output from a background process. By default returns only new lines since last read.",
  parametersSchema: {
    type: "object",
    properties: {
      id: {
        type: "string",
        description: "Process ID from bg_start",
      },
      all: {
        type: "boolean",
        description: "If true, return all output instead of just new lines",
      },
    },
    required: ["id"],
  },
  handler: async (_runtime, _message, args) => {
    const result = executeBgOutput(args as unknown as BgOutputArgs);
    return {
      status: result.status,
      data: result.result,
      durationMs: 0,
    };
  },
};

export const bgStopAction: Action = {
  name: "bg_stop",
  description: "Kill a background process and remove it from the process list.",
  parametersSchema: {
    type: "object",
    properties: {
      id: {
        type: "string",
        description: "Process ID from bg_start",
      },
    },
    required: ["id"],
  },
  handler: async (_runtime, _message, args) => {
    const result = executeBgStop(args as unknown as BgStopArgs);
    return {
      status: result.status,
      data: result.result,
      durationMs: 0,
    };
  },
};

export const bgListAction: Action = {
  name: "bg_list",
  description: "List all background processes with their status and uptime.",
  parametersSchema: {
    type: "object",
    properties: {},
  },
  handler: async () => {
    const result = executeBgList();
    return {
      status: result.status,
      data: result.result,
      durationMs: 0,
    };
  },
};
