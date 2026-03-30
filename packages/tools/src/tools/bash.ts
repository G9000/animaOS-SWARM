// @animaOS-SWARM/tools — tools/bash.ts
// Shell command execution tool.

import { spawn } from "node:child_process";
import type { Action } from "@animaOS-SWARM/core";
import { truncateOutput, LIMITS } from "../truncation.js";
import { getShellLauncher } from "../shell.js";

export interface BashArgs {
  command: string;
  timeout?: number;
  cwd?: string;
}

export interface ToolResult {
  status: "success" | "error";
  result: string;
  stdout?: string[];
  stderr?: string[];
}

export async function executeBash(args: BashArgs): Promise<ToolResult> {
  const { command, timeout = 120000, cwd = process.cwd() } = args;

  try {
    const launcher = getShellLauncher();
    const proc = spawn(launcher[0], [...launcher.slice(1), command], {
      cwd,
      env: { ...process.env },
      stdio: ["pipe", "pipe", "pipe"],
    });

    let stdoutText = "";
    let stderrText = "";

    proc.stdout?.on("data", (chunk: Buffer) => {
      stdoutText += chunk.toString();
    });
    proc.stderr?.on("data", (chunk: Buffer) => {
      stderrText += chunk.toString();
    });

    // Race the process against a timeout
    const exitCode = await new Promise<number | null>((resolve, reject) => {
      const timer = setTimeout(() => {
        proc.kill();
        resolve(null); // null = timeout
      }, timeout);

      proc.on("close", (code) => {
        clearTimeout(timer);
        resolve(code);
      });

      proc.on("error", (err) => {
        clearTimeout(timer);
        reject(err);
      });
    });

    if (exitCode === null) {
      return {
        status: "error",
        result: `Command timed out after ${timeout}ms`,
        stdout: [],
        stderr: [],
      };
    }

    const stdoutArr = stdoutText ? [stdoutText] : [];
    const stderrArr = stderrText ? [stderrText] : [];

    // Smart truncation — preserves error-relevant lines in the middle
    const { content: output } = truncateOutput(stdoutText || stderrText, {
      maxChars: LIMITS.bash.chars,
      maxLines: LIMITS.bash.lines,
      toolName: "bash",
    });

    return {
      status: exitCode === 0 ? "success" : "error",
      result: output,
      stdout: stdoutArr,
      stderr: stderrArr,
    };
  } catch (err) {
    return {
      status: "error",
      result: err instanceof Error ? err.message : String(err),
      stdout: [],
      stderr: [],
    };
  }
}

export const bashAction: Action = {
  name: "bash",
  description: "Execute a shell command and return its output.",
  parameters: {
    type: "object",
    properties: {
      command: {
        type: "string",
        description: "The bash command to execute",
      },
      timeout: {
        type: "number",
        description: "Timeout in milliseconds (default: 120000)",
      },
      cwd: {
        type: "string",
        description: "Working directory (default: cwd)",
      },
    },
    required: ["command"],
  },
  handler: async (_runtime, _message, args) => {
    const startTime = Date.now();
    const result = await executeBash(args as unknown as BashArgs);
    return {
      status: result.status,
      data: { result: result.result, stdout: result.stdout, stderr: result.stderr },
      durationMs: Date.now() - startTime,
    };
  },
};
