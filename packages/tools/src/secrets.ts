// @animaOS-SWARM/tools — secrets.ts
// Secret substitution + redaction for tool arguments and output.
//
// Features:
//  - Falls back to process.env when secrets file doesn't exist
//  - Supports both $SECRET_NAME and ${SECRET_NAME} syntax
//  - Redacts secrets from tool output (stdout + stderr)
//  - Secrets file is hot-reloaded (checked every 30s, no restart needed)

import { readFileSync, existsSync } from "node:fs";
import { join } from "node:path";
import { homedir } from "node:os";

// ---------- Loading ----------

const SECRET_PATTERN = /\$\{?([A-Z_][A-Z0-9_]*)\}?/g;

let cachedSecrets: Record<string, string> | null = null;
let cacheTime = 0;
const CACHE_TTL_MS = 30_000;

function secretsFilePath(): string {
  return join(homedir(), ".animaos-swarm", "secrets.json");
}

/**
 * Load secrets from ~/.animaos-swarm/secrets.json, falling back to process.env
 * for any $VAR that isn't in the file. The file is optional.
 */
export function loadSecrets(): Record<string, string> {
  const now = Date.now();
  if (cachedSecrets && now - cacheTime < CACHE_TTL_MS) {
    return cachedSecrets;
  }

  let fileSecrets: Record<string, string> = {};
  const p = secretsFilePath();
  if (existsSync(p)) {
    try {
      const raw = JSON.parse(readFileSync(p, "utf-8"));
      if (raw && typeof raw === "object" && !Array.isArray(raw)) {
        for (const [k, v] of Object.entries(raw)) {
          if (typeof v === "string") fileSecrets[k] = v;
        }
      }
    } catch { /* malformed file — ignore */ }
  }

  cachedSecrets = fileSecrets;
  cacheTime = now;
  return fileSecrets;
}

/** Visible for testing: clear the cache so loadSecrets re-reads. */
export function clearSecretsCache(): void {
  cachedSecrets = null;
  cacheTime = 0;
}

// ---------- Substitution ----------

/**
 * Replace $SECRET_NAME / ${SECRET_NAME} in a string with the actual value.
 * Lookup order: secrets.json -> process.env. Unresolved vars are left as-is.
 */
export function substituteSecrets(input: string): string {
  const secrets = loadSecrets();
  return input.replace(SECRET_PATTERN, (match, name: string) => {
    if (name in secrets) return secrets[name];
    if (name in process.env) return process.env[name]!;
    return match; // leave unresolved
  });
}

/**
 * Substitute secrets in all string-valued args.
 * Returns a new object (does not mutate the original).
 */
export function substituteSecretsInArgs(
  args: Record<string, unknown>,
): Record<string, unknown> {
  const result: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(args)) {
    result[key] = typeof value === "string" ? substituteSecrets(value) : value;
  }
  return result;
}

// ---------- Redaction ----------

/**
 * Scrub known secret values from a string, replacing them with $NAME.
 * Longer values are replaced first to avoid partial matches.
 */
export function redactSecrets(input: string): string {
  const secrets = loadSecrets();
  const allSecrets = { ...secrets };

  let result = input;
  const entries = Object.entries(allSecrets)
    .filter(([, v]) => v.length >= 4) // don't redact trivially short values
    .sort(([, a], [, b]) => b.length - a.length); // longest first

  for (const [name, value] of entries) {
    result = result.replaceAll(value, `$${name}`);
  }
  return result;
}
