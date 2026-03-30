// @animaOS-SWARM/tools — hooks.ts
// Lightweight lifecycle hook system for extensibility.
//
// Usage:
//   hooks.on("session:start", (data) => { ... });
//   hooks.on("tool:before", (data) => { ... });
//   hooks.on("tool:after", (data) => { ... });
//   hooks.on("session:end", (data) => { ... });
//   hooks.on("error", (data) => { ... });
//
// All listeners are async-safe; errors in listeners are caught and
// emitted as "error" events (unless the error *is* from an error handler,
// in which case it's silently logged to stderr).

export type HookEvent =
  | "session:start"
  | "session:end"
  | "tool:before"
  | "tool:after"
  | "message:received"
  | "error";

export interface SessionStartData {
  sessionId: string;
  cwd: string;
}

export interface SessionEndData {
  reason: "user" | "disconnect" | "error";
}

export interface ToolBeforeData {
  toolName: string;
  args: Record<string, unknown>;
  toolCallId: string;
}

export interface ToolAfterData {
  toolName: string;
  args: Record<string, unknown>;
  toolCallId: string;
  status: "success" | "error";
  durationMs: number;
}

export interface MessageReceivedData {
  type: string;
  [key: string]: unknown;
}

export interface ErrorData {
  error: Error;
  source: string;
}

type HookDataMap = {
  "session:start": SessionStartData;
  "session:end": SessionEndData;
  "tool:before": ToolBeforeData;
  "tool:after": ToolAfterData;
  "message:received": MessageReceivedData;
  error: ErrorData;
};

type Listener<E extends HookEvent> = (data: HookDataMap[E]) => void | Promise<void>;

export class HookRegistry {
  private listeners = new Map<HookEvent, Array<Listener<HookEvent>>>();

  on<E extends HookEvent>(event: E, listener: Listener<E>): () => void {
    const list = this.listeners.get(event) ?? [];
    list.push(listener as Listener<HookEvent>);
    this.listeners.set(event, list);

    // Return unsubscribe function
    return () => {
      const current = this.listeners.get(event);
      if (current) {
        const idx = current.indexOf(listener as Listener<HookEvent>);
        if (idx >= 0) current.splice(idx, 1);
      }
    };
  }

  async emit<E extends HookEvent>(event: E, data: HookDataMap[E]): Promise<void> {
    const list = this.listeners.get(event);
    if (!list || list.length === 0) return;

    for (const listener of list) {
      try {
        await listener(data);
      } catch (err) {
        // Avoid infinite loop: don't re-emit error events from error handlers
        if (event === "error") {
          process.stderr.write(
            `[hooks] Error in error handler: ${err instanceof Error ? err.message : String(err)}\n`,
          );
        } else {
          await this.emit("error", {
            error: err instanceof Error ? err : new Error(String(err)),
            source: `hook:${event}`,
          });
        }
      }
    }
  }

  /** Remove all listeners (useful for testing) */
  clear(): void {
    this.listeners.clear();
  }

  /** Get listener count for an event */
  listenerCount(event: HookEvent): number {
    return this.listeners.get(event)?.length ?? 0;
  }
}

/** Global hook registry */
export const hooks = new HookRegistry();
