// @animaOS-SWARM/tools — shell.ts
// Cross-platform shell launcher detection.

import { existsSync } from "node:fs";
import { execFileSync } from "node:child_process";

export type ShellLauncher = [executable: string, ...flags: string[]];

let cached: ShellLauncher | null = null;

/** Returns [executable, ...flags] for child_process spawn. Cached for process lifetime. */
export function getShellLauncher(): ShellLauncher {
  if (cached) return cached;
  cached = resolve();
  return cached;
}

/** Reset cache — for testing only. */
export function _resetShellCache(): void {
  cached = null;
}

function resolve(): ShellLauncher {
  const platform = process.platform;

  if (platform === "win32") {
    return resolveWindows();
  }
  if (platform === "darwin") {
    return resolveDarwin();
  }
  return resolveLinux();
}

function resolveDarwin(): ShellLauncher {
  // Prefer zsh on macOS — avoids bash 3.2 HEREDOC apostrophe bug
  const candidates: Array<[string, string[]]> = [
    ["/bin/zsh", ["-lc"]],
    ["/bin/bash", ["-c"]],
  ];
  return probeFirst(candidates);
}

function resolveLinux(): ShellLauncher {
  const candidates: Array<[string, string[]]> = [];

  // Respect $SHELL if set
  const userShell = process.env.SHELL;
  if (userShell) {
    const flags = shellFlags(userShell);
    candidates.push([userShell, flags]);
  }

  candidates.push(
    ["/bin/bash", ["-c"]],
    ["/usr/bin/bash", ["-c"]],
    ["/bin/zsh", ["-c"]],
    ["/bin/sh", ["-c"]],
  );
  return probeFirst(candidates);
}

function resolveWindows(): ShellLauncher {
  const candidates: Array<[string, string[]]> = [];

  // Check for Git Bash first (common on Windows dev machines)
  const gitBashPaths = [
    "C:\\Program Files\\Git\\bin\\bash.exe",
    "C:\\Program Files (x86)\\Git\\bin\\bash.exe",
  ];
  for (const p of gitBashPaths) {
    if (existsSync(p)) {
      candidates.push([p, ["-c"]]);
    }
  }

  // WSL bash
  candidates.push(["bash", ["-c"]]);

  // PowerShell
  candidates.push(
    ["powershell.exe", ["-NoProfile", "-Command"]],
    ["pwsh", ["-NoProfile", "-Command"]],
  );

  // cmd.exe as last resort
  const comspec = process.env.ComSpec || "cmd.exe";
  candidates.push([comspec, ["/d", "/s", "/c"]]);

  return probeFirst(candidates);
}

function shellFlags(shell: string): string[] {
  const base = shell.split("/").pop() || "";
  if (base === "bash" || base === "zsh") return ["-lc"];
  return ["-c"];
}

function probeFirst(candidates: Array<[string, string[]]>): ShellLauncher {
  const tried: string[] = [];
  for (const [exe, flags] of candidates) {
    tried.push(exe);
    if (exe.startsWith("/") || exe.startsWith("C:\\")) {
      if (existsSync(exe)) return [exe, ...flags];
    } else {
      // For non-absolute paths, try to find via which/where
      try {
        execFileSync(process.platform === "win32" ? "where" : "which", [exe], {
          encoding: "utf-8",
          timeout: 3000,
          stdio: ["pipe", "pipe", "pipe"],
        });
        return [exe, ...flags];
      } catch {
        // Not found, try next
      }
    }
  }
  // Fallback: return the first candidate and let it fail at spawn time with a clear error
  if (candidates.length > 0) {
    const [exe, flags] = candidates[0];
    return [exe, ...flags];
  }
  throw new Error(`No shell found. Tried: ${tried.join(", ")}`);
}
