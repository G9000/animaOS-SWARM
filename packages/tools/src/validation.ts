// @animaOS-SWARM/tools — validation.ts

/**
 * Validate tool args against a JSON schema.
 * Returns null if valid, or an error message string.
 */
export function validateArgs(
  toolName: string,
  args: Record<string, unknown>,
  schema: Record<string, unknown>,
): string | null {
  const properties = (schema.properties ?? {}) as Record<string, Record<string, unknown>>;
  const required = (schema.required ?? []) as string[];

  // Check required params
  for (const key of required) {
    if (!(key in args) || args[key] === undefined) {
      return `${toolName}: missing required parameter '${key}'`;
    }
  }

  // Check types of provided params
  for (const [key, value] of Object.entries(args)) {
    const propSchema = properties[key];
    if (!propSchema) continue; // unknown param — ignore for forward-compat

    const expectedType = propSchema.type as string | undefined;
    if (!expectedType) continue;

    const typeError = checkType(value, expectedType);
    if (typeError !== null) {
      const article = /^[aeiou]/.test(expectedType) ? "an" : "a";
      return `${toolName}: '${key}' must be ${article} ${expectedType}, received ${typeError}`;
    }

    // Validate array element types
    if (expectedType === "array" && Array.isArray(value)) {
      const itemsSchema = propSchema.items as Record<string, unknown> | undefined;
      const itemType = itemsSchema?.type as string | undefined;
      if (itemType) {
        for (let i = 0; i < value.length; i++) {
          const elemError = checkType(value[i], itemType);
          if (elemError !== null) {
            const art = /^[aeiou]/.test(itemType) ? "an" : "a";
            return `${toolName}: '${key}[${i}]' must be ${art} ${itemType}, received ${elemError}`;
          }
        }
      }
    }
  }

  return null;
}

/**
 * Check whether `value` matches `expectedType`.
 * Returns null if it matches, or the actual type string if it does not.
 */
function checkType(value: unknown, expectedType: string): string | null {
  if (value === null) {
    return expectedType === "null" ? null : "null";
  }
  if (Array.isArray(value)) {
    return expectedType === "array" ? null : "array";
  }
  const t = typeof value;
  if (t === "number") {
    if (expectedType === "integer") {
      return Number.isInteger(value) ? null : "number";
    }
    if (expectedType === "number") {
      return null; // integers are valid numbers
    }
    // value is a number but expected something else — report as "number"
    return "number";
  }
  // string, boolean, object, undefined
  return t === expectedType ? null : t;
}
