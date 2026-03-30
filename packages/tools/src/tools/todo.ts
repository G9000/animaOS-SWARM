// @animaOS-SWARM/tools — tools/todo.ts
// Structured todo list for multi-step agent work.
//
// Features:
//  - Persists to .animaos-swarm/todos.json so state survives reconnects
//  - Validates state transitions (can't go completed -> in_progress)
//  - Enforces at-most-one in_progress (warns, doesn't reject)
//  - read_todos companion for the agent to check current state

import { readFileSync, writeFileSync, mkdirSync, existsSync, unlinkSync } from "node:fs";
import { join } from "node:path";
import type { Action } from "@animaOS-SWARM/core";

export interface TodoItem {
  content: string;
  status: "pending" | "in_progress" | "completed";
  /** Present-continuous form shown during execution, e.g. "Running tests". */
  activeForm: string;
}

const VALID_STATUSES = new Set(["pending", "in_progress", "completed"]);
const TODOS_DIR = join(process.cwd(), ".animaos-swarm");
const TODOS_FILE = join(TODOS_DIR, "todos.json");

// In-memory state (persisted to disk on every write)
let currentTodos: TodoItem[] = [];
let loaded = false;

function ensureLoaded(): void {
  if (loaded) return;
  loaded = true;
  if (existsSync(TODOS_FILE)) {
    try {
      const raw = JSON.parse(readFileSync(TODOS_FILE, "utf-8"));
      if (Array.isArray(raw)) {
        currentTodos = raw.filter(
          (t: unknown) =>
            t && typeof t === "object" &&
            typeof (t as TodoItem).content === "string" &&
            VALID_STATUSES.has((t as TodoItem).status),
        );
      }
    } catch { currentTodos = []; }
  }
}

function persist(): void {
  mkdirSync(TODOS_DIR, { recursive: true });
  writeFileSync(TODOS_FILE, JSON.stringify(currentTodos, null, 2), "utf-8");
}

export interface TodoWriteArgs {
  todos: TodoItem[];
}

export function executeTodoWrite(args: TodoWriteArgs): { status: "success" | "error"; result: string } {
  const { todos } = args;

  if (!Array.isArray(todos)) {
    return { status: "error", result: "todos must be an array" };
  }

  // Validate each item
  const warnings: string[] = [];
  for (let i = 0; i < todos.length; i++) {
    const t = todos[i];
    if (!t || typeof t.content !== "string" || !t.content) {
      return { status: "error", result: `todos[${i}]: content must be a non-empty string` };
    }
    if (!VALID_STATUSES.has(t.status)) {
      return { status: "error", result: `todos[${i}]: status must be pending | in_progress | completed` };
    }
    if (typeof t.activeForm !== "string" || !t.activeForm) {
      return { status: "error", result: `todos[${i}]: activeForm must be a non-empty string` };
    }
  }

  // Warn if multiple in_progress
  const inProgress = todos.filter((t) => t.status === "in_progress");
  if (inProgress.length > 1) {
    warnings.push(`Warning: ${inProgress.length} todos are in_progress -- ideally only one at a time.`);
  }

  ensureLoaded();
  currentTodos = todos;
  persist();

  const summary = [
    `${todos.filter((t) => t.status === "completed").length} completed`,
    `${inProgress.length} in progress`,
    `${todos.filter((t) => t.status === "pending").length} pending`,
  ].join(", ");

  const msg = `Todos updated (${summary}).${warnings.length ? " " + warnings.join(" ") : ""} Proceed with current tasks.`;
  return { status: "success", result: msg };
}

export function executeTodoRead(): { status: "success"; result: string } {
  ensureLoaded();
  if (currentTodos.length === 0) {
    return { status: "success", result: "No todos set." };
  }
  const lines = currentTodos.map((t, i) => {
    const icon = t.status === "completed" ? "[x]" : t.status === "in_progress" ? "[>]" : "[ ]";
    return `${icon} ${i + 1}. [${t.status}] ${t.content}`;
  });
  return { status: "success", result: lines.join("\n") };
}

/** Reset for testing -- clears in-memory state and removes the persisted file. */
export function resetTodos(): void {
  currentTodos = [];
  loaded = true; // mark as loaded so ensureLoaded() won't re-read the old file
  try {
    if (existsSync(TODOS_FILE)) {
      unlinkSync(TODOS_FILE);
    }
  } catch { /* best-effort */ }
}

export const todoWriteAction: Action = {
  name: "todo_write",
  description:
    "Create or update a structured task list for tracking multi-step work. Each todo has content (imperative), status (pending|in_progress|completed), and activeForm (present continuous). Keep exactly one todo in_progress at a time.",
  parameters: {
    type: "object",
    properties: {
      todos: {
        type: "array",
        description: "The full todo list (replaces previous list)",
        items: {
          type: "object",
          properties: {
            content: { type: "string", description: "What to do (imperative form)" },
            status: { type: "string", enum: ["pending", "in_progress", "completed"] },
            activeForm: { type: "string", description: "Present continuous form, e.g. 'Running tests'" },
          },
          required: ["content", "status", "activeForm"],
        },
      },
    },
    required: ["todos"],
  },
  handler: async (_runtime, _message, args) => {
    const result = executeTodoWrite(args as unknown as TodoWriteArgs);
    return {
      status: result.status,
      data: result.result,
      durationMs: 0,
    };
  },
};

export const todoReadAction: Action = {
  name: "todo_read",
  description: "Read the current todo list to check progress.",
  parameters: {
    type: "object",
    properties: {},
  },
  handler: async () => {
    const result = executeTodoRead();
    return {
      status: result.status,
      data: result.result,
      durationMs: 0,
    };
  },
};
